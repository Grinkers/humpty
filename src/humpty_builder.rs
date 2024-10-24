//! Provides the core Humpty app functionality.

use crate::http::response::Response;
use crate::http::status::StatusCode;
use crate::route::SubApp;
use crate::thread::pool::ThreadPool;

#[cfg(feature = "log")]
use log::trace;
use log::{error, info};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;
use std::{io, thread};

/// Represents the Humpty app.
pub struct HumptyBuilder {
  thread_pool: ThreadPool,
  subapps: Vec<SubApp>,
  default_subapp: SubApp,
  error_handler: ErrorHandler,
  not_found_handler: NotFoundHandler,
  connection_timeout: Option<Duration>,
  shutdown: Option<Receiver<()>>,
}

pub use crate::handler_traits::*;
use crate::http::Request;
use crate::humpty_server::HumptyServer;

/// Represents a function able to handle an error.
/// The first parameter of type `Option<Request>` will be `Some` if the request could be parsed.
/// Otherwise, it will be `None` and the status code will be `StatusCode::BadRequest`.
///
/// Every app has a default error handler, which simply displays the status code.
/// The source code for this default error handler is copied below since it is a good example.
///
pub type ErrorHandler = fn(&Request, io::Error) -> io::Result<Response>;

pub type NotFoundHandler = fn(&Request) -> io::Result<Response>;

impl Default for HumptyBuilder {
  /// Initialises a new Humpty app.
  fn default() -> Self {
    Self {
      // TODO remove this Only used by run() call.
      // This should be the users responsibility to implement himself or use any crate if he so desired.
      // Desired behavior is that requests get served in whatever thread supplies the connection.
      // This would even permit using humpty in a non SMP environment where 1 request is handled at a time.
      // The only reason we may still require some reference to a thread pool is for async websockets "maybe"
      // For "Sync" websockets the endpoints can just start a thread if full duplex is desired.
      // For ordinary http 1.1 its not needed.
      #[deprecated]
      thread_pool: ThreadPool::new(32),
      subapps: Vec::new(),
      default_subapp: SubApp::default(),
      error_handler: default_error_handler,
      not_found_handler: default_not_found_handler,
      connection_timeout: None,
      shutdown: None,
    }
  }
}

impl HumptyBuilder {
  /// Initialises a new Humpty app with the given configuration options.
  pub fn new_with_config(threads: usize) -> Self {
    Self { thread_pool: ThreadPool::new(threads), ..Default::default() }
  }

  /// Runs the Humpty app on the given socket address.
  /// This function will only return if a fatal error is thrown such as the port being in use.
  //#[deprecated] //TODO remove this method its hardwired to TCP, an assumption I do not want to make.
  pub fn run<A>(mut self, addr: A) -> io::Result<()>
  where
    A: ToSocketAddrs + Clone,
  {
    let socket = TcpListener::bind(addr.clone())?;
    let server = Arc::new(HumptyServer::new(
      self.default_subapp,
      self.subapps,
      self.error_handler,
      self.not_found_handler,
      self.connection_timeout,
    ));
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
          Ok(stream) => {
            // Check that the client is allowed to connect
            #[cfg(feature = "log")]
            trace!("ConnectionSuccess {:?}", stream.peer_addr());
            let server_clone = server.clone();
            // Spawn a new thread to handle the connection
            self.thread_pool.execute(move || {
              if let Err(e) = server_clone.handle_connection(stream) {
                trace!("ConnectionFailure {:?}", e);
              } else {
                trace!("ConnectionSuccess");
              }
            });
          }
          #[cfg(feature = "log")]
          Err(e) => {
            // TODO this will be removed eventually.
            // Having a connection filter that acts as a "firewall" here is not the best idea.
            // Once we feed the connections externally instead of doing the listening
            // here this becomes redundant anyways.
            trace!("ConnectionDenied {:?}", e);
          }
          #[cfg(not(feature = "log"))]
          Err(_) => {}
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

  /// This method creates the HttpServer from the builder.
  pub fn build(self) -> HumptyServer {
    HumptyServer::new(
      self.default_subapp,
      self.subapps,
      self.error_handler,
      self.not_found_handler,
      self.connection_timeout,
    )
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

  /// Sets the not found handler for the server.
  pub fn with_not_found_handler(mut self, handler: NotFoundHandler) -> Self {
    self.not_found_handler = handler;
    self
  }

  /// Sets the connection timeout, the amount of time to wait between keep-alive requests.
  pub fn with_connection_timeout(mut self, timeout: Option<Duration>) -> Self {
    self.connection_timeout = timeout;
    self
  }
}

/// The default error handler for every Humpty app.
/// This can be overridden by using the `with_error_handler` method when building the app.
pub(crate) fn default_error_handler(request: &Request, error: io::Error) -> io::Result<Response> {
  error!("Internal Server Error {} {} {:?}", request.method, request.path.as_str(), error);
  Ok(Response::empty(StatusCode::InternalServerError))
}

pub(crate) fn default_not_found_handler(request: &Request) -> io::Result<Response> {
  info!("Not found {} {}", request.method, request.path.as_str());
  Ok(Response::empty(StatusCode::NotFound))
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
