use crate::extras::connector::{ActiveConnection, ConnWait};
use crate::extras::{Connector, ConnectorMeta, CONNECTOR_SHUTDOWN_TIMEOUT};
use crate::functional_traits::ThreadAdapter;
use crate::humpty_builder::{DefaultThreadAdapter, ThreadAdapterJoinHandle};
use crate::humpty_error::HumptyResult;
use crate::humpty_server::HumptyServer;
use crate::{error_log, info_log, trace_log, HumptyTlsStream};
use defer_heavy::defer;
use rustls::{ServerConfig, ServerConnection};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Represents a handle to the simple Tls Unix Socket Server that accepts connections and pumps them into Humpty for handling.
#[derive(Debug)]
pub struct TlsUnixConnector {
  inner: Arc<TlsUnixConnectorInner>,
  main_thread: Mutex<Option<ThreadAdapterJoinHandle>>,
}

#[derive(Debug)]
struct TlsUnixConnectorInner {
  thread_adapter: Arc<dyn ThreadAdapter>,
  config: Arc<ServerConfig>,
  path: PathBuf,
  listener: UnixListener,
  waiter: ConnWait,
  shutdown_flag: AtomicBool,
  humpty_server: Arc<HumptyServer>,
}

impl TlsUnixConnectorInner {
  #[expect(unsafe_code)]
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
            "tls_unix_connector[{}]: shutdown failed to wake up the listener thread",
            self.path.display()
          );
          return;
        }

        return;
      }

      //This is very unlikely, I have NEVER seen this happen.
      let errno = *libc::__errno_location();
      if !self.waiter.wait(1, Some(CONNECTOR_SHUTDOWN_TIMEOUT)) {
        error_log!("tls_unix_connector[{}]: shutdown failed: errno={}", self.path.display(), errno);
      }
    }
  }
}

impl Connector for TlsUnixConnector {
  fn shutdown(&self) {
    self.inner.shutdown();
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
            "tls_unix_connector[{}]: listener thread panicked: {}",
            self.inner.path.display(),
            msg
          );
        } else if let Some(msg) = err.downcast_ref::<String>() {
          error_log!(
            "tls_unix_connector[{}]: listener thread panicked: {}",
            self.inner.path.display(),
            msg
          );
        } else {
          error_log!(
            "tls_unix_connector[{}]: listener thread panicked: {:?}",
            self.inner.path.display(),
            err
          );
        };
      }
    }

    true
  }
}

impl TlsUnixConnectorInner {
  fn run(&self) {
    defer! {
      self.waiter.signal(2);
    }

    let mut active_connection = Vec::<ActiveConnection>::with_capacity(1024);

    info_log!("tls_unix_connector[{}]: listening...", self.path.display());
    for (stream, this_connection) in self.listener.incoming().zip(1u128..) {
      if self.humpty_server.is_shutdown() || self.shutdown_flag.load(Ordering::SeqCst) {
        info_log!("tls_unix_connector[{}]: shutdown", self.path.display());
        break;
      }

      info_log!(
        "tls_unix_connector[{}]: connection {this_connection} accepted",
        self.path.display()
      );
      let path_clone = self.path.clone();
      let server_clone = self.humpty_server.clone();
      let done_flag = Arc::new(AtomicBool::new(false));
      let tls_config = self.config.clone();
      let thread_adapter_clone = self.thread_adapter.clone();

      let done_clone = Arc::clone(&done_flag);
      match self.thread_adapter.spawn(Box::new(move || {
        defer! {
          done_clone.store(true, Ordering::SeqCst);
        }
        match stream {
          Ok(stream) => {
            let tls_stream = match ServerConnection::new(tls_config) {
              Ok(tls_con) => {
                match HumptyTlsStream::create(
                  stream,
                  tls_con,
                  thread_adapter_clone.as_ref(),
                ) {
                  Ok(conn) => conn,
                  Err(err) => {
                    error_log!(
                "tls_unix_connector[{}]: connection {} failed to construct HumptyTlsStream err={}",
                path_clone.display(),
                this_connection,
                err);
                    return;
                  }
                }
              }
              Err(err) => {
                error_log!(
                "tls_unix_connector[{}]: connection {} failed to construct rust-tls ServerConnection err={}",
                path_clone.display(),
                this_connection,
                err);
                return;
              }
            };

            match server_clone.handle_connection_with_meta(tls_stream, ConnectorMeta::TlsUnix) {
              Ok(_) => {
                info_log!(
                "tls_unix_connector[{}]: connection {this_connection} processed successfully",
                path_clone.display()
              );
              }
              Err(err) => {
                // User code errored, like return Err in an Error handler.
                error_log!(
                "tls_unix_connector[{}]: connection {} humpty server returned err={}",
                path_clone.display(),
                this_connection,
                err
              );
              }
            }
          },
          Err(err) => {
            // This may just affect a single connection and is likely to recover on its own?
            error_log!(
              "tls_unix_connector[{}]: connection {} failed to accept a unix socket connection err={}",
              path_clone.display(),
              this_connection,
              err
            );
          }
        }
      })) {
        Ok(handle) => {
          active_connection.push(ActiveConnection {
            id: this_connection,
            hdl: Some(handle),
            done_flag,
          });
        }
        Err(err) => {
          //May recover on its own courtesy of the OS once load decreases.
          error_log!("tls_unix_connector[{}]: connection {} failed to spawn new thread to handle the connection err={}, will drop connection.", self.path.display(), err, this_connection);
        }
      }

      active_connection.retain_mut(|con| {
        if !con.done_flag.load(Ordering::SeqCst) {
          return true;
        }
        if con.hdl.is_none() {
          return false;
        }

        //Code for panic enjoyers
        if let Some(Err(err)) = con.hdl.take().map(ThreadAdapterJoinHandle::join) {
          let this_connection = con.id;
          crate::util::panic_msg(err, |msg| {
            error_log!(
              "tls_unix_connector[{}]: connection {} thread panicked: {}",
              self.path.display(),
              this_connection,
              msg
            );
          });
        }

        false
      });
    }

    trace_log!("tls_unix_connector[{}]: waiting for shutdown to finish", self.path.display());
    //Wait for all threads to finish
    for mut con in active_connection {
      let this_connection = con.id;
      if !con.done_flag.load(Ordering::SeqCst) {
        trace_log!(
          "tls_unix_connector[{}]: connection {} is not yet done. blocking...",
          self.path.display(),
          this_connection
        );
      }

      //Code for panic enjoyers
      if let Some(Err(err)) = con.hdl.take().map(ThreadAdapterJoinHandle::join) {
        crate::util::panic_msg(err, |msg| {
          error_log!(
            "tls_unix_connector[{}]: connection {} thread panicked: {}",
            self.path.display(),
            this_connection,
            msg
          );
        });
      }
    }

    info_log!("tls_unix_connector[{}]: shutdown done", self.path.display());
  }
}

impl TlsUnixConnector {
  /// Create a new UnixConnector.
  /// When this fn returns Ok() the socket is already listening in a background thread.
  /// Returns an io::Error if it was unable to bind to the socket.
  pub fn start(
    addr: impl AsRef<Path>,
    humpty_server: Arc<HumptyServer>,
    config: Arc<ServerConfig>,
    thread_adapter: impl ThreadAdapter + 'static,
  ) -> HumptyResult<Self> {
    //Check if the rust-tls server config is "valid".
    let _ = ServerConnection::new(config.clone())?;

    let path = addr.as_ref();
    if std::fs::exists(path)? {
      std::fs::remove_file(path)?;
    }

    let thread_adapter = Arc::new(thread_adapter);

    let inner = Arc::new(TlsUnixConnectorInner {
      thread_adapter: thread_adapter.clone(),
      listener: UnixListener::bind(path)?,
      waiter: ConnWait::default(),
      shutdown_flag: AtomicBool::new(false),
      path: path.to_path_buf(),
      humpty_server: humpty_server.clone(),
      config,
    });

    let main_thread = {
      let inner = inner.clone();
      thread_adapter.spawn(Box::new(move || {
        inner.run();
      }))?
    };

    let connector = Self { inner: inner.clone(), main_thread: Mutex::new(Some(main_thread)) };

    let weak_inner = Arc::downgrade(&inner);

    humpty_server.add_shutdown_hook(move || {
      if let Some(inner) = weak_inner.upgrade() {
        inner.shutdown()
      }
    });

    Ok(connector)
  }

  /// Create a new UnixConnector.
  /// When this fn returns Ok() the socket is already listening in a background thread.
  /// Returns an io::Error if it was unable to bind to the socket.
  ///
  /// Threads are created using "thread::Builder::new().spawn"
  pub fn start_unpooled(
    addr: impl AsRef<Path>,
    config: Arc<ServerConfig>,
    humpty_server: Arc<HumptyServer>,
  ) -> HumptyResult<Self> {
    Self::start(addr, humpty_server, config, DefaultThreadAdapter)
  }
}
