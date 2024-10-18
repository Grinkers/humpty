use std::io;
use std::sync::LockResult;

pub fn unwrap_poison<T>(result: LockResult<T>) -> io::Result<T> {
  result.map_err(|_| io::Error::new(io::ErrorKind::Other, "Poisoned Mutex"))
}
