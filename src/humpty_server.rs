//! Server impl takes care of reading the RequestHead, deciding what type of body there is and delegating processing to a router.
//! It also handles http keep alive and rudimentary (fallback) error handling.
//! If no router wants to handle the request it also has a 404 handler.

use crate::functional_traits::Router;
use crate::http::headers::HeaderName;
use crate::http::request::HttpVersion;
use crate::http::request_context::RequestContext;
use crate::http::{Response, StatusCode};
use crate::humpty_builder::{ErrorHandler, NotFoundHandler, RouterWebSocketServingResponse};
use crate::humpty_error::{HumptyError, HumptyResult};
use crate::stream::{ConnectionStream, IntoConnectionStream};
use crate::{error_log, trace_log};
use std::any::Any;
use std::fmt::{Debug, Formatter};
use std::io;
use std::io::ErrorKind;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Trait for metadata for streams. This could for example be an indicator of what type of stream this is
/// if this is relevant for your application. For example an app may ingest connections from a plain and tls socket at the same time.
/// This could be used to indicate this, or indicate things like "is there a client Certficate present and if so which?"
/// This type is opaque intentionally, and it is left entirely up to the user if such metadata is needed and present when
/// processing a stream.
pub trait ConnectionStreamMetadata: Any + Debug + Send + Sync {
  /// upcast to dyn Any. most likely just return "self".
  fn as_any(&self) -> &dyn Any;
}

#[derive(Debug)]
struct PhantomStreamMetadata;

impl ConnectionStreamMetadata for PhantomStreamMetadata {
  fn as_any(&self) -> &dyn Any {
    // This type is never instantiated. therefore this is unreachable.
    crate::util::unreachable()
  }
}

/// Struct that represents a built server capable of handling connections from some sources.
/// It does NOT own any OS resources like server sockets / file descriptors.
#[derive(Debug)]
pub struct HumptyServer {
  shutdown: AtomicBool,
  routers: Vec<Box<dyn Router>>,
  error_handler: ErrorHandler,
  not_found_handler: NotFoundHandler,
  max_head_buffer_size: usize,
  connection_timeout: Option<Duration>,
  read_timeout: Option<Duration>,
  keep_alive_timeout: Option<Duration>,
  request_body_io_timeout: Option<Duration>,
  write_timeout: Option<Duration>,
  shutdown_hooks: Hooks,
}

struct Hooks(Mutex<Vec<Box<dyn FnMut() + Send + Sync>>>);

impl Debug for Hooks {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    f.write_str("Hooks")
  }
}

impl Default for Hooks {
  fn default() -> Self {
    Self(Mutex::new(Vec::new()))
  }
}

impl HumptyServer {
  #[expect(clippy::too_many_arguments)] //Builder
  pub(crate) fn new(
    routers: Vec<Box<dyn Router>>,
    error_handler: ErrorHandler,
    not_found_handler: NotFoundHandler,
    max_head_buffer_size: usize,
    connection_timeout: Option<Duration>,
    read_timeout: Option<Duration>,
    keep_alive_timeout: Option<Duration>,
    request_body_io_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
  ) -> Self {
    HumptyServer {
      shutdown: AtomicBool::new(false),
      routers,
      error_handler,
      not_found_handler,
      max_head_buffer_size,
      read_timeout,
      connection_timeout: connection_timeout.or(read_timeout),
      keep_alive_timeout: keep_alive_timeout.or(read_timeout),
      request_body_io_timeout: request_body_io_timeout.or(read_timeout),
      write_timeout,
      shutdown_hooks: Hooks::default(),
    }
  }

  /// Handles a connection without any metadata
  pub fn handle_connection<S: IntoConnectionStream>(&self, stream: S) -> HumptyResult<()> {
    self.handle_connection_inner::<S, PhantomStreamMetadata>(stream, None)
  }

  /// Handles a connection with arbitrary metadata
  pub fn handle_connection_with_meta<S: IntoConnectionStream, M: ConnectionStreamMetadata>(
    &self,
    stream: S,
    meta: M,
  ) -> HumptyResult<()> {
    self.handle_connection_inner(stream, Some(meta))
  }

  /// Will mark this humpty server as shutdown.
  /// It will no longer accept new connections, send Connection: Close for all pending requests
  /// but not cancel any ongoing requests.
  ///
  /// This fn will also execute all shutdown hooks.
  ///
  /// Attention: If a Shutdown hook panics then remaining shutdown hooks are not executed.
  /// After a panic subsequent executions of shutdown will also NOT execute remaining hooks!
  ///
  pub fn shutdown(&self) {
    self.shutdown.store(true, SeqCst);
    if let Ok(mut guard) = self.shutdown_hooks.0.lock() {
      while let Some(mut hook) = guard.pop() {
        hook()
      }
    }
  }

  /// Returns true if this HumptyServer is marked for shutdown.
  pub fn is_shutdown(&self) -> bool {
    self.shutdown.load(SeqCst)
  }

  /// Adds the given shutdown hook to the HumptyServer.
  pub fn add_shutdown_hook<F: FnMut() + Sync + Send + 'static>(&self, mut hook: F) {
    let Ok(mut guard) = self.shutdown_hooks.0.lock() else {
      //Only way for poisoned mutex was if shutdown was already called and a hook panicked.
      hook();
      return;
    };

    if self.is_shutdown() {
      drop(guard); // Do not poison the mutex if "hook" blows up.
      hook();
      return;
    }

    guard.push(Box::new(hook));
  }

  /// Impl for handle connection.
  fn handle_connection_inner<S: IntoConnectionStream, M: ConnectionStreamMetadata>(
    &self,
    stream: S,
    meta: Option<M>,
  ) -> HumptyResult<()> {
    if self.shutdown.load(SeqCst) {
      return Err(HumptyError::from_io_kind(ErrorKind::ConnectionAborted));
    }

    let stream = stream.into_connection_stream();

    stream.set_read_timeout(self.connection_timeout)?;
    stream.set_write_timeout(self.write_timeout)?;
    if !stream.ensure_readable()? {
      return Err(HumptyError::from_io_kind(ErrorKind::UnexpectedEof));
    }

    let meta = meta.map(|a| Arc::new(a) as Arc<dyn ConnectionStreamMetadata>);

    let mut count = 0u64;

    loop {
      if count > 0 && !self.handle_keep_alive(stream.as_ref())? {
        break;
      }

      stream.set_read_timeout(self.read_timeout)?;

      let mut context = match RequestContext::new(
        stream.as_ref(),
        meta.as_ref().cloned(),
        self.max_head_buffer_size,
      ) {
        Ok(ctx) => ctx,
        Err(err) => return Err(err),
      };
      count += 1;

      stream.set_read_timeout(self.request_body_io_timeout)?;

      // If the request is valid an is a WebSocket request, call the corresponding handler
      if context.request_head().version() == HttpVersion::Http11
        && context.request_head().get_header(&HeaderName::Upgrade) == Some("websocket")
      {
        //Http 1.0 or 0.9 does not have web sockets

        trace_log!("WebsocketConnectionRequested");

        for router in self.routers.iter() {
          //Note, it's not a good idea to further handle errors form web socket router as
          //We have got no clue if we actually already switched protocols or not in error case.
          //Best bail asap
          match router.serve_websocket(stream.as_ref(), &mut context)? {
            RouterWebSocketServingResponse::HandledWithProtocolSwitch => return Ok(()),
            RouterWebSocketServingResponse::HandledWithoutProtocolSwitch(response) => {
              self.write_response(stream.as_ref(), context, false, response)?;
              return Ok(());
            }
            RouterWebSocketServingResponse::NotHandled => (), // Next router please
          }
        }

        //Respond with 404
        let response = match (self.not_found_handler)(&mut context) {
          Ok(res) => res,
          Err(error) => (self.error_handler)(&mut context, error)
            .unwrap_or_else(|e| self.fallback_error_handler(&mut context, e)),
        };

        self.write_response(stream.as_ref(), context, false, response)?;
        return Ok(());
      }

      // Will we do keep alive?
      let mut keep_alive = !self.is_shutdown()
          // is this http 1.1 because earlier does not support it.
          && context.request_head().version() == HttpVersion::Http11
          // Do we have a keep alive timeout that is not zero?
          && self.keep_alive_timeout.as_ref().map(|a| !a.is_zero()).unwrap_or(true)
          // did the client tell us not to do keep alive?
          && context
            .request_head()
            .get_header(&HeaderName::Connection)
            .map(|e| e.eq_ignore_ascii_case("keep-alive"))
            .unwrap_or_default();

      let mut response = None;
      for router in self.routers.iter() {
        response = Some(match router.serve(&mut context) {
          Ok(Some(resp)) => resp,
          Ok(None) => continue,
          Err(error) => (self.error_handler)(&mut context, error)
            .unwrap_or_else(|e| self.fallback_error_handler(&mut context, e)),
        });

        break;
      }

      let response = response.unwrap_or_else(|| match (self.not_found_handler)(&mut context) {
        Ok(res) => res,
        Err(error) => (self.error_handler)(&mut context, error)
          .unwrap_or_else(|e| self.fallback_error_handler(&mut context, e)),
      });

      keep_alive &= !context.is_connection_close_forced();

      self.write_response(stream.as_ref(), context, keep_alive, response)?;

      // Can we do keep alive?
      if !keep_alive {
        trace_log!("NoKeepAlive");
        break;
      }

      trace_log!("KeepAliveRespected");
    }

    trace_log!("ConnectionClosed");
    Ok(())
  }

  fn handle_keep_alive(&self, stream: &dyn ConnectionStream) -> HumptyResult<bool> {
    if self.is_shutdown() {
      trace_log!("Keep-alive server shutting down...");
      return Ok(false);
    }

    if stream.available() > 0 {
      trace_log!("Keep-alive client sent data. Processing next request...");
      return Ok(true);
    }
    stream.set_read_timeout(self.keep_alive_timeout)?;
    match stream.ensure_readable() {
      Ok(true) => {
        trace_log!("Keep-alive client sent data. Processing next request...");
        Ok(true)
      }
      Ok(false) => {
        trace_log!("Keep-alive client disconnected before timeout expired.");
        Ok(false)
      }
      Err(err) => match err.kind() {
        ErrorKind::UnexpectedEof => {
          trace_log!("Keep-alive client disconnected before timeout expired.");
          Ok(false)
        }
        ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe => {
          trace_log!("Keep-alive OS reset connection before timeout expired.");
          Ok(false)
        }
        ErrorKind::TimedOut | ErrorKind::WouldBlock => {
          trace_log!("Keep-alive time out closing connection.");
          Ok(false)
        }
        _ => {
          error_log!("Keep-alive unspecified error when waiting for data {}", &err);
          Err(err.into())
        }
      },
    }
  }

  fn write_response(
    &self,
    stream: &dyn ConnectionStream,
    context: RequestContext,
    keep_alive: bool,
    mut response: Response,
  ) -> HumptyResult<()> {
    if context.request_head().version() == HttpVersion::Http11 {
      let previous_headers = if keep_alive {
        response.headers.replace_all(HeaderName::Connection, "Keep-Alive")
      } else {
        response.headers.replace_all(HeaderName::Connection, "Close")
      };

      if !previous_headers.is_empty() {
        trace_log!("Endpoint has set banned header 'Connection' {:?}", previous_headers);
        return Err(HumptyError::new_io(
          io::ErrorKind::InvalidInput,
          "Endpoint has set banned header 'Connection'",
        ));
      }
    }

    trace_log!("RequestRespondedWith HTTP {}", response.status_code.code());

    response.write_to(context.request_head().version(), stream.as_stream_write()).inspect_err(
      |e| {
        trace_log!("response.write_to {}", e);
      },
    )?;

    trace_log!("RequestServedSuccess");

    context.consume_request_body()?;
    Ok(())
  }

  fn fallback_error_handler(&self, request: &mut RequestContext, error: HumptyError) -> Response {
    request.force_connection_close();

    error_log!(
      "Error handler failed. Will respond with empty Internal Server Error {} {} {:?}",
      &request.request_head().method(),
      request.request_head().path(),
      error
    );

    Response::new(StatusCode::InternalServerError)
  }
}

impl Drop for HumptyServer {
  fn drop(&mut self) {
    self.shutdown();
    trace_log!("HumptyServer::drop");
  }
}
