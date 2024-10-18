//! Provides functionality for handling HTTP status codes.
use crate::util::unwrap_poison;
use std::fmt::{Debug, Formatter};
use std::io;
use std::io::{Cursor, Error, Read, Take};
use std::ops::DerefMut;
use std::sync::{Arc, Mutex};

///TODO
#[derive(Debug, Clone)]
pub struct RequestBody(Arc<Mutex<RequestBodyInner>>);

impl RequestBody {
  ///TODO
  pub fn new_with_data_ref<T: AsRef<[u8]>>(data: T) -> RequestBody {
    Self::new_with_data(data.as_ref().to_vec())
  }

  ///TODO
  pub fn new_with_data(data: Vec<u8>) -> RequestBody {
    let len = data.len() as u64;
    let cursor = Cursor::new(data);
    Self::new_with_content_length(Box::new(cursor) as Box<dyn Read + Send + 'static>, len)
  }

  ///TODO
  pub fn new_with_content_length<T: Read + Send + 'static>(read: T, len: u64) -> RequestBody {
    RequestBody(Arc::new(Mutex::new(RequestBodyInner::WithContentLength(
      RequestBodyWithContentLength((Box::new(read) as Box<dyn Read + Send>).take(len)),
    ))))
  }

  ///TODO
  pub fn new_chunked<T: Read + Send + 'static>(read: T) -> RequestBody {
    RequestBody(Arc::new(Mutex::new(RequestBodyInner::Chunked(RequestBodyChunked {
      read: Box::new(read) as Box<dyn Read + Send>,
      eof: false,
      remaining_chunk_length: 0,
    }))))
  }
}

impl RequestBody {
  ///TODO
  pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
    match unwrap_poison(self.0.lock())?.deref_mut() {
      RequestBodyInner::WithContentLength(body) => body.0.read(buf),
      RequestBodyInner::Chunked(body) => body.read(buf),
    }
  }

  ///TODO
  pub fn remaining(&self) -> io::Result<Option<u64>> {
    Ok(match unwrap_poison(self.0.lock())?.deref_mut() {
      RequestBodyInner::WithContentLength(wc) => Some(wc.0.limit()),
      _ => None,
    })
  }
}

impl Read for RequestBody {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    RequestBody::read(self, buf) //de-mut upcall
  }
}

#[derive(Debug)]
enum RequestBodyInner {
  WithContentLength(RequestBodyWithContentLength),
  Chunked(RequestBodyChunked), //Gzipped(...)
                               //...
}

struct RequestBodyWithContentLength(Take<Box<dyn Read + Send>>);

impl Debug for RequestBodyWithContentLength {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    f.write_fmt(format_args!("RequestBodyWithContentLength(remaining={})", self.0.limit()))
  }
}

struct RequestBodyChunked {
  read: Box<dyn Read + Send>,
  eof: bool,
  remaining_chunk_length: u64,
}

impl Debug for RequestBodyChunked {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    f.write_fmt(format_args!(
      "RequestBodyChunked(eof={} remaining_chunk_length={})",
      self.eof, self.remaining_chunk_length
    ))
  }
}

impl RequestBodyChunked {
  pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    if buf.is_empty() {
      return Ok(0);
    }

    if self.eof {
      return Ok(0);
    }

    if self.remaining_chunk_length > 0 {
      let to_read = u64::min(buf.len() as u64, self.remaining_chunk_length) as usize;
      let read = self.read.read(&mut buf[0..to_read])?;
      if read == 0 {
        return Err(io::Error::new(
          io::ErrorKind::UnexpectedEof,
          "chunked transfer encoding suggest more data",
        ));
      }

      //TODO for now panic if Box<dyn Read> reports to have read more than bufsize?
      self.remaining_chunk_length = self.remaining_chunk_length.checked_sub(read as u64).unwrap();
      if self.remaining_chunk_length == 0 {
        let mut tiny_buffer = [0u8; 1];
        self.read.read_exact(&mut tiny_buffer)?;
        if tiny_buffer[0] != b'\r' {
          return Err(Error::new(io::ErrorKind::InvalidData, "Chunk trailer is malformed"));
        }
        self.read.read_exact(&mut tiny_buffer)?;
        if tiny_buffer[0] != b'\n' {
          return Err(Error::new(io::ErrorKind::InvalidData, "Chunk trailer is malformed"));
        }
      }
      return Ok(read);
    }

    let mut small_buffer = [0u8; 32];
    let mut n = 0;
    loop {
      if n >= 17 {
        //If the client prefixes the chunks with '0' characters then we just don't support that.
        return Err(Error::new(
          io::ErrorKind::InvalidData,
          "Chunk size is larger than 2^64 or malformed",
        ));
      }
      self.read.read_exact(&mut small_buffer[n..n + 1])?;
      if small_buffer[n] == b'\r' {
        self.read.read_exact(&mut small_buffer[n..n + 1])?;
        if small_buffer[n] != b'\n' {
          return Err(Error::new(io::ErrorKind::InvalidData, "Chunk size is malformed"));
        }
        break;
      }

      n += 1;
    }

    if n == 0 {
      return Err(Error::new(io::ErrorKind::InvalidData, "Chunk size is malformed"));
    }

    let str = std::str::from_utf8(&small_buffer[0..n])
      .map_err(|_| Error::new(io::ErrorKind::InvalidData, "Chunk size is malformed"))?;
    let chunk_len = u64::from_str_radix(str, 16)
      .map_err(|_| Error::new(io::ErrorKind::InvalidData, "Chunk size is malformed"))?;
    if chunk_len == 0 {
      self.read.read_exact(&mut small_buffer[n..n + 1])?;
      if small_buffer[n] != b'\r' {
        return Err(Error::new(io::ErrorKind::InvalidData, "Chunk trailer is malformed"));
      }
      self.read.read_exact(&mut small_buffer[n..n + 1])?;
      if small_buffer[n] != b'\n' {
        return Err(Error::new(io::ErrorKind::InvalidData, "Chunk trailer is malformed"));
      }

      self.eof = true;
      return Ok(0);
    }

    self.remaining_chunk_length = chunk_len;
    self.read(buf)
  }
}
