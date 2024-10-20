//! Provides a wrapper around the stream to allow for simpler APIs.
//! TODO docs before release
#![allow(missing_docs)]

use std::fmt::Debug;
use std::io;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

///
/// This represents a raw stream source the server can use to server requests to.
/// Each instance of this represents a dedicated client connection.
///
/// The Stream source is expected to be reference counted and handle concurrent reads/writes.
/// Separate concurrent calls to read and write must be possible independent of each other.
///
/// The implementation of this trait can assume that multiple concurrent calls to either read or write are not made.
///
/// How concurrent invocations of set_read_timeout/set_write_timeout are handled is implementation and platform specific.
/// Possible outcomes are:
/// - blocking read/write calls are canceled (fail with an error)
/// - set_read_timeout/set_write_timeout blocks until read/write calls are finished
/// - set_read_timeout/set_write_timeout only applies for future invocations of read/write and current invocations are left as is and will keep blocking.
///
///
pub trait ConnectionStream: ConnectionStreamRead + ConnectionStreamWrite {
  fn new_ref(&self) -> Box<dyn ConnectionStream>;

  fn peer_addr(&self) -> io::Result<String>;
}

pub trait ConnectionStreamRead: Send + Debug + Read {
  #[deprecated(
    note = "This is a bad idea, only async web sockets need this for now. This fn will be removed later"
  )]
  fn set_read_non_block(&self, on: bool) -> io::Result<()>;

  ///De-mut of Read
  fn read(&self, buf: &mut [u8]) -> io::Result<usize>;

  ///De-mut of BufReader
  fn read_until(&self, end: u8, buf: &mut Vec<u8>) -> io::Result<usize>;

  ///De-mut of Read
  fn read_exact(&self, buf: &mut [u8]) -> io::Result<()>;

  fn new_ref_read(&self) -> Box<dyn Read + Send>;

  fn as_stream_read(&self) -> &dyn ConnectionStreamRead;

  fn new_ref_stream_read(&self) -> Box<dyn ConnectionStreamRead>;

  fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()>;
}

pub trait ConnectionStreamWrite: Send + Debug + Write {
  ///De-mut of Write
  fn write(&self, buf: &[u8]) -> io::Result<usize>;
  ///De-mut of Write
  fn write_all(&self, buf: &[u8]) -> io::Result<()>;

  ///De-mut of Write
  fn flush(&self) -> io::Result<()>;

  fn set_write_timeout(&self, dur: Option<Duration>) -> io::Result<()>;

  fn new_ref_write(&self) -> Box<dyn Write + Send>;

  fn new_ref_stream_write(&self) -> Box<dyn ConnectionStreamWrite>;
  fn as_stream_write(&self) -> &dyn ConnectionStreamWrite;
}

pub trait IntoConnectionStream {
  fn into_connection_stream(self) -> Box<dyn ConnectionStream>;
}

impl IntoConnectionStream for TcpStream {
  fn into_connection_stream(self) -> Box<dyn ConnectionStream> {
    tcp::new(self)
  }
}

impl IntoConnectionStream for Box<dyn ConnectionStream> {
  fn into_connection_stream(self) -> Box<dyn ConnectionStream> {
    self
  }
}

impl IntoConnectionStream for (Box<dyn Read + Send>, Box<dyn Write + Send>) {
  fn into_connection_stream(self) -> Box<dyn ConnectionStream> {
    boxed::new(self.0, self.1)
  }
}

mod tcp {
  use crate::stream::{ConnectionStream, ConnectionStreamRead, ConnectionStreamWrite};
  use crate::util::unwrap_poison;
  use std::fmt::Debug;
  use std::io;
  use std::io::{BufRead, BufReader, BufWriter, Read, Write};
  use std::net::TcpStream;
  use std::ops::Deref;
  use std::sync::{Arc, Mutex};
  use std::time::Duration;

  pub fn new(stream: TcpStream) -> Box<dyn ConnectionStream> {
    Box::new(TcpStreamOuter(Arc::new(TcpStreamInner::new(stream))))
  }

  #[derive(Debug, Clone)]
  struct TcpStreamOuter(Arc<TcpStreamInner>);

  #[derive(Debug)]
  struct TcpStreamInner {
    //TODO, we just play micky mouse with Arc for now because I cannot be asked to implement the stuff we need from BufRead/BufWrite here yet...
    read_mutex: Mutex<BufReader<ArcTcpStream>>,
    write_mutex: Mutex<BufWriter<ArcTcpStream>>,
    stream: ArcTcpStream,
  }

  //TODO get rid of this and replace it with a custom "BufReader/BufWriter" impl in TcpStreamInner
  #[derive(Debug, Clone)]
  struct ArcTcpStream(Arc<TcpStream>);

  impl ArcTcpStream {
    fn new(stream: TcpStream) -> ArcTcpStream {
      ArcTcpStream(Arc::new(stream))
    }
  }

  impl Read for ArcTcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
      (&mut self.0.as_ref()).read(buf)
    }
  }

  impl Write for ArcTcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
      (&mut self.0.deref()).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
      (&mut self.0.deref()).flush()
    }
  }

  impl TcpStreamInner {
    fn new(stream: TcpStream) -> TcpStreamInner {
      let stream = ArcTcpStream::new(stream);
      TcpStreamInner {
        read_mutex: Mutex::new(BufReader::new(stream.clone())),
        write_mutex: Mutex::new(BufWriter::new(stream.clone())),
        stream,
      }
    }
  }

  impl Read for TcpStreamOuter {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
      ConnectionStreamRead::read(self, buf)
    }
  }

  impl ConnectionStreamRead for TcpStreamOuter {
    fn set_read_non_block(&self, on: bool) -> io::Result<()> {
      self.0.stream.0.set_nonblocking(on)
    }

    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
      unwrap_poison(self.0.read_mutex.lock())?.read(buf)
    }

    fn read_until(&self, end: u8, buf: &mut Vec<u8>) -> io::Result<usize> {
      unwrap_poison(self.0.read_mutex.lock())?.read_until(end, buf)
    }

    fn read_exact(&self, buf: &mut [u8]) -> io::Result<()> {
      unwrap_poison(self.0.read_mutex.lock())?.read_exact(buf)
    }

    fn new_ref_read(&self) -> Box<dyn Read + Send> {
      Box::new(self.clone()) as Box<dyn Read + Send>
    }

    fn as_stream_read(&self) -> &dyn ConnectionStreamRead {
      self
    }

    fn new_ref_stream_read(&self) -> Box<dyn ConnectionStreamRead> {
      Box::new(self.clone()) as Box<dyn ConnectionStreamRead>
    }

    fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
      self.0.stream.0.set_read_timeout(dur)
    }
  }

  impl ConnectionStreamWrite for TcpStreamOuter {
    fn write(&self, buf: &[u8]) -> io::Result<usize> {
      unwrap_poison(self.0.write_mutex.lock())?.write(buf)
    }

    fn write_all(&self, buf: &[u8]) -> io::Result<()> {
      unwrap_poison(self.0.write_mutex.lock())?.write_all(buf)
    }

    fn flush(&self) -> io::Result<()> {
      unwrap_poison(self.0.write_mutex.lock())?.flush()
    }

    fn set_write_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
      self.0.stream.0.set_write_timeout(dur)
    }

    fn new_ref_write(&self) -> Box<dyn Write + Send> {
      Box::new(self.clone()) as Box<dyn Write + Send>
    }

    fn new_ref_stream_write(&self) -> Box<dyn ConnectionStreamWrite> {
      Box::new(self.clone()) as Box<dyn ConnectionStreamWrite>
    }

    fn as_stream_write(&self) -> &dyn ConnectionStreamWrite {
      self
    }
  }

  impl Write for TcpStreamOuter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
      ConnectionStreamWrite::write(self, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
      ConnectionStreamWrite::flush(self)
    }
  }

  impl ConnectionStream for TcpStreamOuter {
    fn new_ref(&self) -> Box<dyn ConnectionStream> {
      Box::new(self.clone()) as Box<dyn ConnectionStream>
    }

    fn peer_addr(&self) -> io::Result<String> {
      Ok(format!("{}", self.0.stream.0.peer_addr()?))
    }
  }
}

//TODO what about timeout?
mod boxed {
  use crate::stream::{ConnectionStream, ConnectionStreamRead, ConnectionStreamWrite};
  use crate::util::unwrap_poison;
  use std::fmt::{Debug, Formatter};
  use std::io;
  use std::io::{BufRead, BufReader, BufWriter, Read, Write};
  use std::sync::{Arc, Mutex};
  use std::time::Duration;

  pub fn new(
    read: Box<dyn Read + Send>,
    write: Box<dyn Write + Send>,
  ) -> Box<dyn ConnectionStream> {
    Box::new(BoxStreamOuter(Arc::new(BoxStreamInner {
      read_mutex: Mutex::new(BufReader::new(read)),
      write_mutex: Mutex::new(BufWriter::new(write)),
    }))) as Box<dyn ConnectionStream>
  }

  #[derive(Debug, Clone)]
  struct BoxStreamOuter(Arc<BoxStreamInner>);

  struct BoxStreamInner {
    read_mutex: Mutex<BufReader<Box<dyn Read + Send>>>,
    write_mutex: Mutex<BufWriter<Box<dyn Write + Send>>>,
  }

  impl Debug for BoxStreamInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
      f.write_str("BoxStreamInner")
    }
  }

  impl ConnectionStreamRead for BoxStreamOuter {
    fn set_read_non_block(&self, _: bool) -> io::Result<()> {
      unimplemented!()
    }

    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
      unwrap_poison(self.0.read_mutex.lock())?.read(buf)
    }

    fn read_until(&self, end: u8, buf: &mut Vec<u8>) -> io::Result<usize> {
      unwrap_poison(self.0.read_mutex.lock())?.read_until(end, buf)
    }

    fn read_exact(&self, buf: &mut [u8]) -> io::Result<()> {
      unwrap_poison(self.0.read_mutex.lock())?.read_exact(buf)
    }

    fn new_ref_read(&self) -> Box<dyn Read + Send> {
      Box::new(self.clone()) as Box<dyn Read + Send>
    }

    fn as_stream_read(&self) -> &dyn ConnectionStreamRead {
      self
    }

    fn new_ref_stream_read(&self) -> Box<dyn ConnectionStreamRead> {
      Box::new(self.clone()) as Box<dyn ConnectionStreamRead>
    }

    fn set_read_timeout(&self, _dur: Option<Duration>) -> io::Result<()> {
      Ok(())
    }
  }

  impl Read for BoxStreamOuter {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
      ConnectionStreamRead::read(self, buf)
    }
  }

  impl ConnectionStreamWrite for BoxStreamOuter {
    fn write(&self, buf: &[u8]) -> io::Result<usize> {
      unwrap_poison(self.0.write_mutex.lock())?.write(buf)
    }

    fn write_all(&self, buf: &[u8]) -> io::Result<()> {
      unwrap_poison(self.0.write_mutex.lock())?.write_all(buf)
    }

    fn flush(&self) -> std::io::Result<()> {
      unwrap_poison(self.0.write_mutex.lock())?.flush()
    }

    fn set_write_timeout(&self, _dur: Option<Duration>) -> io::Result<()> {
      Ok(())
    }

    fn new_ref_write(&self) -> Box<dyn Write + Send> {
      Box::new(self.clone()) as Box<dyn Write + Send>
    }

    fn new_ref_stream_write(&self) -> Box<dyn ConnectionStreamWrite> {
      Box::new(self.clone()) as Box<dyn ConnectionStreamWrite>
    }

    fn as_stream_write(&self) -> &dyn ConnectionStreamWrite {
      self
    }
  }

  impl io::Write for BoxStreamOuter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
      ConnectionStreamWrite::write(self, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
      ConnectionStreamWrite::flush(self)
    }
  }

  impl ConnectionStream for BoxStreamOuter {
    fn new_ref(&self) -> Box<dyn ConnectionStream> {
      Box::new(self.clone()) as Box<dyn ConnectionStream>
    }

    fn peer_addr(&self) -> io::Result<String> {
      Ok("Box".to_string())
    }
  }
}

#[cfg(unix)]
impl IntoConnectionStream for std::os::unix::net::UnixStream {
  fn into_connection_stream(self) -> Box<dyn ConnectionStream> {
    unix::new(self)
  }
}

#[cfg(unix)]
mod unix {
  use crate::stream::{ConnectionStream, ConnectionStreamRead, ConnectionStreamWrite};
  use crate::util::unwrap_poison;
  use std::fmt::Debug;
  use std::io;
  use std::io::{BufRead, BufReader, BufWriter, Read, Write};
  use std::ops::Deref;
  use std::os::unix::net::UnixStream;
  use std::sync::{Arc, Mutex};
  use std::time::Duration;

  pub fn new(stream: UnixStream) -> Box<dyn ConnectionStream> {
    Box::new(UnixStreamOuter(Arc::new(UnixStreamInner::new(stream))))
  }

  #[derive(Debug, Clone)]
  struct UnixStreamOuter(Arc<UnixStreamInner>);

  #[derive(Debug)]
  struct UnixStreamInner {
    //TODO, we just play micky mouse with Arc for now because I cannot be asked to implement the stuff we need from BufRead/BufWrite here yet...
    read_mutex: Mutex<BufReader<ArcUnixStream>>,
    write_mutex: Mutex<BufWriter<ArcUnixStream>>,
    stream: ArcUnixStream,
  }

  //TODO get rid of this and replace it with a custom "BufReader/BufWriter" impl in UnixStreamInner
  #[derive(Debug, Clone)]
  struct ArcUnixStream(Arc<UnixStream>);

  impl ArcUnixStream {
    fn new(stream: UnixStream) -> ArcUnixStream {
      ArcUnixStream(Arc::new(stream))
    }
  }

  impl Read for ArcUnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
      (&mut self.0.as_ref()).read(buf)
    }
  }

  impl Write for ArcUnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
      (&mut self.0.deref()).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
      (&mut self.0.deref()).flush()
    }
  }

  impl UnixStreamInner {
    fn new(stream: UnixStream) -> UnixStreamInner {
      let stream = ArcUnixStream::new(stream);
      UnixStreamInner {
        read_mutex: Mutex::new(BufReader::new(stream.clone())),
        write_mutex: Mutex::new(BufWriter::new(stream.clone())),
        stream,
      }
    }
  }

  impl Read for UnixStreamOuter {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
      ConnectionStreamRead::read(self, buf)
    }
  }

  impl ConnectionStreamRead for UnixStreamOuter {
    fn set_read_non_block(&self, _: bool) -> io::Result<()> {
      unimplemented!()
    }

    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
      unwrap_poison(self.0.read_mutex.lock())?.read(buf)
    }

    fn read_until(&self, end: u8, buf: &mut Vec<u8>) -> io::Result<usize> {
      unwrap_poison(self.0.read_mutex.lock())?.read_until(end, buf)
    }

    fn read_exact(&self, buf: &mut [u8]) -> io::Result<()> {
      unwrap_poison(self.0.read_mutex.lock())?.read_exact(buf)
    }

    fn new_ref_read(&self) -> Box<dyn Read + Send> {
      Box::new(self.clone()) as Box<dyn Read + Send>
    }

    fn as_stream_read(&self) -> &dyn ConnectionStreamRead {
      self
    }

    fn new_ref_stream_read(&self) -> Box<dyn ConnectionStreamRead> {
      Box::new(self.clone()) as Box<dyn ConnectionStreamRead>
    }

    fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
      self.0.stream.0.set_read_timeout(dur)
    }
  }

  impl ConnectionStreamWrite for UnixStreamOuter {
    fn write(&self, buf: &[u8]) -> io::Result<usize> {
      unwrap_poison(self.0.write_mutex.lock())?.write(buf)
    }

    fn write_all(&self, buf: &[u8]) -> io::Result<()> {
      unwrap_poison(self.0.write_mutex.lock())?.write_all(buf)
    }

    fn flush(&self) -> io::Result<()> {
      unwrap_poison(self.0.write_mutex.lock())?.flush()
    }

    fn set_write_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
      self.0.stream.0.set_write_timeout(dur)
    }

    fn new_ref_write(&self) -> Box<dyn Write + Send> {
      Box::new(self.clone()) as Box<dyn Write + Send>
    }

    fn new_ref_stream_write(&self) -> Box<dyn ConnectionStreamWrite> {
      Box::new(self.clone()) as Box<dyn ConnectionStreamWrite>
    }

    fn as_stream_write(&self) -> &dyn ConnectionStreamWrite {
      self
    }
  }

  impl Write for UnixStreamOuter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
      ConnectionStreamWrite::write(self, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
      ConnectionStreamWrite::flush(self)
    }
  }

  impl ConnectionStream for UnixStreamOuter {
    fn new_ref(&self) -> Box<dyn ConnectionStream> {
      Box::new(self.clone()) as Box<dyn ConnectionStream>
    }

    fn peer_addr(&self) -> io::Result<String> {
      Ok(
        self
          .0
          .stream
          .0
          .peer_addr()?
          .as_pathname()
          .map(|a| a.to_string_lossy().to_string())
          .unwrap_or_else(|| "".to_string()),
      )
    }
  }
}

//TODO implement for UnixStream(cfg-if unix), Windows Pipe (cfg-if + feature gate?)
