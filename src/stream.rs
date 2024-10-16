//! Provides a wrapper around the stream to allow for simpler APIs.

use std::io::{Error, Read, Write};
use std::net::SocketAddr;
use std::time::Duration;

use std::net::TcpStream;

/// Represents a connection to a remote client or server.
///
/// This is typically a wrapper around `TcpStream`, but is required to allow for a single API
///   to be used to process both regular and TLS connections.
pub enum Stream {
  /// A regular TCP stream.
  Tcp(TcpStream),
}

impl Read for Stream {
  fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
    match self {
      Stream::Tcp(stream) => stream.read(buf),
    }
  }
}

impl Write for Stream {
  fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
    match self {
      Stream::Tcp(stream) => stream.write(buf),
    }
  }

  fn flush(&mut self) -> std::io::Result<()> {
    match self {
      Stream::Tcp(stream) => stream.flush(),
    }
  }
}

impl Stream {
  /// Returns the socket address of the remote peer of this connection.
  pub fn peer_addr(&self) -> Result<SocketAddr, Error> {
    match self {
      Stream::Tcp(stream) => stream.peer_addr(),
    }
  }

  /// Shuts down both the read and write halves of this connection.
  pub fn shutdown(&self) -> std::io::Result<()> {
    match self {
      Stream::Tcp(stream) => stream.shutdown(std::net::Shutdown::Both),
    }
  }

  /// Sets the read and write timeouts of the stream.
  pub fn set_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
    match self {
      Stream::Tcp(stream) => {
        stream.set_read_timeout(timeout)?;
        stream.set_write_timeout(timeout)
      }
    }
  }

  /// Sets this connection to nonblocking mode.
  pub fn set_nonblocking(&self) -> std::io::Result<()> {
    match self {
      Stream::Tcp(stream) => stream.set_nonblocking(true),
    }
  }

  /// Sets this connection to blocking mode.
  pub fn set_blocking(&self) -> std::io::Result<()> {
    match self {
      Stream::Tcp(stream) => stream.set_nonblocking(false),
    }
  }
}
