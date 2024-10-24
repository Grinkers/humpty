use crate::http::headers::HeaderType;
use crate::http::request::HttpVersion;
use crate::http::{Request, Response, StatusCode};
use crate::humpty_builder::{ErrorHandler, NotFoundHandler};
use crate::route::{Route};
use crate::stream::{ConnectionStream, IntoConnectionStream};
use crate::{error_log, trace_log, HumptyRouter};
use std::io;
use std::time::Duration;
use crate::handler_traits::Router;

pub struct HumptyServer {
  error_handler: ErrorHandler,
  not_found_handler: NotFoundHandler,
  timeout: Option<Duration>,
  routers: Vec<HumptyRouter>,
}
impl HumptyServer {
  pub(crate) fn new(
    sub_apps: Vec<HumptyRouter>,
    error_handler: ErrorHandler,
    not_found_handler: NotFoundHandler,
    timeout: Option<Duration>,
  ) -> Self {
    HumptyServer { error_handler, not_found_handler, timeout, routers: sub_apps }
  }

  pub fn handle_connection<T: IntoConnectionStream>(&self, stream: T) -> io::Result<()> {
    let stream = stream.into_connection_stream();

    let addr = stream.peer_addr()?;

    loop {
      // Parses the request from the stream
      let request = match self.timeout.as_ref() {
        Some(timeout) => Request::from_stream_with_timeout(stream.as_ref(), addr.clone(), *timeout),
        None => Request::from_stream(stream.as_ref(), addr.clone()),
      }?;

      // If the request is valid an is a WebSocket request, call the corresponding handler
      if request.version == HttpVersion::Http11
        && request.headers.get(&HeaderType::Upgrade) == Some("websocket")
      {
        //Http 1.0 or 0.9 does not have web sockets

        trace_log!("WebsocketConnectionRequested");

        if self.call_websocket_handler(&request, stream.as_ref())? {
          trace_log!("WebsocketConnectionClosed");
          return Ok(());
        }

        // TODO how can I tell a websocket request gracefully that there is no one here for it? HTTP 404?, this just shuts the socket.
        trace_log!("WebsocketConnectionClosed Not found");
        return Ok(());
      }

      // Is the keep alive header set?
      let keep_alive = request.version == HttpVersion::Http11
        && request
          .headers
          .get(&HeaderType::Connection)
          .map(|e| e.eq_ignore_ascii_case("keep-alive"))
          .unwrap_or_default();


      let mut response = None;
      for router in self.routers.iter() {
        response = Some(match router.serve(&request) {
          Ok(Some(resp)) => resp,
          Ok(None) => continue,
          Err(error) => (self.error_handler)(&request, error).unwrap_or_else(|e| self.fallback_error_handler(&request, e))
        });

        break;
      }

      let mut response = response.unwrap_or_else(|| {
        match (self.not_found_handler)(&request) {
          Ok(res) => res,
          Err(error) => (self.error_handler)(&request, error).unwrap_or_else(|e| self.fallback_error_handler(&request, e))
        }
      });

      request.ensure_consumed()?;

      if request.version == HttpVersion::Http11 {
        let previous_headers = if keep_alive {
          response.headers.replace_all(HeaderType::Connection, "Keep-Alive")
        } else {
          response.headers.replace_all(HeaderType::Connection, "Close")
        };

        if !previous_headers.is_empty() {
          trace_log!("Endpoint has set banned header 'Connection' {:?}", previous_headers);
          return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Endpoint has set banned header 'Connection'",
          ));
        }
      }

      trace_log!("RequestRespondedWith HTTP {}", response.status_code.code());

      response.write_to(request.version, stream.as_stream_write()).inspect_err(|e| {
        trace_log!("response.write_to {}", e);
      })?;

      trace_log!("RequestServedSuccess");

      // If the request specified to keep the connection open, respect this
      if !keep_alive {
        trace_log!("NoKeepAlive");
        break;
      }

      trace_log!("KeepAliveRespected");
    }

    trace_log!("ConnectionClosed");
    Ok(())
  }

  fn call_websocket_handler(&self, request: &Request, stream: &dyn ConnectionStream) -> io::Result<bool> {
    for router in &self.routers {
      if !router.router_filter.filter(request)? {
        continue;
      }

      if let Some(handler) = router
          .websocket_routes // Get the WebSocket routes of the sub-app
          .iter() // Iterate over the routes
          .find(|route| route.route.route_matches(&request.path)) {
        handler.handler.serve(request.clone(), stream.new_ref());
        return Ok(true);
      }
    }

    Ok(false)
  }

  fn fallback_error_handler(&self, request: &Request, error: io::Error) -> Response {
    error_log!(
      "Error handler failed. Will respond with empty Internal Server Error {} {} {:?}",
      &request.method,
      request.path.as_str(),
      error
    );
    Response::empty(StatusCode::InternalServerError)
  }
}
