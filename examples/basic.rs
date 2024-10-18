use humpty::http::request_body::RequestBody;
use humpty::http::{Request, Response, StatusCode};
use humpty::App;
use std::error::Error;
use std::io::Read;

fn main() -> Result<(), Box<dyn Error>> {
  let app = App::default()
    .with_route("/", home)
    .with_route("/contact", contact)
    .with_route("/ping", pong)
    .with_route("/*", generic);

  app.run("0.0.0.0:8080")?;

  Ok(())
}

fn home(_: Request) -> Response {
  Response::new(StatusCode::OK, "<html><body><h1>Home</h1></body></html>")
}

fn contact(_: Request) -> Response {
  Response::new(StatusCode::OK, "<html><body><h1>Contact</h1></body></html>")
}

fn generic(request: Request) -> Response {
  let html = format!("<html><body><h1>You just requested {}.</h1></body></html>", request.uri);

  Response::new(StatusCode::OK, html)
}

fn pong(request: Request) -> Response {
  let mut body = request.content.unwrap_or_else(|| RequestBody::new_with_data_ref(b"No Body"));
  let mut v = Vec::new();
  body.read_to_end(&mut v).unwrap();
  Response::new(StatusCode::OK, v)
}
