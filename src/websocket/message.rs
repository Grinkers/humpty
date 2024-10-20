//! Provides an abstraction over WebSocket frames called `Message`.

use crate::websocket::error::WebsocketError;
use crate::websocket::frame::{Frame, Opcode};
use crate::websocket::restion::Restion;
use crate::websocket::WebsocketStream;

use std::io::Write;
use std::time::Instant;

/// Represents a WebSocket message.
#[derive(Debug, Clone)]
pub struct Message {
  payload: Vec<u8>,
  text: bool,
}

impl Message {
  /// Creates a new message with the given payload.
  ///
  /// Marks the message as text if the payload is valid UTF-8.
  /// To avoid this behaviour, use the `Message::new_binary` constructor.
  pub fn new<T>(payload: T) -> Self
  where
    T: AsRef<[u8]>,
  {
    Self { payload: payload.as_ref().to_vec(), text: std::str::from_utf8(payload.as_ref()).is_ok() }
  }

  /// Creates a new binary message with the given payload.
  pub fn new_binary<T>(payload: T) -> Self
  where
    T: AsRef<[u8]>,
  {
    Self { payload: payload.as_ref().to_vec(), text: false }
  }

  /// Attempts to read a message from the given stream.
  ///
  /// Silently responds to pings with pongs, as specified in [RFC 6455 Section 5.5.2](https://datatracker.ietf.org/doc/html/rfc6455#section-5.5.2).
  pub fn from_stream(stream: &mut WebsocketStream) -> Result<Self, WebsocketError> {
    let mut frames: Vec<Frame> = Vec::new();

    // Keep reading frames until we get the finish frame
    while frames.last().map(|f| !f.fin).unwrap_or(true) {
      let frame = Frame::from_stream(stream.stream.as_stream_read())?;

      // If this is a ping, respond with a pong
      if frame.opcode == Opcode::Ping {
        let pong = Frame::new(Opcode::Pong, frame.payload);
        stream.stream.write_all(pong.as_ref()).map_err(|_| WebsocketError::WriteError)?;
        continue;
      }

      // If this is a pong, store the time
      if frame.opcode == Opcode::Pong {
        stream.last_pong = Instant::now();
        continue;
      }

      // If this closes the connection, return the error
      if frame.opcode == Opcode::Close {
        let close = Frame::new(Opcode::Close, frame.payload);
        stream.stream.write_all(close.as_ref()).map_err(|_| WebsocketError::WriteError)?;
        return Err(WebsocketError::ConnectionClosed);
      }

      frames.push(frame);
    }

    // Concatenate the payloads of all frames into a single payload
    let payload = frames.iter().fold(Vec::new(), |mut acc, frame| {
      acc.extend(frame.payload.iter());
      acc
    });

    Ok(Self { payload, text: frames.first().map(|f| f.opcode == Opcode::Text).unwrap_or(false) })
  }

  /// Attempts to read a message from the given stream without blocking.
  ///
  /// Silently responds to pings with pongs, as specified in [RFC 6455 Section 5.5.2](https://datatracker.ietf.org/doc/html/rfc6455#section-5.5.2).
  pub fn from_stream_nonblocking(stream: &mut WebsocketStream) -> Restion<Self, WebsocketError> {
    let mut frames: Vec<Frame> = Vec::new();
    let mut is_first_frame = true;

    // Keep reading frames until we get the finish frame
    while frames.last().map(|f| !f.fin).unwrap_or(true) {
      let frame = if is_first_frame {
        Frame::from_stream_nonblocking(stream.stream.as_stream_read()) //This is stupid
      } else {
        Frame::from_stream(stream.stream.as_stream_read()).into()
      };

      match frame {
        Restion::Ok(frame) => {
          // If this is a ping, respond with a pong
          if frame.opcode == Opcode::Ping {
            let pong = Frame::new(Opcode::Pong, frame.payload);
            if stream.stream.write_all(pong.as_ref()).is_err() {
              return Restion::Err(WebsocketError::WriteError);
            }
            continue;
          }

          // If this is a pong, store the time
          if frame.opcode == Opcode::Pong {
            stream.last_pong = Instant::now();
            continue;
          }

          // If this closes the connection, return the error
          if frame.opcode == Opcode::Close {
            let close = Frame::new(Opcode::Close, frame.payload);
            if stream.stream.write_all(close.as_ref()).is_err() {
              return Restion::Err(WebsocketError::WriteError);
            }
            return Restion::Err(WebsocketError::ConnectionClosed);
          }

          frames.push(frame);
        }
        Restion::Err(e) => return Restion::Err(e),
        Restion::None => return Restion::None,
      }

      is_first_frame = false;
    }

    // Concatenate the payloads of all frames into a single payload
    let payload = frames.iter().fold(Vec::new(), |mut acc, frame| {
      acc.extend(frame.payload.iter());
      acc
    });

    Restion::Ok(Self {
      payload,
      text: frames.first().map(|f| f.opcode == Opcode::Text).unwrap_or(false),
    })
  }

  /// Returns whether the sender of this message specified that it contains text.
  pub fn is_text(&self) -> bool {
    self.text
  }

  /// Returns the payload as a string, if possible.
  ///
  /// If the opcode is `Opcode::Text` (`0x1`), but the payload is not valid UTF-8, the function will return `None`.
  /// Otherwise, it will not attempt to convert the payload to a string and will immediately return `None`.
  pub fn text(&self) -> Option<&str> {
    if self.text {
      std::str::from_utf8(&self.payload).ok()
    } else {
      None
    }
  }

  /// Returns the payload as a slice of bytes.
  pub fn bytes(&self) -> &[u8] {
    &self.payload
  }

  /// Converts the message to a `Vec<u8>` for transmission.
  pub fn to_frame(self) -> Vec<u8> {
    if self.text {
      Frame::new(Opcode::Text, self.payload).into()
    } else {
      Frame::new(Opcode::Binary, self.payload).into()
    }
  }
}

impl AsRef<[u8]> for Message {
  fn as_ref(&self) -> &[u8] {
    &self.payload
  }
}
