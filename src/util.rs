use std::io;
use std::sync::LockResult;

pub fn unwrap_poison<T>(result: LockResult<T>) -> io::Result<T> {
  result.map_err(|_| io::Error::new(io::ErrorKind::Other, "Poisoned Mutex"))
}

pub const fn three_digit_to_utf(num: u16) -> [u8; 3] {
  let n1 = num % 10;
  let n2 = ((num - n1) / 10) % 10;
  let n3 = (((num - n1) - n2) / 100) % 10;
  [b'0' + n3 as u8, b'0' + n2 as u8, b'0' + n1 as u8]
}
