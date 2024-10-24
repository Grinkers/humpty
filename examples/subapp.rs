use humpty::handlers::serve_file;
use humpty::{HumptyBuilder, HumptyRouter};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
  let localhost_subapp = HumptyRouter::new()
    .with_route("/different_response", serve_file("./static/localhost.html"))
    .with_route("/localhost_only", serve_file("./static/localhost.html"));

  let localip_subapp = HumptyRouter::new()
    .with_route("/different_response", serve_file("./static/localip.html"))
    .with_route("/localip_only", serve_file("./static/localip.html"));

  let app = HumptyBuilder::default()
    .with_route("/", serve_file("./static/index.html"))
    .with_router("localhost", localhost_subapp)
    .with_router("127.0.0.1", localip_subapp);

  app.run("0.0.0.0:8080")?;

  Ok(())
}
