use humpty::http::{Request, Response, StatusCode};
use humpty::monitor::event::{Event, EventLevel};
use humpty::monitor::MonitorConfig;
use humpty::App;

use std::error::Error;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::spawn;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
  let (tx, rx): (Sender<Event>, Receiver<Event>) = channel();

  let app = App::new_with_config(4)
    .with_connection_timeout(Some(Duration::from_secs(5)))
    .with_monitor(MonitorConfig::new(tx).with_subscription_to(EventLevel::Debug))
    .with_route("/", home);

  spawn(move || {
    for event in rx {
      println!("{}", event);
    }
  });

  app.run("0.0.0.0:8080")?;

  Ok(())
}

fn home(_: Request) -> Response {
  Response::new(StatusCode::OK, "<html><body><h1>Home</h1></body></html>")
}
