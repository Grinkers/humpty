//! Static content server example.
//!
//! ## Important
//! This example must be run from the `static-content` directory to successfully find the paths.
//! This is because content is found relative to the CWD instead of the binary.

use humpty::handlers;
use humpty::HumptyBuilder;

use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
  let app = HumptyBuilder::default()
    // Serve the "/" route with the specified file
    .with_route("/", handlers::serve_file("./examples/static/pages/index.html"))
    // Serve the "/img/*" route with files stored in the "./static/images" directory.
    .with_path_aware_route("/img/*", handlers::serve_dir("./examples/static/images"))
    // Serve a regular file path in the current directory.
    // This means simply appending the request URI to the directory path and looking for a file there.
    .with_route("/examples/*", handlers::serve_as_file_path("."))
    // Redirect requests to "/ferris" to "/img/ferris.png"
    .with_route("/ferris", handlers::redirect("/img/ferris.png"));

  app.run("0.0.0.0:8080")?;

  Ok(())
}
