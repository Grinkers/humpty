//! Provides an implementation of WebSocket frames as specified in [RFC 6455 Section 5](https://datatracker.ietf.org/doc/html/rfc6455#section-5).

use crate::humpty_error::{HumptyResult, RequestHeadParsingError};
use crate::stream::{ConnectionStreamRead, ConnectionStreamWrite};
use crate::util;

/// Represents a frame of WebSocket data.
/// Follows [Section 5.2 of RFC 6455](https://datatracker.ietf.org/doc/html/rfc6455#section-5.2)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frame {
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
pub(crate) enum Opcode {
  Continuation = 0x0,
  Text = 0x1,
  Binary = 0x2,
  Close = 0x8,
  Ping = 0x9,
  Pong = 0xA,
}

impl Opcode {
  pub fn parse(value: u8) -> Option<Self> {
    Some(match value {
      0x0 => Self::Continuation,
      0x1 => Self::Text,
      0x2 => Self::Binary,
      0x8 => Self::Close,
      0x9 => Self::Ping,
      0xA => Self::Pong,
      _ => return None,
    })
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

  /// Directly writes a slice to an output connection without copying the buffer.
  pub fn write_unowned_payload_frame<T: ConnectionStreamWrite + ?Sized>(
    write: &T,
    opcode: Opcode,
    payload: impl AsRef<[u8]>,
  ) -> HumptyResult<()> {
    let payload = payload.as_ref();
    let tmp_frame = Self {
      fin: true,
      rsv: [false; 3],
      opcode,
      mask: false,
      length: payload.len() as u64,
      masking_key: [0; 4],
      payload: Vec::new(),
    };

    tmp_frame.write_to_no_flush(write)?;
    write.write_all(payload)?;
    write.flush()?;
    Ok(())
  }

  /// Attempts to read a frame from the given stream, blocking until the frame is read.
  pub fn from_stream<T: ConnectionStreamRead + ?Sized>(stream: &T) -> HumptyResult<Self> {
    let mut header: [u8; 2] = [0; 2];
    stream.read_exact(&mut header)?;

    // Parse header information
    let fin = header[0] & 0x80 != 0;
    let rsv = [header[0] & 0x40 != 0, header[0] & 0x20 != 0, header[0] & 0x10 != 0];
    let opcode =
      Opcode::parse(header[0] & 0xF).ok_or(RequestHeadParsingError::InvalidWebSocketOpcode)?;
    let mask = header[1] & 0x80 != 0;

    let mut length: u64 = (header[1] & 0x7F) as u64;
    if length == 126 {
      stream.read_exact(&mut header)?;
      length = u16::from_be_bytes(header) as u64;
    } else if length == 127 {
      let mut buf: [u8; 8] = [0; 8];
      stream.read_exact(&mut buf)?;
      length = u64::from_be_bytes(buf);
    }

    let masking_key = {
      let mut buf: [u8; 4] = [0; 4];
      if mask {
        stream.read_exact(&mut buf)?;
      }
      buf
    };

    // Read the payload
    let mut payload: Vec<u8> = vec![0; length as usize];
    stream.read_exact(&mut payload)?;

    // Unmask the payload
    payload
      .iter_mut()
      .enumerate()
      // n % 4 is always 0-3
      .for_each(|(i, tem)| *tem ^= util::unwrap_some(masking_key.get(i % 4)));

    Ok(Self { fin, rsv, opcode, mask, length, masking_key, payload })
  }

  pub fn write_to<T: ConnectionStreamWrite + ?Sized>(self, write: &T) -> HumptyResult<()> {
    self.write_to_no_flush(write)?;
    write.flush()?;
    Ok(())
  }

  fn write_to_no_flush<T: ConnectionStreamWrite + ?Sized>(self, write: &T) -> HumptyResult<()> {
    let mut buf = [0, 0];

    // Set the header bits
    buf[0] = (self.fin as u8) << 7
      | (self.rsv[0] as u8) << 6
      | (self.rsv[1] as u8) << 5
      | (self.rsv[2] as u8) << 4
      | self.opcode as u8;

    // Write Header Bits + Length information
    if self.length < 126 {
      buf[1] = (self.mask as u8) << 7 | self.length as u8;
      write.write_all(&buf)?;
    } else if self.length < 65536 {
      buf[1] = (self.mask as u8) << 7 | 126;
      write.write_all(&buf)?;
      write.write_all(&(self.length as u16).to_be_bytes())?;
    } else {
      buf[1] = (self.mask as u8) << 7 | 127;
      write.write_all(&buf)?;
      write.write_all(&(self.length).to_be_bytes())?;
    }

    // Write the masking key (if required)
    if self.mask {
      write.write_all(&self.masking_key)?;
    }

    // Write the payload and return
    write.write_all(&self.payload)?;
    Ok(())
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

    let ms = MockStream::with_data(Vec::new());
    let stream = ms.clone().into_connection_stream();
    frame.write_to(stream.as_stream_write()).unwrap();
    let bytes: Vec<u8> = ms.copy_written_data();

    assert_eq!(bytes, UNMASKED_BYTES.to_vec());
  }
}
