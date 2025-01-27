//! Provides functionality for working with a WebSocket stream.

use crate::websocket::frame::{Frame, Opcode};
use crate::websocket::message::WebsocketMessage;
use std::collections::VecDeque;
use std::{io, mem};

use crate::humpty_error::{HumptyError, HumptyResult, RequestHeadParsingError};
use crate::stream::ConnectionStream;
use crate::util::{unwrap_poison, unwrap_some};
use crate::{error_log, trace_log, warn_log};
use std::io::{Cursor, ErrorKind, Read, Write};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug)]
struct WebSocketGuard {
  closed: AtomicBool,
  write_mutex: Mutex<()>,
  stream: Box<dyn ConnectionStream>,
}

/// Sending side of a web socket
#[derive(Debug, Clone)]
#[repr(transparent)]
pub struct WebsocketSender(Arc<WebSocketGuard>);

/// Creates a new WebSocket receiver sender pair.
pub fn new(connection: &dyn ConnectionStream) -> (WebsocketSender, WebsocketReceiver) {
  let guard = Arc::new(WebSocketGuard {
    closed: AtomicBool::new(false),
    write_mutex: Mutex::new(()),
    stream: connection.new_ref(),
  });

  let sender = WebsocketSender(guard.clone());

  let receiver = WebsocketReceiver {
    guard,
    state: Vec::new(),
    cursor: Default::default(),
    unhandled_messages: Default::default(),
  };

  (sender, receiver)
}

impl WebsocketSender {
  /// returns true if this web socket sender refers to a closed web socket.
  #[must_use]
  pub fn is_closed(&self) -> bool {
    self.0.closed.load(SeqCst)
  }

  /// Sends a message to the client.
  pub fn send(&self, message: WebsocketMessage) -> HumptyResult<()> {
    match message {
      WebsocketMessage::Text(txt) => self.text(txt),
      WebsocketMessage::Binary(bin) => self.binary(bin),
      WebsocketMessage::Ping => self.ping(),
      WebsocketMessage::Pong => self.pong(),
    }
  }

  /// Closes the Websocket sending the close frame.
  pub fn close(&self) -> HumptyResult<()> {
    let _g = unwrap_poison(self.0.write_mutex.lock())?;

    if self.0.closed.swap(true, SeqCst) {
      return Ok(()); //ALREADY CLOSED!
    }

    Frame::new(Opcode::Close, Vec::new()).write_to(self.0.stream.as_stream_write())
  }

  /// Sends a binary message to the client
  pub fn binary(&self, message: impl Into<Vec<u8>>) -> HumptyResult<()> {
    let _g = unwrap_poison(self.0.write_mutex.lock())?;
    Frame::new(Opcode::Binary, message.into()).write_to(self.0.stream.as_stream_write())
  }

  /// Sends a text message to the client
  pub fn text(&self, message: impl ToString) -> HumptyResult<()> {
    let _g = unwrap_poison(self.0.write_mutex.lock())?;
    Frame::new(Opcode::Text, message.to_string().into_bytes())
      .write_to(self.0.stream.as_stream_write())
  }

  /// Sends a ping to the client.
  pub fn ping(&self) -> HumptyResult<()> {
    let _g = unwrap_poison(self.0.write_mutex.lock())?;
    Frame::new(Opcode::Ping, Vec::new()).write_to(self.0.stream.as_stream_write())
  }

  /// Sends a pong message to the client.
  pub fn pong(&self) -> HumptyResult<()> {
    let _g = unwrap_poison(self.0.write_mutex.lock())?;
    Frame::new(Opcode::Ping, Vec::new()).write_to(self.0.stream.as_stream_write())
  }

  /// Attempts to get the peer address of this stream.
  pub fn peer_addr(&self) -> HumptyResult<String> {
    Ok(self.0.stream.peer_addr()?)
  }
}

/// Receiving side of a web socket
#[derive(Debug)]
pub struct WebsocketReceiver {
  guard: Arc<WebSocketGuard>,
  state: Vec<Frame>,
  cursor: Cursor<Vec<u8>>,
  unhandled_messages: VecDeque<WebsocketMessage>,
}

/// Return enum for the fn WebsocketReceiver::read_message_timeout
#[derive(Debug)]
pub enum ReadMessageTimeoutResult {
  /// We got a message without running into any timeout
  Message(WebsocketMessage),
  /// We got a timeout before the first byte of the next message was received.
  Timeout,
  /// We received the Close 'Message' without running into any timeout
  Closed,
}

impl WebsocketReceiver {
  /// Closes the Websocket sending the close frame to the client.
  pub fn close(&self) -> HumptyResult<()> {
    let _g = unwrap_poison(self.guard.write_mutex.lock())?;

    if self.guard.closed.swap(true, SeqCst) {
      return Ok(()); //ALREADY CLOSED!
    }

    Frame::new(Opcode::Close, Vec::new()).write_to(self.guard.stream.as_stream_write())
  }

  /// If the WebsocketReceiver is used with the "io::Read" trait then
  /// any ping/pong messages received are not handled. They are instead queued.
  /// This fn pop_front's the head of the queue.
  pub fn unhandled(&mut self) -> Option<WebsocketMessage> {
    self.unhandled_messages.pop_front()
  }

  /// receive the next complete message.
  /// Ok(None) indicates that the web socket is closed.
  pub fn read_message(&mut self) -> HumptyResult<Option<WebsocketMessage>> {
    if let Some(message) = self.unhandled_messages.pop_front() {
      return Ok(Some(message));
    }

    self.read_next_frame()
  }

  /// This fn waits until timeout expires before the first byte of the next Message is received.
  ///
  /// The specified timeout is completely independent of the read timeout of the HumptyServer.
  /// Values where timeout.is_zero() returns true may cause Err to be returned depending on how the
  /// underlying connection treats this value.
  ///
  /// The actual reading of the Message is still subject to the normal timeout mechanics.
  /// Should the client pause in the middle of a frame before sending the rest of it then
  /// this fn will return the fatal error Err(TimedOut).
  ///
  /// Passing None for timeout means Infinite timeout until either the client closes the connection
  /// sends a byte or the OS reset the connection;
  ///
  pub fn read_message_timeout(
    &mut self,
    timeout: Option<Duration>,
  ) -> HumptyResult<ReadMessageTimeoutResult> {
    if let Some(message) = self.unhandled_messages.pop_front() {
      return Ok(ReadMessageTimeoutResult::Message(message));
    }

    if self.guard.stream.available() == 0 {
      if self.guard.closed.load(SeqCst) {
        return Ok(ReadMessageTimeoutResult::Closed);
      }

      let old_timeout = self.guard.stream.get_read_timeout()?.as_ref().cloned();
      if let Err(err) = self.guard.stream.set_read_timeout(timeout) {
        self.guard.closed.store(true, SeqCst);
        error_log!("WebsocketReceiver::read_message_timeout error setting timeout for 1st byte of next frame {}", &err);
        return Err(HumptyError::from(err));
      }
      let res = self.guard.stream.ensure_readable();
      let res2 = self.guard.stream.set_read_timeout(old_timeout);

      if let Err(err) = res2 {
        self.guard.closed.store(true, SeqCst);
        error_log!("WebsocketReceiver::read_message_timeout error setting timeout back to read timeout after waiting for 1st byte of next frame {}", &err);
        return Err(HumptyError::from(err));
      }

      if let Err(err) = res {
        if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) {
          return Ok(ReadMessageTimeoutResult::Timeout);
        }
        self.guard.closed.store(true, SeqCst);
        error_log!("WebsocketReceiver::read_message_timeout error while waiting for 1st byte of next frame {}", &err);
        return Err(HumptyError::from(err));
      }
    }

    match self.read_next_frame() {
      Ok(Some(message)) => Ok(ReadMessageTimeoutResult::Message(message)),
      Ok(None) => Ok(ReadMessageTimeoutResult::Closed),
      Err(err) => Err(err),
    }
  }

  /// Attempts to read a message from the given stream.
  ///
  /// Silently responds to pings with pongs, as specified in [RFC 6455 Section 5.5.2](https://datatracker.ietf.org/doc/html/rfc6455#section-5.5.2).
  fn read_next_frame(&mut self) -> HumptyResult<Option<WebsocketMessage>> {
    if self.guard.closed.load(SeqCst) {
      return Ok(None);
    }

    let as_read = self.guard.stream.as_stream_read();
    // Keep reading frames until we get the finish frame
    while self.state.last().map(|f| !f.fin).unwrap_or(true) {
      let frame = Frame::from_stream(as_read).inspect_err(|e| {
        self.guard.closed.store(true, SeqCst);
        error_log!("WebsocketReceiver::read_next_frame Frame::from_stream error: {}", e);
      })?;

      if frame.opcode == Opcode::Ping {
        return Ok(Some(WebsocketMessage::Ping));
      }

      if frame.opcode == Opcode::Pong {
        return Ok(Some(WebsocketMessage::Pong));
      }

      if frame.opcode == Opcode::Close {
        self.guard.closed.store(true, SeqCst);
        if self.state.is_empty() {
          return Ok(None);
        }

        return Err(HumptyError::RequestHeadParsing(
          RequestHeadParsingError::WebSocketClosedDuringPendingMessage,
        ));
      }

      self.state.push(frame);
    }

    let frames = mem::take(&mut self.state);
    let frame_type = unwrap_some(frames.first()).opcode;

    let size = frames.iter().map(|f| f.payload.len()).sum();
    let mut payload = Vec::with_capacity(size);

    for (idx, frame) in frames.into_iter().enumerate() {
      if idx != 0 && frame.opcode != Opcode::Continuation {
        return Err(HumptyError::RequestHeadParsing(
          RequestHeadParsingError::UnexpectedWebSocketOpcode,
        ));
      }
      payload.extend_from_slice(frame.payload.as_slice());
    }

    match frame_type {
      Opcode::Text => {
        let payload = String::from_utf8(payload).map_err(|e| {
          self.guard.closed.store(true, SeqCst);
          HumptyError::RequestHeadParsing(RequestHeadParsingError::WebSocketTextMessageIsNotUtf8(
            e.into_bytes(),
          ))
        })?;

        Ok(Some(WebsocketMessage::Text(payload)))
      }
      Opcode::Binary => Ok(Some(WebsocketMessage::Binary(payload))),
      _ => {
        self.guard.closed.store(true, SeqCst);
        Err(HumptyError::RequestHeadParsing(RequestHeadParsingError::UnexpectedWebSocketOpcode))
      }
    }
  }
}

impl Read for WebsocketReceiver {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    loop {
      let cnt = self.cursor.read(buf)?;
      if cnt != 0 {
        return Ok(cnt);
      }

      return match self.read_next_frame() {
        Ok(Some(message)) => match message.bytes() {
          Some(bytes) => {
            if bytes.len() <= buf.len() {
              unwrap_some(buf.get_mut(..bytes.len())).copy_from_slice(bytes);
              Ok(bytes.len())
            } else {
              self.cursor = Cursor::new(bytes.to_vec());
              continue;
            }
          }
          None => {
            self.unhandled_messages.push_back(message);
            continue;
          }
        },
        Ok(None) => Ok(0),
        Err(err) => {
          return Err(err.into());
        }
      };
    }
  }
}

impl Write for WebsocketSender {
  fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
    if self.0.closed.load(SeqCst) {
      return Err(io::Error::from(ErrorKind::ConnectionReset));
    }
    Frame::write_unowned_payload_frame(self.0.stream.as_stream_write(), Opcode::Binary, buf)
      .inspect_err(|e| {
        self.0.closed.store(true, SeqCst);
        error_log!("WebsocketSender::write error: {}", e);
      })?;
    Ok(buf.len())
  }

  fn flush(&mut self) -> io::Result<()> {
    Ok(())
  }
}

impl Drop for WebSocketGuard {
  fn drop(&mut self) {
    trace_log!("WebsocketReceiver::drop");
    if self.closed.load(SeqCst) {
      trace_log!("WebsocketReceiver::drop already closed");
      return;
    }

    trace_log!("WebsocketReceiver::drop closing...");
    if let Err(err) = Frame::new(Opcode::Close, Vec::new()).write_to(self.stream.as_stream_write())
    {
      warn_log!("WebsocketSender::drop error: {}", err);
    }
    trace_log!("WebsocketReceiver::drop closed.");
  }
}
