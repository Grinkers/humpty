use humpty::extras::{Connector, TcpConnector};
use humpty::http::method::Method;
use humpty::http::mime::{AcceptMimeType, MimeType};
use humpty::http::request_context::RequestContext;
use humpty::http::Response;
use humpty::humpty_builder::HumptyBuilder;
use humpty::humpty_error::HumptyResult;
use log::info;

fn main() -> HumptyResult<()> {
  colog::default_builder().filter_level(log::LevelFilter::Trace).init();

  let humpty_server = HumptyBuilder::builder_arc(|builder| {
    //This example only has 1 router, you could have several by just calling .router(...) again.
    builder.router(|router| {
      router
        // All these different ways of adding routes do the same thing, pick the one most "optically" pleasing for you.
        //
        // Build endpoint "closure" style. Causes an indent making it easier to spot.
        .begin_get("/", |route| {
          route
            //
            .produces(MimeType::TextHtml)
            .produces(MimeType::TextPlain)
            .endpoint(home)
        })?
        //
        //build endpoint directly without any indents.
        .get("/contact")
        .produces(MimeType::TextHtml)
        .endpoint(contact)?
        // as you can see without this comment it would be hard to tell what belongs to which endpoint.
        .post("/ping")
        .consumes(AcceptMimeType::Wildcard)
        .produces(MimeType::ApplicationOctetStream)
        .endpoint(pong)?
        //If you do not desire any media type handling you can also use the "route" type of methods.
        // This endpoint is called for any normal http method and any media type.
        .route_any("/any/method", echo_method)?
        //Same but limited to http GET method
        .route_get("/only/get", echo_method)?
        // Humpty also supports non-standard custom methods.
        .route_method(Method::from("QUERY"), "/custom/stuff", echo_method)?
        // Begin is just a visual indent so you can group several other things together.
        // It does nothing else.
        .route_get("/path/param/{key1}/{key2}/{regex:.*}", path_param)?
        .begin(|router| {
          router
            //
            .get("/closure/*")
            //You don't have to pass a function pointer, if your endpoint is tiny you can also do it in a closure
            //You do have to explicitly write out "&RequestContext" tho otherwise rust gets confused.
            .endpoint(|ctx: &RequestContext| {
              Response::ok(
                format!("This is a closure to {}!", ctx.request_head().path()),
                MimeType::TextPlain,
              )
            })?
            .get("/*")
            .produces(MimeType::TextHtml)
            .endpoint(generic)
        })?
        // There 3 are not endpoints, they are filters etc.
        .with_pre_routing_request_filter(pre_routing)?
        .with_request_filter(routing)?
        .with_response_filter(resp)?
        .ok()
    })
  })?;

  let _ = TcpConnector::start_unpooled("0.0.0.0:8080", humpty_server)?.join(None);

  Ok(())
}

fn pre_routing(req: &mut RequestContext) -> HumptyResult<Option<Response>> {
  info!("pre_routing {:?}", req);
  Ok(None)
}

fn routing(req: &mut RequestContext) -> HumptyResult<Option<Response>> {
  info!("routing {:?}", req);
  Ok(None)
}

fn resp(req: &mut RequestContext, mut resp: Response) -> HumptyResult<Response> {
  info!("resp {:?}", req);
  resp.add_header("X-Magic", "true magic")?;
  Ok(resp)
}

fn home(_: &RequestContext) -> HumptyResult<Response> {
  Ok(Response::ok("<html><body><h1>Home</h1></body></html>", MimeType::TextHtml))
}

fn contact(_: &RequestContext) -> HumptyResult<Response> {
  Ok(Response::ok("<html><body><h1>Contact</h1></body></html>", MimeType::TextHtml))
}

fn generic(request: &RequestContext) -> HumptyResult<Response> {
  let html = format!(
    "<html><body><h1>You just requested {}.</h1></body></html>",
    request.request_head().path()
  );

  Ok(Response::ok(html, MimeType::TextHtml))
}

fn pong(request: &RequestContext) -> HumptyResult<Response> {
  let body = if let Some(body) = request.request_body() {
    let mut buffer = Vec::new();
    body.clone().read_to_end(&mut buffer)?;
    buffer
  } else {
    b"No Body".to_vec()
  };

  Ok(Response::ok(body, MimeType::ApplicationOctetStream))
}

fn echo_method(request: &RequestContext) -> Response {
  Response::ok(request.request_head().method().as_str(), MimeType::TextPlain)
}

fn path_param(request: &RequestContext) -> Response {
  for (key, value) in request.get_path_params() {
    info!("path_param {} {}", key, value);
  }

  Response::no_content()
}
