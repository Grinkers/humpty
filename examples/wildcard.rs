use humpty::http::{Request, Response, StatusCode};
use humpty::HumptyBuilder;
use std::error::Error;
use std::io;

const HTML: &str = r##"
<html>

<head>
  <title>Humpty Wildcard Example</title>

  <script>
    function goToWildcard() {
      let text = document.querySelector("#text").value;
      window.location = `/wildcard/${text}`;
    }
  </script>
</head>

<body>
  <h1>Humpty Wildcard Example</h1>

  Type anything in the box below and press the button.
  <br><br>

  <input id="text" placeholder="Type something here">
  <button onclick="goToWildcard();">Go to wildcard page</button>
</body>

</html>"##;

fn main() -> Result<(), Box<dyn Error>> {
  let app: HumptyBuilder = HumptyBuilder::default().with_route("/", home).with_route("/wildcard/*", wildcard);

  app.run("127.0.0.1:8080")?;

  Ok(())
}

fn home(_: Request) -> io::Result<Response> {
  Ok(Response::new(StatusCode::OK, HTML))
}

fn wildcard(request: Request) -> io::Result<Response> {
  let wildcard_path = request
    .path // get the URI of the request
    .strip_prefix("/wildcard/") // remove the initial slash
    .unwrap(); // unwrap from the option

  let html = format!("<html><body><h1>Wildcard Path: {}</h1></body></html>", wildcard_path);

  Ok(Response::new(StatusCode::OK, html))
}
