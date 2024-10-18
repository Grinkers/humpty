use humpty::{handlers, App};

use humpty::websocket::error::WebsocketError;
use humpty::websocket::message::Message;
use humpty::websocket::stream::WebsocketStream;
use humpty::websocket::websocket_handler;

use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};

/// App state with a simple global atomic counter
static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn main() -> Result<(), Box<dyn Error>> {
  let app = App::default()
    // Serve the `static` directory to regular HTTP requests.
    .with_path_aware_route("/*", handlers::serve_dir("./examples/static/ws"))
    // Use the `humpty_ws` WebSocket handler to wrap our own echo handler.
    .with_websocket_route("/ws", websocket_handler(echo_handler));
  app.run("0.0.0.0:8080")?;

  Ok(())
}

/// Handler for WebSocket connections.
/// This is wrapped in `websocket_handler` to manage the handshake for us using the `humpty_ws` crate.
fn echo_handler(mut stream: WebsocketStream) {
  // Get the address of the client.
  let addr = stream.inner().peer_addr().unwrap();

  println!("New connection from {}", addr);

  // Loop while the client is connected.
  loop {
    // Block while waiting to receive a message.
    match stream.recv() {
      // If the message was received successfully, echo it back with an increasing number at the end.
      Ok(message) => {
        let message = message.text().unwrap();
        let count = COUNTER.fetch_add(1, Ordering::SeqCst);
        let response = format!("{} {}", message, count);

        // Send the WebSocket response
        stream.send(Message::new(response)).unwrap();

        println!("Received message `{}` from {}, echoing with the number {}", message, addr, count)
      }
      // If the connection was closed, break out of the loop and clean up
      Err(WebsocketError::ConnectionClosed) => {
        break;
      }
      // Ignore any other errors
      _ => (),
    }
  }

  println!("Connection closed by {}", addr);
}
