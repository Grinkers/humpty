//! Provides an implementation of WebSocket frames as specified in [RFC 6455 Section 5](https://datatracker.ietf.org/doc/html/rfc6455#section-5).

use crate::websocket::error::WebsocketError;

use crate::stream::ConnectionStreamRead;
use crate::websocket::restion::Restion;
use std::convert::TryFrom;

/// Represents a frame of WebSocket data.
/// Follows [Section 5.2 of RFC 6455](https://datatracker.ietf.org/doc/html/rfc6455#section-5.2)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
  pub(crate) fin: bool,
  pub(crate) rsv: [bool; 3],
  pub(crate) opcode: Opcode,
  pub(crate) mask: bool,
  pub(crate) length: u64,
  pub(crate) masking_key: [u8; 4],
  pub(crate) payload: Vec<u8>,
}

/// Represents the type of WebSocket frame.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
  Continuation = 0x0,
  Text = 0x1,
  Binary = 0x2,
  Close = 0x8,
  Ping = 0x9,
  Pong = 0xA,
}

impl TryFrom<u8> for Opcode {
  type Error = WebsocketError;

  fn try_from(value: u8) -> Result<Self, Self::Error> {
    match value {
      0x0 => Ok(Self::Continuation),
      0x1 => Ok(Self::Text),
      0x2 => Ok(Self::Binary),
      0x8 => Ok(Self::Close),
      0x9 => Ok(Self::Ping),
      0xA => Ok(Self::Pong),
      _ => Err(WebsocketError::InvalidOpcode),
    }
  }
}

impl Frame {
  /// Creates a new frame with the given parameters.
  /// Does not mask the payload.
  pub fn new(opcode: Opcode, payload: Vec<u8>) -> Self {
    Self {
      fin: true,
      rsv: [false; 3],
      opcode,
      mask: false,
      length: payload.len() as u64,
      masking_key: [0; 4],
      payload,
    }
  }

  /// Attempts to read a frame from the given stream, blocking until the frame is read.
  pub fn from_stream<T: ConnectionStreamRead + ?Sized>(stream: &T) -> Result<Self, WebsocketError> {
    let mut buf: [u8; 2] = [0; 2];
    stream.read_exact(&mut buf).map_err(|_| WebsocketError::ReadError)?;

    Self::from_stream_inner(stream, buf)
  }

  /// Attempts to read a frame from the given stream, immediately returning instead of blocking if there is no frame to read.

  #[allow(deprecated)]
  pub fn from_stream_nonblocking<T: ConnectionStreamRead + ?Sized>(
    stream: &T,
  ) -> Restion<Self, WebsocketError> {
    // Set the stream to nonblocking to read the header
    if stream.set_read_non_block(true).is_err() {
      return Restion::Err(WebsocketError::ReadError);
    }

    // Attempt to read the header
    let mut buf: [u8; 2] = [0; 2];
    let result = stream.read(&mut buf);

    // Set the stream to blocking for further reads
    if stream.set_read_non_block(false).is_err() {
      return Restion::Err(WebsocketError::ReadError);
    }

    match result {
      Ok(0) => Restion::None,
      Ok(_) => Self::from_stream_inner(stream, buf).into(),
      Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => Restion::None,
      Err(_) => Restion::Err(WebsocketError::ReadError),
    }
  }

  fn from_stream_inner<T: ConnectionStreamRead + ?Sized>(
    stream: &T,
    mut header: [u8; 2],
  ) -> Result<Self, WebsocketError> {
    // Parse header information
    let fin = header[0] & 0x80 != 0;
    let rsv = [header[0] & 0x40 != 0, header[0] & 0x20 != 0, header[0] & 0x10 != 0];
    let opcode = Opcode::try_from(header[0] & 0xF)?;
    let mask = header[1] & 0x80 != 0;

    let mut length: u64 = (header[1] & 0x7F) as u64;
    if length == 126 {
      stream.read_exact(&mut header).map_err(|_| WebsocketError::ReadError)?;
      length = u16::from_be_bytes(header) as u64;
    } else if length == 127 {
      let mut buf: [u8; 8] = [0; 8];
      stream.read_exact(&mut buf).map_err(|_| WebsocketError::ReadError)?;
      length = u64::from_be_bytes(buf);
    }

    let masking_key = {
      let mut buf: [u8; 4] = [0; 4];
      if mask {
        stream.read_exact(&mut buf).map_err(|_| WebsocketError::ReadError)?;
      }
      buf
    };

    // Read the payload
    let mut payload: Vec<u8> = vec![0; length as usize];
    stream.read_exact(&mut payload).map_err(|_| WebsocketError::ReadError)?;

    // Unmask the payload
    payload.iter_mut().enumerate().for_each(|(i, tem)| *tem ^= masking_key[i % 4]);

    Ok(Self { fin, rsv, opcode, mask, length, masking_key, payload })
  }
}

impl From<Frame> for Vec<u8> {
  fn from(f: Frame) -> Self {
    let mut buf: Vec<u8> = vec![0; 2];

    // Set the header bits
    buf[0] = (f.fin as u8) << 7
      | (f.rsv[0] as u8) << 6
      | (f.rsv[1] as u8) << 5
      | (f.rsv[2] as u8) << 4
      | f.opcode as u8;

    // Set the length information
    if f.length < 126 {
      buf[1] = (f.mask as u8) << 7 | f.length as u8;
    } else if f.length < 65536 {
      buf[1] = (f.mask as u8) << 7 | 126;
      buf.extend_from_slice(&(f.length as u16).to_be_bytes());
    } else {
      buf[1] = (f.mask as u8) << 7 | 127;
      buf.extend_from_slice(&(f.length).to_be_bytes());
    }

    // Add the masking key (if required)
    if f.mask {
      buf.extend_from_slice(&f.masking_key);
    }

    // Add the payload and return
    buf.extend_from_slice(&f.payload);

    buf
  }
}

impl AsRef<[u8]> for Frame {
  fn as_ref(&self) -> &[u8] {
    &self.payload
  }
}

#[cfg(test)]
mod test {
  #![allow(clippy::unusual_byte_groupings)]
  #![allow(dead_code)]

  use crate::stream::{ConnectionStream, IntoConnectionStream};
  use crate::websocket::frame::{Frame, Opcode};
  use std::collections::VecDeque;
  use std::io::{Read, Write};
  use std::sync::{Arc, Mutex};

  #[derive(Debug, Clone)]
  pub struct MockStream {
    read_data: Arc<Mutex<VecDeque<u8>>>,
    write_data: Arc<Mutex<Vec<u8>>>,
  }

  impl MockStream {
    pub fn with_data(data: Vec<u8>) -> Self {
      Self {
        read_data: Arc::new(Mutex::new(VecDeque::from_iter(data.iter().cloned()))),
        write_data: Arc::new(Mutex::new(Vec::new())),
      }
    }

    pub fn copy_written_data(&self) -> Vec<u8> {
      self.write_data.lock().unwrap().clone()
    }
  }

  impl IntoConnectionStream for MockStream {
    fn into_connection_stream(self) -> Box<dyn ConnectionStream> {
      let cl = self.clone();
      (Box::new(cl) as Box<dyn Read + Send>, Box::new(self) as Box<dyn Write + Send>)
        .into_connection_stream()
    }
  }

  impl Write for MockStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
      self.write_data.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
      Ok(())
    }
  }

  impl Read for MockStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
      let mut bytes_written: usize = 0;

      for byte in buf {
        if let Some(new_byte) = self.read_data.lock().unwrap().pop_front() {
          *byte = new_byte;
          bytes_written += 1;
        } else {
          return Ok(bytes_written);
        }
      }

      Ok(bytes_written)
    }
  }

  #[rustfmt::skip]
    pub const FRAME_1_BYTES: [u8; 12] = [
        0b0000_0001, // not fin, opcode text
        0b1_0000110, // mask, payload length 6
        0x69, 0x69, 0x69, 0x69, // masking key 0x69696969
        1, 12, 5, 5, 6, 73 // masked payload "hello "
    ];

  #[rustfmt::skip]
    pub const FRAME_2_BYTES: [u8; 11] = [
        0b1000_0000, // fin, opcode continuation
        0b1_0000101, // mask, payload length 5
        0x69, 0x69, 0x69, 0x69, // masking key 0x69696969
        30, 6, 27, 5, 13 // masked payload "world"
    ];

  #[rustfmt::skip]
    pub const STANDALONE_FRAME_BYTES: [u8; 11] = [
        0b1000_0001, // fin, opcode text
        0b1_0000101, // mask, payload length 5
        0x69, 0x69, 0x69, 0x69, // masking key 0x69696969
        1, 12, 5, 5, 6 // masked payload "hello"
    ];

  #[rustfmt::skip]
    pub const UNMASKED_BYTES: [u8; 13] = [
        0b1000_0001, // fin, opcode text
        0b0_0001011, // not mask, payload length 11
        b'h', b'e', b'l', b'l', b'o', b' ', b'w', b'o', b'r', b'l', b'd' // unmasked payload "hello world"
    ];

  #[rustfmt::skip]
    pub const MEDIUM_FRAME_BYTES: [u8; 8] = [
        0b1000_0001, // fin, opcode text
        0b1_1111110, // mask, payload length 126 (extended payload length 16 bit)
        0x01, 0x00, // extended payload length of 256
        0x69, 0x69, 0x69, 0x69, // masking key 0x69696969
    ];

  #[rustfmt::skip]
    pub const LONG_FRAME_BYTES: [u8; 14] = [
        0b1000_0001, // fin, opcode text
        0b1_1111111, // mask, payload length 127 (extended payload length 64 bit)
        0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, // extended payload length of 65536
        0x69, 0x69, 0x69, 0x69, // masking key 0x69696969
    ];

  #[test]
  fn test_initial_frame() {
    let mut bytes = Vec::with_capacity(23);
    bytes.extend(FRAME_1_BYTES);
    bytes.extend(FRAME_2_BYTES);

    let stream = MockStream::with_data(bytes);
    let frame = Frame::from_stream(stream.into_connection_stream().as_ref()).unwrap();

    let expected_frame = Frame {
      fin: false,
      rsv: [false; 3],
      opcode: Opcode::Text,
      mask: true,
      masking_key: [0x69; 4],
      length: 6,
      payload: b"hello ".to_vec(),
    };

    assert_eq!(frame, expected_frame);
  }

  #[test]
  fn test_continuation_frame() {
    let stream = MockStream::with_data(FRAME_2_BYTES.to_vec());
    let frame = Frame::from_stream(stream.into_connection_stream().as_ref()).unwrap();

    let expected_frame = Frame {
      fin: true,
      rsv: [false; 3],
      opcode: Opcode::Continuation,
      mask: true,
      masking_key: [0x69; 4],
      length: 5,
      payload: b"world".to_vec(),
    };

    assert_eq!(frame, expected_frame);
  }

  #[test]
  fn test_standalone_frame() {
    let stream = MockStream::with_data(STANDALONE_FRAME_BYTES.to_vec());
    let frame = Frame::from_stream(stream.into_connection_stream().as_ref()).unwrap();

    let expected_frame = Frame {
      fin: true,
      rsv: [false; 3],
      opcode: Opcode::Text,
      mask: true,
      masking_key: [0x69; 4],
      length: 5,
      payload: b"hello".to_vec(),
    };

    assert_eq!(frame, expected_frame);
  }

  #[test]
  fn test_medium_frame() {
    let mut bytes = Vec::with_capacity(264);
    bytes.extend(MEDIUM_FRAME_BYTES);
    bytes.extend(vec![b'x' ^ 0x69; 256]);

    let stream = MockStream::with_data(bytes);
    let frame = Frame::from_stream(stream.into_connection_stream().as_ref()).unwrap();

    let expected_frame = Frame {
      fin: true,
      rsv: [false; 3],
      opcode: Opcode::Text,
      mask: true,
      masking_key: [0x69; 4],
      length: 256,
      payload: vec![b'x'; 256],
    };

    assert_eq!(frame, expected_frame);
  }

  #[test]
  fn test_long_frame() {
    let mut bytes = Vec::with_capacity(65550);
    bytes.extend(LONG_FRAME_BYTES);
    bytes.extend(vec![b'x' ^ 0x69; 65536]);

    let stream = MockStream::with_data(bytes);

    let frame = Frame::from_stream(stream.into_connection_stream().as_ref()).unwrap();

    let expected_frame = Frame {
      fin: true,
      rsv: [false; 3],
      opcode: Opcode::Text,
      mask: true,
      masking_key: [0x69; 4],
      length: 65536,
      payload: vec![b'x'; 65536],
    };

    assert_eq!(frame, expected_frame);
  }

  #[test]
  fn test_write() {
    let frame = Frame {
      fin: true,
      rsv: [false; 3],
      opcode: Opcode::Text,
      mask: false,
      masking_key: [0; 4],
      length: 11,
      payload: b"hello world".to_vec(),
    };

    let bytes: Vec<u8> = frame.into();

    assert_eq!(bytes, UNMASKED_BYTES.to_vec());
  }
}
