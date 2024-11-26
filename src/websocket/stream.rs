//! Provides functionality for working with a WebSocket stream.

use std::io;

use crate::websocket::frame::{Frame, Opcode};
use crate::websocket::message::Message;

use crate::stream::ConnectionStream;
use std::io::{Read, Write};
use std::time::Instant;
use crate::humpty_error::{HumptyError, HumptyResult, WebsocketError};

/// Represents a WebSocket stream.
///
/// Messages can be sent and received through the `send` and `recv` methods.
///
/// The stream also implements the `Read` and `Write` traits to help with compatibility with
///   other crates. These simply wrap and unwrap the bytes in WebSocket frames.
pub struct WebsocketStream {
  pub(crate) stream: Box<dyn ConnectionStream>,
  pub(crate) closed: bool,
  pub(crate) last_pong: Instant,
}

impl WebsocketStream {
  /// Creates a new `WebsocketStream` wrapping an underlying Humpty stream.
  ///
  /// When the `WebsocketStream` is dropped, a close frame will be sent to the client.
  pub fn new(stream: Box<dyn ConnectionStream>) -> Self {
    Self { stream, closed: false, last_pong: Instant::now() }
  }

  /// Blocks until a message is received from the client.
  pub fn recv(&mut self) -> HumptyResult<Message> {
    let message = Message::from_stream(self);


    if let Err(HumptyError::WebsocketError(WebsocketError::ConnectionClosed)) = message {
      self.closed = true;
    }

    message
  }

  /// Sends a message to the client.
  pub fn send(&mut self, message: Message) -> HumptyResult<()> {
    self.send_raw(message.to_frame())
  }

  /// Sends a ping to the client.
  pub fn ping(&mut self) -> HumptyResult<()> {
    let bytes: Vec<u8> = Frame::new(Opcode::Ping, Vec::new()).into();
    self.send_raw(bytes)
  }

  /// Sends a raw frame to the client.
  ///
  /// ## Warning
  /// This function does not check that the frame is valid.
  pub(crate) fn send_raw(&mut self, bytes: impl AsRef<[u8]>) -> HumptyResult<()> {
    self.stream.write_all(bytes.as_ref())?;
    self.stream.flush()?;
    Ok(())
  }

  /// Attempts to get the peer address of this stream.
  pub fn peer_addr(&self) -> io::Result<String> {
    self.stream.peer_addr()
  }

  /// Returns a mutable reference to the underlying stream.
  pub fn inner(&mut self) -> &mut Box<dyn ConnectionStream> {
    &mut self.stream
  }
}

impl Read for WebsocketStream {
  fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
    if let Ok(message) = self.recv() {
      let bytes = message.bytes();

      if bytes.len() <= buf.len() {
        buf[..bytes.len()].copy_from_slice(bytes);
        Ok(bytes.len())
      } else {
        Err(std::io::Error::new(std::io::ErrorKind::Other, "Buffer is too small"))
      }
    } else {
      Err(std::io::Error::new(std::io::ErrorKind::Other, "Failed to receive message"))
    }
  }
}

impl Write for WebsocketStream {
  fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
    let message = Message::new(buf);

    if self.send(message).is_ok() {
      Ok(buf.len())
    } else {
      Err(std::io::Error::new(std::io::ErrorKind::Other, "Failed to send message"))
    }
  }

  fn flush(&mut self) -> std::io::Result<()> {
    Ok(())
  }
}

impl Drop for WebsocketStream {
  fn drop(&mut self) {
    if !self.closed {
      self.stream.write_all(Frame::new(Opcode::Close, Vec::new()).as_ref()).ok();
    }
  }
}
