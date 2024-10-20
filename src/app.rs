//! Provides the core Humpty app functionality.

use crate::http::cors::Cors;
use crate::http::date::DateTime;
use crate::http::headers::HeaderType;
use crate::http::method::Method;
use crate::http::request::{Request, RequestError};
use crate::http::response::Response;
use crate::http::status::StatusCode;
use crate::krauss::wildcard_match;
use crate::monitor::event::{Event, EventType};
use crate::monitor::MonitorConfig;
use crate::route::{Route, RouteHandler, SubApp};
use crate::thread::pool::ThreadPool;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Represents the Humpty app.
pub struct App {
  thread_pool: ThreadPool,
  subapps: Vec<SubApp>,
  default_subapp: SubApp,
  error_handler: ErrorHandler,
  monitor: MonitorConfig,
  connection_condition: ConnectionCondition,
  connection_timeout: Option<Duration>,
  shutdown: Option<Receiver<()>>,
}

/// Represents a function able to calculate whether a connection will be accepted.
pub type ConnectionCondition = fn(&mut TcpStream) -> bool;

pub use crate::handler_traits::*;
use crate::stream::{ConnectionStream, IntoConnectionStream};

/// Represents a function able to handle an error.
/// The first parameter of type `Option<Request>` will be `Some` if the request could be parsed.
/// Otherwise, it will be `None` and the status code will be `StatusCode::BadRequest`.
///
/// Every app has a default error handler, which simply displays the status code.
/// The source code for this default error handler is copied below since it is a good example.
///
pub type ErrorHandler = fn(StatusCode) -> Response;

/// Represents a generic error with the program.
pub type HumptyError = Box<dyn std::error::Error>;

impl Default for App {
  /// Initialises a new Humpty app.
  fn default() -> Self {
    Self {
      thread_pool: ThreadPool::new(32),
      subapps: Vec::new(),
      default_subapp: SubApp::default(),
      error_handler,
      monitor: MonitorConfig::default(),
      connection_condition: |_| true,
      connection_timeout: None,
      shutdown: None,
    }
  }
}

impl App {
  /// Initialises a new Humpty app with the given configuration options.
  pub fn new_with_config(threads: usize) -> Self {
    Self { thread_pool: ThreadPool::new(threads), ..Default::default() }
  }

  /// Runs the Humpty app on the given socket address.
  /// This function will only return if a fatal error is thrown such as the port being in use.
  pub fn run<A>(mut self, addr: A) -> Result<(), HumptyError>
  where
    A: ToSocketAddrs + Clone,
  {
    let socket = TcpListener::bind(addr.clone())?;
    let subapps = Arc::new(self.subapps);
    let default_subapp = Arc::new(self.default_subapp);
    let error_handler = Arc::new(self.error_handler);

    self.thread_pool.register_monitor(self.monitor.clone());
    self.thread_pool.start();

    // Shared shutdown signal between socket.incoming() and shutdown signal receiver.
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();
    let main_app_thread = thread::spawn(move || {
      for stream in socket.incoming() {
        if shutdown_clone.load(Ordering::SeqCst) {
          break;
        }

        match stream {
          Ok(mut stream) => {
            // Check that the client is allowed to connect
            if (self.connection_condition)(&mut stream) {
              let cloned_monitor = self.monitor.clone();
              let cloned_subapps = subapps.clone();
              let cloned_default_subapp = default_subapp.clone();
              let cloned_error_handler = error_handler.clone();
              let cloned_timeout = self.connection_timeout;

              cloned_monitor.send(
                Event::new(EventType::ConnectionSuccess).with_peer_result(stream.peer_addr()),
              );

              // Spawn a new thread to handle the connection
              self.thread_pool.execute(move || {
                cloned_monitor.send(
                  Event::new(EventType::ThreadPoolProcessStarted)
                    .with_peer_result(stream.peer_addr()),
                );

                client_handler(
                  stream,
                  cloned_subapps,
                  cloned_default_subapp,
                  cloned_error_handler,
                  cloned_monitor,
                  cloned_timeout,
                )
              });
            } else {
              self
                .monitor
                .send(Event::new(EventType::ConnectionDenied).with_peer_result(stream.peer_addr()));
            }
          }
          Err(e) => {
            self.monitor.send(Event::new(EventType::ConnectionError).with_info(e.to_string()))
          }
        }
      }
      self.thread_pool.stop();
    });

    if let Some(s) = self.shutdown {
      // We wait for the shutdown signal, then wake up the main app thread with a new connection
      let _ = s.recv();
      shutdown.store(true, Ordering::SeqCst);
      let _ = TcpStream::connect(unspecified_socket_to_loopback(addr));
    };

    let _ = main_app_thread.join();

    Ok(())
  }

  /// Adds a new host sub-app to the server.
  /// The host can contain wildcards, for example `*.example.com`.
  ///
  /// ## Panics
  /// This function will panic if the host is equal to `*`, since this is the default host.
  /// If you want to add a route to every host, simply add it directly to the main app.
  pub fn with_host(mut self, host: &str, mut handler: SubApp) -> Self {
    if host == "*" {
      panic!("Cannot add a sub-app with wildcard `*`");
    }

    handler.host = host.to_string();
    self.subapps.push(handler);

    self
  }

  /// Adds a route and associated handler to the server.
  /// Routes can include wildcards, for example `/blog/*`.
  pub fn with_route<T>(mut self, route: &str, handler: T) -> Self
  where
    T: RequestHandler + 'static,
  {
    self.default_subapp = self.default_subapp.with_route(route, handler);
    self
  }

  /// Adds a path-aware route and associated handler to the server.
  /// Routes can include wildcards, for example `/blog/*`.
  /// Will also pass the route to the handler at runtime.
  pub fn with_path_aware_route<T>(mut self, route: &'static str, handler: T) -> Self
  where
    T: PathAwareRequestHandler + 'static,
  {
    self.default_subapp = self.default_subapp.with_path_aware_route(route, handler);
    self
  }

  /// Adds a WebSocket route and associated handler to the server.
  /// Routes can include wildcards, for example `/ws/*`.
  /// The handler is passed the stream and the request which triggered its calling.
  pub fn with_websocket_route<T>(mut self, route: &str, handler: T) -> Self
  where
    T: WebsocketHandler + 'static,
  {
    self.default_subapp = self.default_subapp.with_websocket_route(route, handler);
    self
  }

  /// Sets the default sub-app for the server.
  /// This overrides all the routes added, as they will be replaced by the routes in the default sub-app.
  pub fn with_default_subapp(mut self, subapp: SubApp) -> Self {
    self.default_subapp = subapp;
    self
  }

  /// Registers a monitor for the server.
  pub fn with_monitor(mut self, monitor: MonitorConfig) -> Self {
    self.monitor = monitor;
    self
  }

  /// Registers a shutdown signal to gracefully shutdown the app, ending the run/run_tls loop.
  pub fn with_shutdown(mut self, shutdown_receiver: Receiver<()>) -> Self {
    self.shutdown = Some(shutdown_receiver);
    self
  }

  /// Sets the error handler for the server.
  pub fn with_error_handler(mut self, handler: ErrorHandler) -> Self {
    self.error_handler = handler;
    self
  }

  /// Sets the connection condition, a function which decides whether to accept the connection.
  /// For example, this could be used for implementing whitelists and blacklists.
  pub fn with_connection_condition(mut self, condition: ConnectionCondition) -> Self {
    self.connection_condition = condition;
    self
  }

  /// Sets the connection timeout, the amount of time to wait between keep-alive requests.
  pub fn with_connection_timeout(mut self, timeout: Option<Duration>) -> Self {
    self.connection_timeout = timeout;
    self
  }

  /// Sets the CORS configuration for the app.
  ///
  /// This overrides the CORS configuration for existing and future individual routes.
  ///
  /// To simply allow every origin, method and header, use `Cors::wildcard()`.
  pub fn with_cors(mut self, cors: Cors) -> Self {
    self.default_subapp = self.default_subapp.with_cors(cors);
    self
  }

  /// Sets the CORS configuration for the specified route.
  ///
  /// To simply allow every origin, method and header, use `Cors::wildcard()`.
  pub fn with_cors_config(mut self, route: &str, cors: Cors) -> Self {
    self.default_subapp = self.default_subapp.with_cors_config(route, cors);
    self
  }
}

/// Handles a connection with a client.
/// The connection will be opened upon the first request and closed as soon as a request is
///   received without the `Connection: Keep-Alive` header.
fn client_handler<T: IntoConnectionStream>(
  stream: T,
  subapps: Arc<Vec<SubApp>>,
  default_subapp: Arc<SubApp>,
  error_handler: Arc<ErrorHandler>,
  monitor: MonitorConfig,
  timeout: Option<Duration>,
) {
  let stream = stream.into_connection_stream();

  let addr = if let Ok(addr) = stream.peer_addr() {
    addr
  } else {
    monitor.send(EventType::StreamDisconnectedWhileWaiting);
    return;
  };

  loop {
    // Parses the request from the stream
    let request = match timeout {
      Some(timeout) => Request::from_stream_with_timeout(stream.as_ref(), addr.clone(), timeout),
      None => Request::from_stream(stream.as_ref(), addr.clone()),
    };

    // If the request is valid an is a WebSocket request, call the corresponding handler
    if let Ok(req) = &request {
      if req.headers.get(&HeaderType::Upgrade) == Some("websocket") {
        monitor.send(Event::new(EventType::WebsocketConnectionRequested).with_peer(addr.clone()));

        call_websocket_handler(req, &subapps, &default_subapp, stream.as_ref());

        monitor.send(Event::new(EventType::WebsocketConnectionClosed).with_peer(addr.clone()));
        break;
      }
    }

    // Get the keep alive information from the request before it is consumed by the handler
    let keep_alive = if let Ok(request) = &request {
      if let Some(connection) = request.headers.get(&HeaderType::Connection) {
        connection.to_ascii_lowercase() == "keep-alive"
      } else {
        false
      }
    } else {
      false
    };

    // Generate the response based on the handlers
    let response = match &request {
      Ok(request) if request.method == Method::Options => {
        let handler = get_handler(request, &subapps, &default_subapp);

        match handler {
          Some(handler) => {
            let mut response = Response::empty(StatusCode::NoContent)
              .with_header(HeaderType::Date, DateTime::now().to_string())
              .with_header(HeaderType::Server, "Humpty")
              .with_header(
                HeaderType::Connection,
                match keep_alive {
                  true => "Keep-Alive",
                  false => "Close",
                },
              );

            handler.cors.set_headers(&mut response.headers);

            response
          }
          None => error_handler(StatusCode::NotFound),
        }
      }
      Ok(request) => {
        let handler = get_handler(request, &subapps, &default_subapp);

        let mut response = match handler {
          Some(handler) => {
            let mut response: Response = handler.handler.serve(request.clone());

            handler.cors.set_headers(&mut response.headers);

            response
          }
          None => error_handler(StatusCode::NotFound),
        };

        // Automatically generate required headers
        match response.headers.get_mut(HeaderType::Connection) {
          Some(_) => (),
          None => {
            if let Some(connection) = &request.headers.get(&HeaderType::Connection) {
              response.headers.add(HeaderType::Connection, connection);
            } else {
              response.headers.add(HeaderType::Connection, "Close");
            }
          }
        }

        match response.headers.get_mut(HeaderType::Server) {
          Some(_) => (),
          None => {
            response.headers.add(HeaderType::Server, "Humpty");
          }
        }

        match response.headers.get_mut(HeaderType::Date) {
          Some(_) => (),
          None => {
            response.headers.add(HeaderType::Date, DateTime::now().to_string());
          }
        }

        // Set HTTP version
        response.version.clone_from(&request.version);
        response
      }
      Err(e) => match e {
        RequestError::Request => error_handler(StatusCode::BadRequest),
        RequestError::Timeout => error_handler(StatusCode::RequestTimeout),
        RequestError::Disconnected => return,
        RequestError::Stream => return monitor.send(Event::new(EventType::RequestServedError)),
      },
    };

    // Write the response to the stream
    let status = response.status_code.clone();

    if let Err(e) = response.write_to(stream.as_stream_write()) {
      monitor.send(
        Event::new(EventType::RequestServedError).with_peer(addr.clone()).with_info(e.to_string()),
      );

      break;
    };

    // if let Err(e) = stream.flush() {
    //   monitor.send(
    //     Event::new(EventType::RequestServedError).with_peer(addr.clone()).with_info(e.to_string()),
    //   );
    //
    //   break;
    // };

    let status_str: &str = status.status_line();

    match &status {
      StatusCode::OK => monitor.send(
        Event::new(EventType::RequestServedSuccess)
          .with_peer(addr.clone())
          .with_info(format!("200 OK {}", request.unwrap().uri)),
      ),
      StatusCode::RequestTimeout => monitor.send(
        Event::new(EventType::RequestTimeout)
          .with_peer(addr.clone())
          .with_info("408 Request Timeout"),
      ),
      e => {
        if let Ok(request) = request {
          monitor.send(
            Event::new(EventType::RequestServedError).with_peer(addr.clone()).with_info(format!(
              "{} {} {}",
              e.code(),
              status_str,
              request.uri
            )),
          )
        } else {
          monitor.send(
            Event::new(EventType::RequestServedError).with_peer(addr.clone()).with_info(format!(
              "{} {}",
              e.code(),
              status_str
            )),
          )
        }
      }
    }

    // If the request specified to keep the connection open, respect this
    if !keep_alive {
      break;
    }

    monitor.send(Event::new(EventType::KeepAliveRespected).with_peer(addr.clone()));
  }

  monitor.send(Event::new(EventType::ConnectionClosed).with_peer(addr.clone()));
}

/// Gets the correct handler for the given request.
pub(crate) fn get_handler<'a>(
  request: &'a Request,
  subapps: &'a [SubApp],
  default_subapp: &'a SubApp,
) -> Option<&'a RouteHandler> {
  // Iterate over the sub-apps and find the one which matches the host
  if let Some(host) = request.headers.get(&HeaderType::Host) {
    if let Some(subapp) = subapps.iter().find(|subapp| wildcard_match(&subapp.host, host)) {
      // If the sub-app has a handler for this route, call it
      if let Some(handler) = subapp
        .routes // Get the routes of the sub-app
        .iter() // Iterate over the routes
        .find(|route| route.route.route_matches(&request.uri))
      // Find the route that matches
      {
        return Some(handler);
      }
    }
  }

  // If no sub-app was found, try to use the handler on the default sub-app
  if let Some(handler) =
    default_subapp.routes.iter().find(|route| route.route.route_matches(&request.uri))
  {
    return Some(handler);
  }

  None
}

/// Calls the correct WebSocket handler for the given request.
fn call_websocket_handler(
  request: &Request,
  subapps: &[SubApp],
  default_subapp: &SubApp,
  stream: &dyn ConnectionStream,
) {
  // Iterate over the sub-apps and find the one which matches the host
  if let Some(host) = request.headers.get(&HeaderType::Host) {
    if let Some(subapp) = subapps.iter().find(|subapp| wildcard_match(&subapp.host, host)) {
      // If the sub-app has a handler for this route, call it
      if let Some(handler) = subapp
        .websocket_routes // Get the WebSocket routes of the sub-app
        .iter() // Iterate over the routes
        .find(|route| route.route.route_matches(&request.uri))
      {
        handler.handler.serve(request.clone(), stream.new_ref());
        return;
      }
    }
  }

  // If no sub-app was found, try to use the handler on the default sub-app
  if let Some(handler) =
    default_subapp.websocket_routes.iter().find(|route| route.route.route_matches(&request.uri))
  {
    handler.handler.serve(request.clone(), stream.new_ref())
  }
}

/// The default error handler for every Humpty app.
/// This can be overridden by using the `with_error_handler` method when building the app.
pub(crate) fn error_handler(status_code: StatusCode) -> Response {
  // TODO change this to http 500 without body.
  // This code makes the assumption that we are talking to a browser, this is a dangerous assumption to make.
  // We would if we want to send html check if the Accept header of the request even allows for html.
  // This is effort I want to outsource to the user of the library, as for 99% of cases this is probably a wrong guess.
  let body = format!(
    "<html><body><h1>{} {}</h1></body></html>",
    status_code.code(),
    status_code.status_line(),
  );

  Response::new(status_code, body.as_bytes())
}

fn unspecified_socket_to_loopback<S>(socket: S) -> SocketAddr
where
  S: ToSocketAddrs,
{
  let mut socket = socket.to_socket_addrs().unwrap().next().unwrap(); // This can't fail, because the server was able to start.
  if socket.ip().is_unspecified() {
    match socket.ip() {
      IpAddr::V4(_) => socket.set_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
      IpAddr::V6(_) => socket.set_ip(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0x1))),
    };
  }
  socket
}
