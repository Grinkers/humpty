use std::error::Error;
use std::thread::{self, spawn};
use std::time::Duration;

use tii::extras::{TiiConnector, TiiTcpConnector, TiiWsbAppBuilder, TiiWsbHandle};
use tii::TiiBuilder;
use tii::TiiWebsocketMessage;

fn main() -> Result<(), Box<dyn Error>> {
  let websocket_linker = TiiWsbAppBuilder::default()
    .with_message_handler(message_handler)
    .with_connect_handler(connect_handler)
    .with_disconnect_handler(disconnect_handler);

  let tii_server = TiiBuilder::builder_arc(|builder| {
    builder
      .router(|router| router.ws_route_any("/ws", websocket_linker.endpoint()))?
      .with_connection_timeout(Some(Duration::from_secs(8)))
  })
  .unwrap();

  // This can be shared around any threads to broadcast
  let _sender = websocket_linker.sender();

  let websocket_thread = spawn(|| {
    websocket_linker.finalize().run().unwrap();
  });

  let app = TiiTcpConnector::start_unpooled("0.0.0.0:8080", tii_server).unwrap();

  // Send shutdown signal after 420 seconds. Override with command line args for valgrind.
  let dur: u64 = {
    let args: Vec<String> = std::env::args().collect();
    let dur = if let Some(n) = args.get(1) { n.to_owned() } else { "420".to_string() };
    let dur = dur.parse().unwrap();
    println!("shutting down in {dur:?} sec");
    dur
  };
  thread::sleep(Duration::from_secs(dur));
  app.shutdown_and_join(None);

  // Dropping the TcpConnector will automatically halt the websocket app.
  // This explicit drop and join is optional. In this case, it gets sanity checked with Valgrind
  drop(app);
  websocket_thread.join().unwrap();
  Ok(())
}

// Processes the input from the ws client and does a fizzbuzz-like return.
// Each WS client has their own fizzbuzz state, but when any client triggers a "fizzbuzz",
// it ends up being broadcast to all ws clients.
fn message_handler(handle: TiiWsbHandle, message: TiiWebsocketMessage) {
  let ret = fizzbuzz(message.text().unwrap());
  if ret == "fizzbuzz" {
    println!("doing fizzbuzz broadcast");
    handle.broadcast(TiiWebsocketMessage::new_text(&ret));
  } else {
    handle.send(TiiWebsocketMessage::new_text(&ret));
  }
}

fn connect_handler(handle: TiiWsbHandle) {
  println!("{}: Client connected!", handle.peer_addr());
}

fn disconnect_handler(handle: TiiWsbHandle) {
  let msg = format!("{}: Client disconnected", handle.peer_addr());
  println!("{msg}");
  handle.broadcast(TiiWebsocketMessage::new_text(&msg));
}

fn fizzbuzz(s: &str) -> &str {
  let mut s = s.trim_end();

  if let Ok(i) = s.parse::<i64>() {
    s = match (i % 3 == 0, i % 5 == 0) {
      (true, true) => "fizzbuzz",
      (true, _) => "fizz",
      (_, true) => "buzz",
      _ => s,
    };
  }

  s
}
