use crate::http::headers::HeaderType;
use crate::http::request::HttpVersion;
use crate::http::{Request, Response, StatusCode};
use crate::humpty_builder::{ErrorHandler, NotFoundHandler};
use crate::krauss::wildcard_match;
use crate::route::{Route, RouteHandler};
use crate::stream::{ConnectionStream, IntoConnectionStream};
use crate::{error_log, trace_log, SubApp};
use std::io;
use std::time::Duration;

pub struct HumptyServer {
  default_subapp: SubApp,
  error_handler: ErrorHandler,
  not_found_handler: NotFoundHandler,
  timeout: Option<Duration>,

  //TODO Questionable if we should keep this. This is only really useful if you develop something like nginx/apache2 aka a Reverse Proxy...  Just use nginx or apache2?
  sub_apps: Vec<SubApp>,
}
impl HumptyServer {
  pub(crate) fn new(
    default_subapp: SubApp,
    sub_apps: Vec<SubApp>,
    error_handler: ErrorHandler,
    not_found_handler: NotFoundHandler,
    timeout: Option<Duration>,
  ) -> Self {
    HumptyServer { default_subapp, error_handler, not_found_handler, timeout, sub_apps }
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

        self.call_websocket_handler(&request, stream.as_ref());

        trace_log!("WebsocketConnectionClosed");
        return Ok(());
      }

      // Is the keep alive header set?
      let keep_alive = request.version == HttpVersion::Http11
        && request
          .headers
          .get(&HeaderType::Connection)
          .map(|e| e.eq_ignore_ascii_case("keep-alive"))
          .unwrap_or_default();

      let mut response = match self.get_handler(&request) {
        Some(handler) => handler.handler.serve(request.clone()), //TODO get rid of this clone
        None => (self.not_found_handler)(&request),
      }
      .or_else(|e| (self.error_handler)(&request, e))
      .unwrap_or_else(|e| self.fallback_error_handler(&request, e));

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

  fn call_websocket_handler(&self, request: &Request, stream: &dyn ConnectionStream) {
    // Iterate over the sub-apps and find the one which matches the host
    if let Some(host) = request.headers.get(&HeaderType::Host) {
      if let Some(subapp) = self.sub_apps.iter().find(|subapp| wildcard_match(&subapp.host, host)) {
        // If the sub-app has a handler for this route, call it
        if let Some(handler) = subapp
          .websocket_routes // Get the WebSocket routes of the sub-app
          .iter() // Iterate over the routes
          .find(|route| route.route.route_matches(&request.path))
        {
          handler.handler.serve(request.clone(), stream.new_ref());
          return;
        }
      }
    }

    // If no sub-app was found, try to use the handler on the default sub-app
    if let Some(handler) = self
      .default_subapp
      .websocket_routes
      .iter()
      .find(|route| route.route.route_matches(&request.path))
    {
      handler.handler.serve(request.clone(), stream.new_ref())
    }
  }

  /// Gets the correct handler for the given request.
  fn get_handler(&self, request: &Request) -> Option<&RouteHandler> {
    // Iterate over the sub-apps and find the one which matches the host
    if let Some(host) = request.headers.get(&HeaderType::Host) {
      if let Some(subapp) = self.sub_apps.iter().find(|subapp| wildcard_match(&subapp.host, host)) {
        // If the sub-app has a handler for this route, call it
        if let Some(handler) = subapp
          .routes // Get the routes of the sub-app
          .iter() // Iterate over the routes
          .find(|route| route.route.route_matches(&request.path))
        // Find the route that matches
        {
          return Some(handler);
        }
      }
    }

    // If no sub-app was found, try to use the handler on the default sub-app
    if let Some(handler) =
      self.default_subapp.routes.iter().find(|route| route.route.route_matches(&request.path))
    {
      return Some(handler);
    }

    None
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
