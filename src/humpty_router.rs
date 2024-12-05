//! Contains the impl of the router.

use crate::functional_traits::{
  HttpEndpoint, RequestFilter, ResponseFilter, Router, RouterFilter,
  RouterWebSocketServingResponse, WebsocketEndpoint,
};
use crate::http::headers::HeaderName;
use crate::http::method::Method;
use crate::http::mime::{AcceptMimeType, QValue};
use crate::http::request::HttpVersion;
use crate::http::request_context::RequestContext;
use crate::http::{Response, StatusCode};
use crate::humpty_builder::{ErrorHandler, NotRouteableHandler};
use crate::humpty_error::{HumptyError, HumptyResult, InvalidPathError, RequestHeadParsingError};
use crate::stream::ConnectionStream;
use crate::util::unwrap_some;
use crate::{trace_log, util};
use base64::Engine;
use regex::{Error, Regex};
use sha1::{Digest, Sha1};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;

#[derive(Debug, Clone)]
enum PathPart {
  Literal(String),
  Variable(String),
  Wildcard,
  RegexVariable(String, Regex),
  RegexTailVariable(String, Regex),
}

impl PathPart {
  fn parse(path: impl AsRef<str>) -> HumptyResult<Vec<PathPart>> {
    let mut path = path.as_ref();
    let full_path = path;
    if path == "/" || path.is_empty() {
      return Ok(Vec::new());
    }

    if path.starts_with("/") {
      path = &path[1..];
    }

    let mut parts = Vec::new();
    loop {
      if path.is_empty() || path == "/" {
        return Ok(parts);
      }

      let part = if let Some(idx) = path.find("/") {
        let part = &path[0..idx];
        path = &path[idx + 1..];
        part
      } else {
        let part = path;
        path = "";
        part
      };

      if part == "*" {
        parts.push(PathPart::Wildcard);
        if !path.is_empty() && path != "/" {
          return Err(InvalidPathError::MorePartsAfterWildcard(full_path.to_string()).into());
        }
        return Ok(parts);
      }

      if part.starts_with("{") && part.ends_with("}") {
        let variable = &part[1..part.len() - 1];
        if let Some((name, regex)) = variable.split_once(":") {
          let reg = Regex::new(regex).map_err(|e| match e {
            Error::Syntax(syntax) => {
              InvalidPathError::RegexSyntaxError(full_path.to_string(), regex.to_string(), syntax)
            }
            Error::CompiledTooBig(limit) => {
              InvalidPathError::RegexTooBig(full_path.to_string(), regex.to_string(), limit)
            }
            _ => InvalidPathError::RegexSyntaxError(
              full_path.to_string(),
              regex.to_string(),
              e.to_string(),
            ),
          })?;
          if !path.is_empty() && path != "/" {
            parts.push(PathPart::RegexVariable(name.to_string(), reg));
            continue;
          }

          parts.push(PathPart::RegexTailVariable(name.to_string(), reg));
          continue;
        }

        parts.push(PathPart::Variable(variable.to_string()));
        continue;
      }

      parts.push(PathPart::Literal(part.to_string()));
    }
  }

  const fn is_tail(&self) -> bool {
    matches!(self, PathPart::Wildcard | PathPart::RegexTailVariable(_, _))
  }
  fn matches(
    &self,
    part: &str,
    remaining: &str,
    variables: &mut Option<HashMap<String, String>>,
  ) -> bool {
    match self {
      PathPart::Literal(literal) => part == literal.as_str(),
      PathPart::Variable(var_name) => {
        if variables.is_none() {
          variables.replace(HashMap::new());
        }

        unwrap_some(variables.as_mut()).insert(var_name.to_string(), part.to_string());
        true
      }
      PathPart::Wildcard => true,
      PathPart::RegexVariable(var_name, regex) => {
        if regex.is_match(part) {
          if variables.is_none() {
            variables.replace(HashMap::new());
          }

          unwrap_some(variables.as_mut()).insert(var_name.to_string(), part.to_string());
          return true;
        }
        false
      }
      PathPart::RegexTailVariable(var_name, regex) => {
        if regex.is_match(remaining) {
          if variables.is_none() {
            variables.replace(HashMap::new());
          }

          unwrap_some(variables.as_mut()).insert(var_name.to_string(), remaining.to_string());
          return true;
        }
        false
      }
    }
  }
}

#[derive(Debug, Clone)]
/// Encapsulates a route and its handler.
pub struct Routeable {
  /// The route that this handler will match.
  path: String,

  parts: Vec<PathPart>,

  /// The method this route will handle
  method: Method,

  /// The mime types this route can consume
  /// EMPTY SET means this route does not expect a request body.
  consumes: HashSet<AcceptMimeType>,

  /// The mime types this route can produce
  /// EMPTY SET means this route will produce a matching body type.
  produces: HashSet<AcceptMimeType>,
}

pub(crate) struct HttpRoute {
  pub(crate) routeable: Routeable,

  /// The handler to run when the route is matched.
  pub(crate) handler: Box<dyn HttpEndpoint>,
}

pub(crate) struct WebSocketRoute {
  pub(crate) routeable: Routeable,

  /// The handler to run when the route is matched.
  pub(crate) handler: Box<dyn WebsocketEndpoint>,
}

impl HttpRoute {
  pub(crate) fn new(
    path: impl ToString,
    method: impl Into<Method>,
    consumes: HashSet<AcceptMimeType>,
    produces: HashSet<AcceptMimeType>,
    route: impl HttpEndpoint + 'static,
  ) -> HumptyResult<Self> {
    Ok(HttpRoute {
      routeable: Routeable::new(path, method, consumes, produces)?,
      handler: Box::new(route) as Box<dyn HttpEndpoint>,
    })
  }
}

impl WebSocketRoute {
  pub(crate) fn new(
    path: impl ToString,
    method: impl Into<Method>,
    consumes: HashSet<AcceptMimeType>,
    produces: HashSet<AcceptMimeType>,
    route: impl WebsocketEndpoint + 'static,
  ) -> HumptyResult<Self> {
    Ok(WebSocketRoute {
      routeable: Routeable::new(path, method, consumes, produces)?,
      handler: Box::new(route) as Box<dyn WebsocketEndpoint>,
    })
  }
}

/// Enum that shows information on how a particular request could be routed on a route.
#[derive(Clone, Eq, PartialEq, Debug)]
pub enum RoutingDecision {
  /// Routing matches with the given quality and path params.
  Match(QValue, Option<HashMap<String, String>>),
  /// Path doesnt match.
  PathMismatch,
  /// Path matches, but method doesn't.
  MethodMismatch,
  /// Path and method do match, but the request body cannot be processed by the route.
  MimeMismatch,
  /// Path and method do match, the body can be processed but the response of the endpoint will not be processable by the client.
  AcceptMismatch,
}

impl Display for RoutingDecision {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    //TODO make this not shit
    Debug::fmt(self, f)
  }
}

impl PartialOrd for RoutingDecision {
  fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
    Some(self.cmp(other))
  }
}

impl Ord for RoutingDecision {
  fn cmp(&self, other: &Self) -> Ordering {
    match (self, other) {
      (RoutingDecision::Match(q1, _), RoutingDecision::Match(q2, _)) => q1.cmp(q2),
      (RoutingDecision::Match(_, _), RoutingDecision::PathMismatch) => Ordering::Greater,
      (RoutingDecision::Match(_, _), RoutingDecision::MethodMismatch) => Ordering::Greater,
      (RoutingDecision::Match(_, _), RoutingDecision::MimeMismatch) => Ordering::Greater,
      (RoutingDecision::Match(_, _), RoutingDecision::AcceptMismatch) => Ordering::Greater,

      (RoutingDecision::PathMismatch, RoutingDecision::Match(_, _)) => Ordering::Less,
      (RoutingDecision::PathMismatch, RoutingDecision::PathMismatch) => Ordering::Equal,
      (RoutingDecision::PathMismatch, RoutingDecision::MethodMismatch) => Ordering::Less,
      (RoutingDecision::PathMismatch, RoutingDecision::MimeMismatch) => Ordering::Less,
      (RoutingDecision::PathMismatch, RoutingDecision::AcceptMismatch) => Ordering::Less,

      (RoutingDecision::MethodMismatch, RoutingDecision::Match(_, _)) => Ordering::Less,
      (RoutingDecision::MethodMismatch, RoutingDecision::PathMismatch) => Ordering::Greater,
      (RoutingDecision::MethodMismatch, RoutingDecision::MethodMismatch) => Ordering::Equal,
      (RoutingDecision::MethodMismatch, RoutingDecision::MimeMismatch) => Ordering::Less,
      (RoutingDecision::MethodMismatch, RoutingDecision::AcceptMismatch) => Ordering::Less,

      (RoutingDecision::MimeMismatch, RoutingDecision::Match(_, _)) => Ordering::Less,
      (RoutingDecision::MimeMismatch, RoutingDecision::PathMismatch) => Ordering::Greater,
      (RoutingDecision::MimeMismatch, RoutingDecision::MethodMismatch) => Ordering::Greater,
      (RoutingDecision::MimeMismatch, RoutingDecision::MimeMismatch) => Ordering::Equal,
      (RoutingDecision::MimeMismatch, RoutingDecision::AcceptMismatch) => Ordering::Less,

      (RoutingDecision::AcceptMismatch, RoutingDecision::Match(_, _)) => Ordering::Less,
      (RoutingDecision::AcceptMismatch, RoutingDecision::PathMismatch) => Ordering::Greater,
      (RoutingDecision::AcceptMismatch, RoutingDecision::MethodMismatch) => Ordering::Greater,
      (RoutingDecision::AcceptMismatch, RoutingDecision::MimeMismatch) => Ordering::Greater,
      (RoutingDecision::AcceptMismatch, RoutingDecision::AcceptMismatch) => Ordering::Equal,
    }
  }
}

impl Routeable {
  pub(crate) fn new(
    path: impl ToString,
    method: impl Into<Method>,
    consumes: HashSet<AcceptMimeType>,
    produces: HashSet<AcceptMimeType>,
  ) -> HumptyResult<Routeable> {
    let path = path.to_string();
    Ok(Routeable {
      parts: PathPart::parse(path.as_str())?,
      path,
      method: method.into(),
      consumes,
      produces,
    })
  }

  /// The path for this route
  pub fn path(&self) -> &str {
    self.path.as_str()
  }

  /// The method for this route
  pub fn method(&self) -> &Method {
    &self.method
  }

  /// The mime types this route can consume
  pub fn consumes(&self) -> &HashSet<AcceptMimeType> {
    &self.consumes
  }

  /// The mime types this route can produce
  pub fn produces(&self) -> &HashSet<AcceptMimeType> {
    &self.produces
  }

  fn matches_path(
    &self,
    route: &RequestContext,
    path_params: &mut Option<HashMap<String, String>>,
  ) -> bool {
    let mut request_path = route.request_head().path();
    if !request_path.starts_with("/") {
      return false;
    }

    request_path = &request_path[1..];

    if request_path.is_empty() && self.parts.is_empty() {
      return true;
    }

    let mut parts = self.parts.iter();
    loop {
      if let Some((path_part, remaining)) = request_path.split_once("/") {
        if let Some(part) = parts.next() {
          if !part.matches(path_part, request_path, path_params) {
            return false;
          }
          if part.is_tail() {
            return true;
          }

          request_path = remaining;
          continue;
        }

        return false;
      }

      if let Some(part) = parts.next() {
        if !part.matches(request_path, request_path, path_params) {
          return false;
        }

        if part.is_tail() {
          return true;
        }

        request_path = "";
        continue;
      }

      if request_path.is_empty() {
        return true;
      }

      return false;
    }
  }

  /// Checks whether this route matches the given one, respecting its own wildcards only.
  /// For example, `/blog/*` will match `/blog/my-first-post` but not the other way around.
  pub fn matches(&self, route: &RequestContext) -> RoutingDecision {
    let head = route.request_head();
    let mut path_params = None;

    if !self.matches_path(route, &mut path_params) {
      return RoutingDecision::PathMismatch;
    }

    if &self.method != head.method() {
      return RoutingDecision::MethodMismatch;
    }

    if let Some(content_type) = head.get_content_type() {
      let mut found = false;
      for mime in &self.consumes {
        if mime.permits_specific(content_type) {
          found = true;
          break;
        }
      }

      if !found {
        return RoutingDecision::MimeMismatch;
      }
    }

    if self.produces.is_empty() {
      //The endpoint either doesn't produce a body or declares that it will produce a matching body...
      return RoutingDecision::Match(QValue::MAX, path_params);
    }

    let acc = head.get_accept();
    if acc.is_empty() {
      //The client doesn't accept a body.
      return RoutingDecision::MimeMismatch;
    }

    let mut current_q = None;
    for accept in acc {
      for mime in &self.produces {
        if !accept.get_type().permits(mime) {
          continue;
        }

        let qvalue = accept.qvalue();
        if qvalue == QValue::MAX {
          return RoutingDecision::Match(qvalue, path_params);
        }

        match &current_q {
          None => {
            current_q = Some(accept.qvalue());
          }
          Some(current) => {
            if current < &qvalue {
              current_q = Some(accept.qvalue());
            }
          }
        }
      }
    }

    if let Some(qval) = current_q {
      return RoutingDecision::Match(qval, path_params);
    }

    RoutingDecision::AcceptMismatch
  }
}

impl Debug for HttpRoute {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    f.write_fmt(format_args!("HttpRoute({})", self.routeable.path.as_str()))
  }
}

impl Debug for WebSocketRoute {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    f.write_fmt(format_args!("HttpRoute({})", self.routeable.path.as_str()))
  }
}

/// Represents a sub-app to run for a specific host.
pub struct HumptyRouter {
  /// This filter/predicate will decide if the router should even serve the request at all
  router_filter: Box<dyn RouterFilter>,

  /// Filters that run before the route is matched.
  /// These filters may modify the path of the request to affect routing decision.
  pre_routing_filters: Vec<Box<dyn RequestFilter>>,

  /// Filters that run once the routing decision has been made.
  /// These filters only run if there is an actual endpoint.
  routing_filters: Vec<Box<dyn RequestFilter>>,

  /// These filters run on the response after the actual endpoint (or the error handler) has been called.
  response_filters: Vec<Box<dyn ResponseFilter>>,

  /// Contains all pathing information for websockets and normal http routes.
  /// This is essentially a union of routes and websocket_routes without the handler
  routeables: Vec<Routeable>,

  /// The routes to process requests for and their handlers.
  routes: Vec<HttpRoute>,

  /// The routes to process WebSocket requests for and their handlers.
  websocket_routes: Vec<WebSocketRoute>,

  /// Called when no route has been found in the router.
  not_found_handler: NotRouteableHandler,

  /// Called when no acceptable route has been found
  not_acceptable_handler: NotRouteableHandler,
  /// Called when no route with a handled method has been found.
  method_not_allowed_handler: NotRouteableHandler,
  /// Called when no route with a given media type has been found.
  unsupported_media_type_handler: NotRouteableHandler,

  /// Called when an error in any of the above occurs.
  error_handler: ErrorHandler,
}

impl Debug for HumptyRouter {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    f.write_fmt(format_args!("HumptyRouter(pre_routing_filters={}, routing_filters={}, response_filters={}, routes={:?}, websocket_routes={})",
                                 self.pre_routing_filters.len(),
            self.routing_filters.len(),
            self.response_filters.len(),
            self.routes,
            self.websocket_routes.len(),
        ))
  }
}

/// Performs the WebSocket handshake.
fn websocket_handshake(request: &RequestContext) -> HumptyResult<Response> {
  const HANDSHAKE_KEY_CONSTANT: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

  // Get the handshake key header
  let handshake_key = request
    .request_head()
    .get_header("Sec-WebSocket-Key")
    .ok_or(RequestHeadParsingError::MissingSecWebSocketKeyHeader)?;

  // Calculate the handshake response
  let sha1 =
    Sha1::new().chain_update(handshake_key).chain_update(HANDSHAKE_KEY_CONSTANT).finalize();
  let sec_websocket_accept = base64::prelude::BASE64_STANDARD.encode(sha1);

  //let sec_websocket_accept = sha1.encode();

  // Serialise the handshake response
  let response = Response::new(StatusCode::SwitchingProtocols)
    .with_header(HeaderName::Upgrade, "websocket")?
    .with_header(HeaderName::Connection, "Upgrade")?
    .with_header("Sec-WebSocket-Accept", sec_websocket_accept)?;

  // Oddly enough I think you can establish a WS connection with a POST request that has data.
  // This will consume that data if it has not already been used by a filter.
  // Some beta versions of Web Sockets used the request body to convey the Sec-WebSocket-Key...
  request.consume_request_body()?;
  Ok(response)
}

impl HumptyRouter {
  #[expect(clippy::too_many_arguments)] //Only called by the builder.
  pub(crate) fn new(
    router_filter: Box<dyn RouterFilter>,
    pre_routing_filters: Vec<Box<dyn RequestFilter>>,
    routing_filters: Vec<Box<dyn RequestFilter>>,
    response_filters: Vec<Box<dyn ResponseFilter>>,
    routes: Vec<HttpRoute>,
    websocket_routes: Vec<WebSocketRoute>,
    not_found_handler: NotRouteableHandler,
    not_acceptable_handler: NotRouteableHandler,
    method_not_allowed_handler: NotRouteableHandler,
    unsupported_media_type_handler: NotRouteableHandler,
    error_handler: ErrorHandler,
  ) -> Self {
    let mut routeables = Vec::new();
    for x in routes.iter() {
      routeables.push(x.routeable.clone());
    }
    for x in websocket_routes.iter() {
      routeables.push(x.routeable.clone());
    }

    Self {
      router_filter,
      pre_routing_filters,
      routing_filters,
      response_filters,
      routeables,
      routes,
      websocket_routes,
      not_found_handler,
      not_acceptable_handler,
      method_not_allowed_handler,
      unsupported_media_type_handler,
      error_handler,
    }
  }

  fn serve_ws(
    &self,
    stream: &dyn ConnectionStream,
    request: &mut RequestContext,
  ) -> HumptyResult<RouterWebSocketServingResponse> {
    //TODO this fn is too long and has significant duplicate parts with normal http serving.
    //TODO consolidate both impls and split it into smaller sub fn's

    if !self.router_filter.filter(request)? {
      return Ok(RouterWebSocketServingResponse::NotHandled);
    }

    for filter in self.pre_routing_filters.iter() {
      let resp = match filter.filter(request) {
        Ok(Some(res)) => res,
        Ok(None) => continue,
        Err(err) => (self.error_handler)(request, err)?,
      };

      let resp = self.call_response_filters(request, resp)?;
      return Ok(RouterWebSocketServingResponse::HandledWithoutProtocolSwitch(resp));
    }

    let mut best_decision = RoutingDecision::PathMismatch;
    let mut best_handler = None;

    for handler in &self.websocket_routes {
      let decision = handler.routeable.matches(request);
      if best_decision >= decision {
        continue;
      }

      best_decision = decision;
      if let RoutingDecision::Match(qv, _) = &best_decision {
        best_handler = Some(handler);
        if qv == &QValue::MAX {
          break;
        }
      }
    }

    if let Some(handler) = best_handler {
      request.set_routed_path(handler.routeable.path.as_str());
      self.handle_path_parameters(request, &best_decision);

      for filter in self.routing_filters.iter() {
        let resp = match filter.filter(request) {
          Ok(Some(res)) => res,
          Ok(None) => continue,
          Err(err) => (self.error_handler)(request, err)?,
        };

        let resp = self.call_response_filters(request, resp)?;
        return Ok(RouterWebSocketServingResponse::HandledWithoutProtocolSwitch(resp));
      }

      return match websocket_handshake(request) {
        Err(err) => {
          let resp = (self.error_handler)(request, err)?;
          let resp = self.call_response_filters(request, resp)?;
          Ok(RouterWebSocketServingResponse::HandledWithoutProtocolSwitch(resp))
        }
        Ok(resp) => {
          let resp = self.call_response_filters(request, resp)?;
          if resp.status_code != StatusCode::SwitchingProtocols {
            return Ok(RouterWebSocketServingResponse::HandledWithoutProtocolSwitch(resp));
          }

          resp.write_to(HttpVersion::Http11, stream)?; //Errors here are fatal

          let (sender, receiver) = crate::websocket::stream::new(stream);
          handler.handler.serve(request, receiver, sender)?;
          Ok(RouterWebSocketServingResponse::HandledWithProtocolSwitch)
        }
      };
    }

    trace_log!("WebsocketConnectionClosed Invoke fallback {}", &best_decision);

    let fallback = self.invoke_appropriate_fallback_handler(request, &best_decision);

    let fallback_resp = match fallback {
      Ok(resp) => self.call_response_filters(request, resp)?,
      Err(err) => {
        let resp = (self.error_handler)(request, err)?;
        self.call_response_filters(request, resp)?
      }
    };
    Ok(RouterWebSocketServingResponse::HandledWithoutProtocolSwitch(fallback_resp))
  }

  fn handle_path_parameters(&self, request: &mut RequestContext, best_decision: &RoutingDecision) {
    match best_decision {
      RoutingDecision::Match(_, path_params) => {
        if let Some(path_params) = path_params {
          for (key, value) in path_params {
            request.set_path_param(key, value);
          }
        }
      }
      _ => util::unreachable(),
    }
  }

  fn call_error_handler(
    &self,
    request: &mut RequestContext,
    error: HumptyError,
  ) -> HumptyResult<Response> {
    //TODO i am not 100% sure this is a good idea, but it probably is a good idea.
    //The only thing i could consider is having the default impl do this and outsource this responsibility to the user
    //Not doing this on io::Errors when reading the request body will cause stuff to break in a horrific manner.
    //So always doing this is safer, but prevents keepalive in cases where the error is unrelated to the http stream
    //and the user properly handles it.
    request.force_connection_close();

    (self.error_handler)(request, error)
  }

  fn serve_outer(&self, request: &mut RequestContext) -> HumptyResult<Option<Response>> {
    if !self.router_filter.filter(request)? {
      return Ok(None);
    }

    let mut resp = self.serve_inner(request).or_else(|e| self.call_error_handler(request, e))?;
    resp = self.call_response_filters(request, resp)?;

    Ok(Some(resp))
  }

  fn call_response_filters(
    &self,
    request: &mut RequestContext,
    mut resp: Response,
  ) -> HumptyResult<Response> {
    for filter in self.response_filters.iter() {
      resp = filter.filter(request, resp).or_else(|e| self.call_error_handler(request, e))?;
    }
    Ok(resp)
  }

  fn serve_inner(&self, request: &mut RequestContext) -> HumptyResult<Response> {
    for filter in self.pre_routing_filters.iter() {
      if let Some(resp) = filter.filter(request)? {
        return Ok(resp);
      }
    }

    let mut best_decision = RoutingDecision::PathMismatch;
    let mut best_handler = None;

    for handler in &self.routes {
      let decision = handler.routeable.matches(request);
      if best_decision >= decision {
        continue;
      }

      best_decision = decision;
      if let RoutingDecision::Match(qv, _) = &best_decision {
        best_handler = Some(handler);
        if qv == &QValue::MAX {
          break;
        }
      }
    }

    if let Some(handler) = best_handler {
      request.set_routed_path(handler.routeable.path.as_str());
      self.handle_path_parameters(request, &best_decision);

      for filter in self.routing_filters.iter() {
        if let Some(resp) = filter.filter(request)? {
          return Ok(resp);
        }
      }

      return handler.handler.serve(request);
    }

    self.invoke_appropriate_fallback_handler(request, &best_decision)
  }

  fn invoke_appropriate_fallback_handler(
    &self,
    request: &mut RequestContext,
    best_decision: &RoutingDecision,
  ) -> HumptyResult<Response> {
    match best_decision {
      RoutingDecision::PathMismatch => (self.not_found_handler)(request, &self.routeables),
      RoutingDecision::MethodMismatch => {
        (self.method_not_allowed_handler)(request, &self.routeables)
      }
      RoutingDecision::MimeMismatch => {
        (self.unsupported_media_type_handler)(request, &self.routeables)
      }
      RoutingDecision::AcceptMismatch => (self.not_acceptable_handler)(request, &self.routeables),
      // We found a handler! Why are we here?
      RoutingDecision::Match(_, _) => util::unreachable(),
    }
  }
}

impl Router for HumptyRouter {
  fn serve(&self, request: &mut RequestContext) -> HumptyResult<Option<Response>> {
    self.serve_outer(request)
  }

  fn serve_websocket(
    &self,
    stream: &dyn ConnectionStream,
    request: &mut RequestContext,
  ) -> HumptyResult<RouterWebSocketServingResponse> {
    self.serve_ws(stream, request)
  }
}

impl Router for Arc<HumptyRouter> {
  fn serve(&self, request: &mut RequestContext) -> HumptyResult<Option<Response>> {
    Arc::as_ref(self).serve(request)
  }

  fn serve_websocket(
    &self,
    stream: &dyn ConnectionStream,
    request: &mut RequestContext,
  ) -> HumptyResult<RouterWebSocketServingResponse> {
    Arc::as_ref(self).serve_websocket(stream, request)
  }
}
