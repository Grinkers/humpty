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

#[cfg(feature = "log")]
#[macro_export]
///Calls trace!
macro_rules! trace_log {
    (target: $target:expr, $($arg:tt)+) => (log::log!(target: $target, log::Level::Trace, $($arg)+));
    ($($arg:tt)+) => (log::log!(log::Level::Trace, $($arg)+))
}

#[cfg(not(feature = "log"))]
#[macro_export]
///Calls trace!
macro_rules! trace_log {

  (target: $target:expr, $($arg:tt)+) => {
      let _ = &($($arg)+);
  };
  ($($arg:tt)+) => {
      let _ = &($($arg)+);
  }
}


#[cfg(feature = "log")]
#[macro_export]
///Calls info!
macro_rules! info_log {
    (target: $target:expr, $($arg:tt)+) => (log::log!(target: $target, log::Level::Info, $($arg)+));
    ($($arg:tt)+) => (log::log!(log::Level::Info, $($arg)+))
}

#[cfg(not(feature = "log"))]
#[macro_export]
///Calls info!
macro_rules! info_log {

  (target: $target:expr, $($arg:tt)+) => {
      let _ = &($($arg)+);
  };
  ($($arg:tt)+) => {
      let _ = &($($arg)+);
  }
}

#[cfg(feature = "log")]
#[macro_export]
///Calls error!
macro_rules! error_log {
    (target: $target:expr, $($arg:tt)+) => (log::log!(target: $target, log::Level::Error, $($arg)+));
    ($($arg:tt)+) => (log::log!(log::Level::Error, $($arg)+))
}

#[cfg(not(feature = "log"))]
#[macro_export]
///Calls error!
macro_rules! error_log {

  (target: $target:expr, $($arg:tt)+) => {
      let _ = &($($arg)+);
  };
  ($($arg:tt)+) => {
      let _ = &($($arg)+);
  }
}