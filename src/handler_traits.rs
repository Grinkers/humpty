//! Defines traits for handler functions.

use crate::http::{Request, Response};
use crate::stream::ConnectionStream;

/// Represents a function able to handle a WebSocket handshake and consequent data frames.
pub trait WebsocketHandler: Send + Sync {
  fn serve(&self, request: Request, stream: Box<dyn ConnectionStream>);
}
impl<F> WebsocketHandler for F
where
  F: Fn(Request, Box<dyn ConnectionStream>) + Send + Sync,
{
  fn serve(&self, request: Request, stream: Box<dyn ConnectionStream>) {
    self(request, stream)
  }
}

/// Represents a function able to handle a request.
/// It is passed the request and must return a response.
///
/// ## Example
/// The most basic request handler would be as follows:
/// ```
/// fn handler(_: humpty::http::Request) -> humpty::http::Response {
///     humpty::http::Response::new(humpty::http::StatusCode::OK, b"Success")
/// }
/// ```
pub trait RequestHandler: Send + Sync {
  fn serve(&self, request: Request) -> Response;
}
impl<F> RequestHandler for F
where
  F: Fn(Request) -> Response + Send + Sync,
{
  fn serve(&self, request: Request) -> Response {
    self(request)
  }
}

/// Represents a function able to handle a request with respect to the route it was called from.
/// It is passed the request and the route it was called from, and must return a response.
///
/// ## Example
/// The most basic path-aware request handler would be as follows:
/// ```
/// fn handler(_: humpty::http::Request, route: &str) -> humpty::http::Response {
///     humpty::http::Response::new(humpty::http::StatusCode::OK, format!("Success matching route {}", route))
/// }
/// ```
pub trait PathAwareRequestHandler: Send + Sync {
  fn serve(&self, request: Request, route: &'static str) -> Response;
}
impl<F> PathAwareRequestHandler for F
where
  F: Fn(Request, &'static str) -> Response + Send + Sync,
{
  fn serve(&self, request: Request, route: &'static str) -> Response {
    self(request, route)
  }
}
