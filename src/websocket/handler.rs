//! Provides a Humpty-compatible WebSocket handler for performing the handshake.

use crate::websocket::error::WebsocketError;
use crate::websocket::stream::WebsocketStream;
use crate::websocket::util::base64::Base64Encode;
use crate::websocket::util::sha1::SHA1Hash;
use crate::websocket::MAGIC_STRING;

use crate::http::headers::HeaderType;
use crate::http::{Request, Response, StatusCode};

use crate::stream::ConnectionStream;
use std::io::Write;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// Represents a function able to handle WebSocket streams.
pub trait WebsocketHandler: Fn(WebsocketStream) + Send + Sync {}
impl<T> WebsocketHandler for T where T: Fn(WebsocketStream) + Send + Sync {}

/// Provides WebSocket handshake functionality.
/// Supply a `WebsocketHandler` to handle the subsequent messages.
///
/// ## Example
/// ```no_run
/// use humpty::App;
/// use humpty::websocket::message::Message;
/// use humpty::websocket::stream::WebsocketStream;
/// use humpty::websocket::websocket_handler;
///
///
/// fn main() {
///     let app = App::default()
///         .with_websocket_route("/", websocket_handler(my_handler));
///
///     app.run("0.0.0.0:8080").unwrap();
/// }
///
/// fn my_handler(mut stream: WebsocketStream) {
///     stream.send(Message::new("Hello, World!")).unwrap();
/// }
/// ```
pub fn websocket_handler<T>(handler: T) -> impl Fn(Request, Box<dyn ConnectionStream>)
where
  T: WebsocketHandler,
{
  move |request: Request, mut stream: Box<dyn ConnectionStream>| {
    if handshake(request, &mut stream).is_ok() {
      handler(WebsocketStream::new(stream));
    }
  }
}

/// Provides asynchronous WebSocket functionality.
/// Supply a hook to an asynchronous WebSocket app to handle the subsequent messages.
///
/// It is important to note that, unless you need to modify the underlying Humpty application, it is
///   easier to simply create a regular app with `AsyncWebsocketApp::new()` which manages the Humpty
///   application internally.
///
/// ## Example
/// ```no_run
/// use humpty::App;
/// use humpty::websocket::async_app::{AsyncStream, AsyncWebsocketApp};
/// use humpty::websocket::handler::async_websocket_handler;
/// use humpty::websocket::message::Message;
///
/// use std::thread::spawn;
///
/// fn main() {
///     let websocket_app = AsyncWebsocketApp::new_unlinked().with_message_handler(message_handler);
///
///     let humpty_app = App::default()
///         .with_websocket_route("/ws", async_websocket_handler(websocket_app.connect_hook().unwrap()));
///
///     spawn(move || humpty_app.run("0.0.0.0:80").unwrap());
///
///     websocket_app.run();
/// }
///
/// fn message_handler(stream: AsyncStream, message: Message) {
///     println!(
///         "{}: Message received: {}",
///         stream.peer_addr(),
///         message.text().unwrap().trim()
///     );
///
///     stream.send(Message::new("Message received!"));
/// }
/// ```
pub fn async_websocket_handler(
  hook: Arc<Mutex<Sender<WebsocketStream>>>,
) -> impl Fn(Request, Box<dyn ConnectionStream>) {
  move |request: Request, mut stream: Box<dyn ConnectionStream>| {
    if handshake(request, &mut stream).is_ok() {
      hook.lock().unwrap().send(WebsocketStream::new(stream)).ok();
    }
  }
}

/// Performs the WebSocket handshake.
fn handshake(
  request: Request,
  stream: &mut Box<dyn ConnectionStream>,
) -> Result<(), WebsocketError> {
  // Get the handshake key header
  let handshake_key =
    request.headers.get("Sec-WebSocket-Key").ok_or(WebsocketError::HandshakeError)?;

  // Calculate the handshake response
  let sec_websocket_accept = format!("{}{}", handshake_key, MAGIC_STRING).hash().encode();

  // Serialise the handshake response
  let response = Response::empty(StatusCode::SwitchingProtocols)
    .with_header(HeaderType::Upgrade, "websocket")
    .with_header(HeaderType::Connection, "Upgrade")
    .with_header("Sec-WebSocket-Accept", sec_websocket_accept);

  // Transmit the handshake response
  let response_bytes: Vec<u8> = response.into();
  stream.write_all(&response_bytes).map_err(|_| WebsocketError::WriteError)?;
  stream.flush().map_err(|_| WebsocketError::WriteError)?;

  Ok(())
}
