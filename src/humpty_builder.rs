//! Provides the core Humpty app functionality.

use crate::http::response::Response;
use crate::http::status::StatusCode;
use crate::route::HumptyRouter;
use crate::thread::pool::ThreadPool;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;
use std::{io, thread};

/// Represents the Humpty app.
pub struct HumptyBuilder {
  thread_pool: ThreadPool,
  routers: Vec<HumptyRouter>,
  error_handler: ErrorHandler,
  not_found_handler: NotFoundHandler,
  connection_timeout: Option<Duration>,
  shutdown: Option<Receiver<()>>,
}

pub use crate::handler_traits::*;
use crate::http::Request;
use crate::humpty_server::HumptyServer;
use crate::{error_log, info_log, trace_log};

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
      routers: Vec::new(),
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
      self.routers,
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
            trace_log!("ConnectionSuccess {:?}", stream.peer_addr());
            let server_clone = server.clone();
            // Spawn a new thread to handle the connection
            self.thread_pool.execute(move || {
              if let Err(e) = server_clone.handle_connection(stream) {
                trace_log!("ConnectionFailure {:?}", e);
              } else {
                trace_log!("ConnectionSuccess");
              }
            });
          }
          Err(e) => {
            // TODO this will be removed eventually.
            // Having a connection filter that acts as a "firewall" here is not the best idea.
            // Once we feed the connections externally instead of doing the listening
            // here this becomes redundant anyways.
            trace_log!("ConnectionDenied {:?}", e);
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

  /// This method creates the HttpServer from the builder.
  pub fn build(self) -> HumptyServer {
    HumptyServer::new(
      self.routers,
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
  pub fn with_router(mut self, handler: HumptyRouter) -> Self {
    self.routers.push(handler);
    self
  }


  /// Adds a new router to the server and calls the closure with the new router so it can be configured.
  pub fn router<T: FnOnce(HumptyRouter) -> HumptyRouter>(mut self, builder: T) -> Self {
    let def = HumptyRouter::default();
    self.routers.push(builder(def));
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
  error_log!("Internal Server Error {} {} {:?}", &request.method, request.path.as_str(), error);
  Ok(Response::empty(StatusCode::InternalServerError))
}

pub(crate) fn default_not_found_handler(request: &Request) -> io::Result<Response> {
  info_log!("Not found {} {}", &request.method, request.path.as_str());
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
