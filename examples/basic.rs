use humpty::http::request_body::RequestBody;
use humpty::http::{Request, Response, StatusCode};
use humpty::HumptyBuilder;
use std::error::Error;
use std::io;
use std::io::Read;

fn main() -> Result<(), Box<dyn Error>> {
  let app = HumptyBuilder::default()
    .with_route("/", home)
    .with_route("/contact", contact)
    .with_route("/ping", pong)
    .with_route("/*", generic);

  app.run("0.0.0.0:8080")?;

  Ok(())
}

fn home(_: Request) -> io::Result<Response> {
  Ok(Response::new(StatusCode::OK, "<html><body><h1>Home</h1></body></html>"))
}

fn contact(_: Request) -> io::Result<Response> {
  Ok(Response::new(StatusCode::OK, "<html><body><h1>Contact</h1></body></html>"))
}

fn generic(request: Request) -> io::Result<Response> {
  let html = format!("<html><body><h1>You just requested {}.</h1></body></html>", request.path);

  Ok(Response::new(StatusCode::OK, html))
}

fn pong(request: Request) -> io::Result<Response> {
  let mut body = request.body.unwrap_or_else(|| RequestBody::new_with_data_ref(b"No Body"));
  let mut v = Vec::new();
  body.read_to_end(&mut v)?;
  Ok(Response::new(StatusCode::OK, v))
}
