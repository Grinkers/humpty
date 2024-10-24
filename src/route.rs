//! Provides functionality for handling app routes.

use crate::humpty_builder::{default_error_handler, default_not_found_handler, ErrorHandler, NotFoundHandler, PathAwareRequestHandler, RequestHandler, WebsocketHandler};
use crate::krauss;
use std::io;
use crate::handler_traits::{RequestFilter, ResponseFilter, Router, RouterFilter};
use crate::http::{Request, Response};

/// Represents a sub-app to run for a specific host.
pub struct HumptyRouter {
  /// This filter/predicate will decide if the router should even serve the request at all
  pub router_filter: Box<dyn RouterFilter>,

  /// Filters that run before the route is matched.
  /// These filters may modify the path of the request to affect routing decision.
  pub pre_routing_filters: Vec<Box<dyn RequestFilter>>,
  /// Filters that run once the routing decision has been made.
  /// These filters only run if there is an actual endpoint.
  pub routing_filters: Vec<Box<dyn RequestFilter>>,

  /// These filters run on the response after the actual endpoint (or the error handler) has been called.
  pub response_filters: Vec<Box<dyn ResponseFilter>>,

  /// The routes to process requests for and their handlers.
  pub routes: Vec<RouteHandler>,

  /// The routes to process WebSocket requests for and their handlers.
  pub websocket_routes: Vec<WebsocketRouteHandler>,

  /// Called when no route has been found in the router.
  pub not_found_handler: NotFoundHandler,
  /// Called when an error in any of the above occurs.
  pub error_handler: ErrorHandler,
}

/// Encapsulates a route and its handler.
pub struct RouteHandler {
  /// The route that this handler will match.
  pub route: String,
  /// The handler to run when the route is matched.
  pub handler: Box<dyn RequestHandler>,
}

/// Encapsulates a route and its WebSocket handler.
pub struct WebsocketRouteHandler {
  /// The route that this handler will match.
  pub route: String,
  /// The handler to run when the route is matched.
  pub handler: Box<dyn WebsocketHandler>,
}

impl Default for HumptyRouter {
  fn default() -> Self {
    HumptyRouter { router_filter: Box::new(default_pre_routing_filter),
      pre_routing_filters: Vec::default(),
      routing_filters: Vec::default(),
      response_filters: Vec::default(),
      routes: Vec::new(),
      websocket_routes: Vec::new(),
      not_found_handler: default_not_found_handler,
      error_handler: default_error_handler
    }
  }
}

impl HumptyRouter {
  /// Create a new sub-app with no routes.
  pub fn new() -> Self {
    HumptyRouter::default()
  }

  /// Adds a route and associated handler to the sub-app.
  /// Routes can include wildcards, for example `/blog/*`.
  pub fn with_pre_routing_filter<T>(mut self, filter: T) -> Self
  where
      T: RouterFilter + 'static,
  {
    self.router_filter = Box::new(filter);
    self
  }


  /// Adds a route and associated handler to the sub-app.
  /// Routes can include wildcards, for example `/blog/*`.
  pub fn with_route<T>(mut self, route: &str, handler: T) -> Self
  where
    T: RequestHandler + 'static,
  {
    self.routes.push(RouteHandler { route: route.to_string(), handler: Box::new(handler) });
    self
  }

  /// Adds a path-aware route and associated handler to the sub-app.
  /// Routes can include wildcards, for example `/blog/*`.
  /// Will also pass the route to the handler at runtime.
  pub fn with_path_aware_route<T>(mut self, route: &'static str, handler: T) -> Self
  where
    T: PathAwareRequestHandler + 'static,
  {
    self.routes.push(RouteHandler {
      route: route.to_string(),
      handler: Box::new(move |request| handler.serve(request, route)),
    });
    self
  }

  /// Adds a WebSocket route and associated handler to the sub-app.
  /// Routes can include wildcards, for example `/ws/*`.
  /// The handler is passed the stream and the request which triggered its calling.
  pub fn with_websocket_route<T>(mut self, route: &str, handler: T) -> Self
  where
    T: WebsocketHandler + 'static,
  {
    self
      .websocket_routes
      .push(WebsocketRouteHandler { route: route.to_string(), handler: Box::new(handler) });
    self
  }

  fn serve_outer(&self, request: &Request) -> io::Result<Option<Response>> {
    if !self.router_filter.filter(request)? {
      return Ok(None);
    }

    let mut resp = self.serve_inner(request).or_else(|e| (self.error_handler)(request, e))?;
    for filter in self.response_filters.iter() {
      resp = filter.filter(request, resp).or_else(|e| (self.error_handler)(request, e))?;
    }

    Ok(Some(resp))
  }
  fn serve_inner(&self, request: &Request) -> io::Result<Response> {
    for filter in self.pre_routing_filters.iter() {
      if let Some(resp) = filter.filter(request)? {
        return Ok(resp);
      }
    }

    if let Some(handler) = self
        .routes // Get the routes of the sub-app
        .iter() // Iterate over the routes
        .find(|route| route.route.route_matches(&request.path)) {
      for filter in self.routing_filters.iter() {
        if let Some(resp) = filter.filter(request)? {
          return Ok(resp);
        }
      }

      return Ok(handler.handler.serve(request.clone())?); //TODO get rid of this clone
    }

    (self.not_found_handler)(request)
  }
}

impl Router for HumptyRouter {
  fn serve(&self, request: &Request) -> io::Result<Option<Response>> {
    self.serve_outer(request)
  }
}

/// An object that can represent a route, currently only `String`.
pub trait Route {
  /// Returns true if the given route matches the path.
  fn route_matches(&self, route: &str) -> bool;
}

impl Route for String {
  /// Checks whether this route matches the given one, respecting its own wildcards only.
  /// For example, `/blog/*` will match `/blog/my-first-post` but not the other way around.
  fn route_matches(&self, route: &str) -> bool {
    krauss::wildcard_match(self, route)
  }
}

fn default_pre_routing_filter(_request: &Request) -> io::Result<bool> {
  Ok(true)
}
