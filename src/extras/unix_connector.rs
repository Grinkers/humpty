use crate::extras::connector::{ActiveConnection, ConnWait};
use crate::extras::{Connector, CONNECTOR_SHUTDOWN_TIMEOUT};
use crate::humpty_error::HumptyResult;
use crate::humpty_server::HumptyServer;
use crate::{error_log, info_log, trace_log};
use defer_heavy::defer;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

/// Represents a handle to the simple Unix Socket Server that accepts connections and pumps them into Humpty for handling.
#[derive(Debug)]
pub struct UnixConnector {
  inner: Arc<UnixConnectorInner>,
  main_thread: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Debug)]
struct UnixConnectorInner {
  path: PathBuf,
  listener: UnixListener,
  waiter: ConnWait,
  shutdown_flag: AtomicBool,
  humpty_server: Arc<HumptyServer>,
}

impl UnixConnectorInner {
  #[allow(unsafe_code)]
  fn shutdown(&self) {
    if self.shutdown_flag.swap(true, Ordering::SeqCst) {
      return;
    }

    if self.waiter.is_done(1) {
      //No need to libc::shutdown() if somehow by magic the main_thread is already dead.
      return;
    }

    unsafe {
      if libc::shutdown(self.listener.as_raw_fd(), libc::SHUT_RDWR) != -1 {
        if !self.waiter.wait(1, Some(CONNECTOR_SHUTDOWN_TIMEOUT)) {
          error_log!(
            "unix_connector[{}]: shutdown failed to wake up the listener thread",
            self.path.display()
          );
          return;
        }

        return;
      }

      //This is very unlikely, I have NEVER seen this happen.
      let errno = *libc::__errno_location();
      if !self.waiter.wait(1, Some(CONNECTOR_SHUTDOWN_TIMEOUT)) {
        error_log!("unix_connector[{}]: shutdown failed: errno={}", self.path.display(), errno);
      }
    }
  }
}

impl Connector for UnixConnector {
  fn shutdown(&self) {
    self.inner.shutdown();
  }

  #[cfg(not(unix))]
  pub fn shutdown(&self) {
    self.shutdown_by_connecting();
  }

  fn is_marked_for_shutdown(&self) -> bool {
    self.inner.shutdown_flag.load(Ordering::SeqCst)
  }

  fn is_shutting_down(&self) -> bool {
    self.inner.waiter.is_done(2)
  }

  fn is_shutdown(&self) -> bool {
    self.inner.waiter.is_done(2)
  }

  fn shutdown_and_join(&self, timeout: Option<Duration>) -> bool {
    self.shutdown();
    self.join(timeout)
  }

  fn join(&self, timeout: Option<Duration>) -> bool {
    if !self.inner.waiter.wait(2, timeout) {
      return false;
    }

    let Ok(mut guard) = self.main_thread.lock() else {
      return false;
    };

    let Some(join_handle) = guard.take() else {
      return true;
    };

    match join_handle.join() {
      Ok(_) => (),
      Err(err) => {
        if let Some(msg) = err.downcast_ref::<&'static str>() {
          error_log!(
            "tcp_connector[{}]: listener thread panicked: {}",
            self.inner.path.display(),
            msg
          );
        } else if let Some(msg) = err.downcast_ref::<String>() {
          error_log!(
            "tcp_connector[{}]: listener thread panicked: {}",
            self.inner.path.display(),
            msg
          );
        } else {
          error_log!(
            "tcp_connector[{}]: listener thread panicked: {:?}",
            self.inner.path.display(),
            err
          );
        };
      }
    }

    true
  }
}

impl UnixConnectorInner {
  fn run(&self) {
    defer! {
      self.waiter.signal(2);
    }

    let mut active_connection = Vec::<ActiveConnection>::with_capacity(1024);

    info_log!("unix_connector[{}]: listening...", self.path.display());
    for (stream, this_connection) in self.listener.incoming().zip(1u128..) {
      if self.humpty_server.is_shutdown() || self.shutdown_flag.load(Ordering::SeqCst) {
        info_log!("unix_connector[{}]: shutdown", self.path.display());
        break;
      }

      info_log!("unix_connector[{}]: connection {this_connection} accepted", self.path.display());
      let path_clone = self.path.clone();
      let server_clone = self.humpty_server.clone();

      match thread::Builder::new().spawn(move || {
        match stream {
          Ok(stream) => match server_clone.handle_connection(stream) {
            Ok(_) => {
              info_log!(
                "unix_connector[{}]: connection {this_connection} processed successfully",
                path_clone.display()
              );
            }
            Err(err) => {
              // User code errored, like return Err in an Error handler.
              error_log!(
                "unix_connector[{}]: connection {} humpty server returned err={}",
                path_clone.display(),
                this_connection,
                err
              );
            }
          },
          Err(err) => {
            // This may just affect a single connection and is likely to recover on its own?
            error_log!(
              "unix_connector[{}]: connection {} failed to accept a unix socket connection err={}",
              path_clone.display(),
              this_connection,
              err
            );
          }
        }
      }) {
        Ok(handle) => {
          active_connection.push(ActiveConnection { id: this_connection, hdl: Some(handle) });
        }
        Err(err) => {
          //May recover on its own courtesy of the OS once load decreases.
          error_log!("unix_connector[{}]: connection {} failed to spawn new thread to handle the connection err={}, will drop connection.", self.path.display(), err, this_connection);
        }
      }

      active_connection.retain_mut(|con| {
        match con.hdl.as_ref() {
          Some(hdl) => {
            if !hdl.is_finished() {
              return true;
            }
          }
          None => return false,
        }

        //Code for panic enjoyers
        if let Some(Err(err)) = con.hdl.take().map(JoinHandle::join) {
          let this_connection = con.id;
          if let Some(msg) = err.downcast_ref::<&'static str>() {
            error_log!(
              "unix_connector[{}]: connection {} thread panicked: {}",
              self.path.display(),
              this_connection,
              msg
            );
          } else if let Some(msg) = err.downcast_ref::<String>() {
            error_log!(
              "unix_connector[{}]: connection {} thread panicked: {}",
              self.path.display(),
              this_connection,
              msg
            );
          } else {
            error_log!(
              "unix_connector[{}]: connection {} thread panicked: {:?}",
              self.path.display(),
              this_connection,
              err
            );
          };
        }

        false
      });
    }

    trace_log!("unix_connector[{}]: waiting for shutdown to finish", self.path.display());
    //Wait for all threads to finish
    for mut con in active_connection {
      //Code for panic enjoyers
      if let Some(Err(err)) = con.hdl.take().map(JoinHandle::join) {
        let this_connection = con.id;
        if let Some(msg) = err.downcast_ref::<&'static str>() {
          error_log!(
            "unix_connector[{}]: connection {} thread panicked: {}",
            self.path.display(),
            this_connection,
            msg
          );
        } else if let Some(msg) = err.downcast_ref::<String>() {
          error_log!(
            "unix_connector[{}]: connection {} thread panicked: {}",
            self.path.display(),
            this_connection,
            msg
          );
        } else {
          error_log!(
            "unix_connector[{}]: connection {this_connection} thread panicked: {:?}",
            self.path.display(),
            err
          );
        };
      }
    }

    info_log!("unix_connector[{}]: shutdown done", self.path.display());
  }
}

impl UnixConnector {
  /// Create a new App. Returns an io::Error if it was unable to bind to the socket.
  pub fn start(addr: impl AsRef<Path>, humpty_server: Arc<HumptyServer>) -> HumptyResult<Self> {
    let path = addr.as_ref();
    if std::fs::exists(path)? {
      std::fs::remove_file(path)?;
    }

    let inner = Arc::new(UnixConnectorInner {
      listener: UnixListener::bind(path)?,
      waiter: ConnWait::default(),
      shutdown_flag: AtomicBool::new(false),
      path: path.to_path_buf(),
      humpty_server: humpty_server.clone(),
    });

    let main_thread = {
      let inner = inner.clone();
      thread::Builder::new().spawn(move || {
        inner.run();
      })?
    };

    let connector = Self { inner: inner.clone(), main_thread: Mutex::new(Some(main_thread)) };

    humpty_server.add_shutdown_hook(move || {
      inner.shutdown();
    });

    Ok(connector)
  }
}
