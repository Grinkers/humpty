use humpty::stream::{ConnectionStream, IntoConnectionStream};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct MockStream {
  read_data: Arc<Mutex<VecDeque<u8>>>,
  write_data: Arc<Mutex<Vec<u8>>>,
}

impl MockStream {
  pub fn with_data(data: VecDeque<u8>) -> Self {
    Self { read_data: Arc::new(Mutex::new(data)), write_data: Arc::new(Mutex::new(Vec::new())) }
  }

  #[allow(dead_code)]
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
