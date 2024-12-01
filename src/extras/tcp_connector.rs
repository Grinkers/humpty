use crate::extras::connector::{ActiveConnection, ConnWait};
use crate::extras::{Connector, CONNECTOR_SHUTDOWN_TIMEOUT};
use crate::functional_traits::{DefaultThreadAdapter, ThreadAdapter, ThreadAdapterJoinHandle};
use crate::humpty_error::HumptyResult;
use crate::humpty_server::HumptyServer;
use crate::{error_log, info_log, trace_log, warn_log};
use defer_heavy::defer;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use std::{io, net, thread};

fn specify_socket_to_loopback(sock: &mut SocketAddr) {
  if sock.ip().is_unspecified() {
    match sock.ip() {
      IpAddr::V4(_) => sock.set_ip(IpAddr::V4(net::Ipv4Addr::new(127, 0, 0, 1))),
      IpAddr::V6(_) => sock.set_ip(IpAddr::V6(net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0x1))),
    };
  }
}

/// Represents a handle to the simple TCP Socket Server that accepts connections and pumps them into Humpty for handling.
#[derive(Debug)]
pub struct TcpConnector {
  main_thread: Mutex<Option<ThreadAdapterJoinHandle>>,
  inner: Arc<TcpConnectorInner>,
}

#[derive(Debug)]
struct TcpConnectorInner {
  addr_string: String,
  addr: Vec<SocketAddr>,
  waiter: ConnWait,
  listener: TcpListener,
  shutdown_flag: AtomicBool,
  humpty_server: Arc<HumptyServer>,
}

impl TcpConnectorInner {
  #[cfg(target_os = "windows")]
  #[allow(unsafe_code)]
  fn next(&self) -> io::Result<TcpStream> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
      WSAGetLastError, WSAPoll, POLLRDNORM, SOCKET_ERROR, WSAPOLLFD,
    };

    let windows_sock_handle = self.listener.as_raw_socket() as usize;

    loop {
      let mut pollfd =
        Box::pin(WSAPOLLFD { fd: windows_sock_handle, events: POLLRDNORM, revents: 0 });

      let result = unsafe {
        //https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-wsapoll
        WSAPoll(pollfd.as_mut().get_mut(), 1, 1000)
      };
      drop(pollfd);
      if self.humpty_server.is_shutdown() || self.shutdown_flag.load(Ordering::SeqCst) {
        return Err(io::ErrorKind::ConnectionAborted.into());
      }

      if result == SOCKET_ERROR {
        unsafe {
          return Err(io::Error::from_raw_os_error(WSAGetLastError()));
        }
      }

      if result == 0 {
        continue;
      }

      return self.listener.accept().map(|(stream, _)| stream);
    }
  }
  #[cfg(not(target_os = "windows"))]
  fn next(&self) -> io::Result<TcpStream> {
    self.listener.accept().map(|(stream, _)| stream)
  }

  fn run(&self) {
    defer! {
      self.waiter.signal(2);
      println!("SHIT IS HAPPENING");
    }
    let mut active_connection = Vec::<ActiveConnection>::with_capacity(1024);

    info_log!("tcp_connector[{}]: listening...", &self.addr_string);
    for this_connection in 1u128.. {
      let stream = self.next();
      if self.humpty_server.is_shutdown() || self.shutdown_flag.load(Ordering::SeqCst) {
        info_log!("tcp_connector[{}]: shutdown", &self.addr_string);
        break;
      }

      info_log!("tcp_connector[{}]: connection {this_connection} accepted", &self.addr_string);
      let path_clone = self.addr_string.clone();
      let server_clone = self.humpty_server.clone();

      match thread::Builder::new().spawn(move || {
        match stream {
          Ok(stream) => match server_clone.handle_connection(stream) {
            Ok(_) => {
              info_log!(
                "tcp_connector[{}]: connection {} processed successfully",
                path_clone,
                this_connection
              );
            }
            Err(err) => {
              // User code errored, like return Err in an Error handler.
              error_log!(
                "tcp_connector[{}]: connection {} humpty server returned err={}",
                path_clone,
                this_connection,
                err
              );
            }
          },
          Err(err) => {
            // This may just affect a single connection and is likely to recover on its own?
            error_log!(
              "tcp_connector[{}]: connection {} failed to accept a unix socket connection err={}",
              path_clone,
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
          error_log!("tcp_connector[{}]: connection {} failed to spawn new thread to handle the connection err={}, will drop connection.", &self.addr_string, this_connection, err);
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
              "tcp_connector[{}]: connection {} thread panicked: {}",
              &self.addr_string,
              this_connection,
              msg
            );
          } else if let Some(msg) = err.downcast_ref::<String>() {
            error_log!(
              "tcp_connector[{}]: connection {} thread panicked: {}",
              &self.addr_string,
              this_connection,
              msg
            );
          } else {
            error_log!(
              "tcp_connector[{}]: connection {} thread panicked: {:?}",
              &self.addr_string,
              this_connection,
              err
            );
          };
        }

        false
      });
    }

    self.waiter.signal(1);

    trace_log!("tcp_connector[{}]: waiting for shutdown to finish", &self.addr_string);
    //Wait for all threads to finish
    for mut con in active_connection {
      //Code for panic enjoyers
      if let Some(Err(err)) = con.hdl.take().map(JoinHandle::join) {
        let this_connection = con.id;
        if let Some(msg) = err.downcast_ref::<&'static str>() {
          error_log!(
            "tcp_connector[{}]: connection {} thread panicked: {}",
            &self.addr_string,
            this_connection,
            msg
          );
        } else if let Some(msg) = err.downcast_ref::<String>() {
          error_log!(
            "tcp_connector[{}]: connection {} thread panicked: {}",
            &self.addr_string,
            this_connection,
            msg
          );
        } else {
          error_log!(
            "tcp_connector[{}]: connection {} thread panicked: {:?}",
            &self.addr_string,
            this_connection,
            err
          );
        };
      }
    }

    info_log!("tcp_connector[{}]: shutdown done", &self.addr_string);
  }
}

impl TcpConnectorInner {
  #[allow(unsafe_code)]
  #[cfg(unix)]
  fn shutdown(&self) {
    use std::os::fd::AsRawFd;

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
            "tcp_connector[{}]: shutdown failed to wake up the listener thread",
            &self.addr_string
          );
          return;
        }

        return;
      }

      //This is very unlikely, I have NEVER seen this happen.
      let errno = *libc::__errno_location();
      if !self.waiter.wait(1, Some(CONNECTOR_SHUTDOWN_TIMEOUT)) {
        error_log!("tcp_connector[{}]: shutdown failed: errno={}", &self.addr_string, errno);
      }
    }
  }

  #[cfg(not(unix))]
  pub fn shutdown(&self) {
    self.shutdown_by_connecting();
  }

  fn shutdown_by_connecting(&self) {
    if self.shutdown_flag.swap(true, Ordering::SeqCst) {
      return;
    }

    if self.waiter.is_done(1) {
      return;
    }

    for addr in self.addr.iter() {
      if self.waiter.is_done(1) {
        return;
      }
      let mut addr = *addr;
      specify_socket_to_loopback(&mut addr);
      if TcpStream::connect_timeout(&addr, CONNECTOR_SHUTDOWN_TIMEOUT).is_ok() {
        info_log!(
          "tcp_connector[{}]: connection to wakeup for shutdown was successful",
          &self.addr_string
        );

        if !self.waiter.wait(1, Some(CONNECTOR_SHUTDOWN_TIMEOUT)) {
          continue;
        }

        return;
      }
    }

    warn_log!(
      "tcp_connector[{}]: connection to wakeup for shutdown was not successful",
      &self.addr_string
    );
  }
}
impl Connector for TcpConnector {
  #[cfg(unix)]
  fn shutdown(&self) {
    self.inner.shutdown();
  }
  #[cfg(not(unix))]
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
            "tcp_connector[{}]: listener thread panicked: {}",
            &self.inner.addr_string,
            msg
          );
        } else if let Some(msg) = err.downcast_ref::<String>() {
          error_log!(
            "tcp_connector[{}]: listener thread panicked: {}",
            &self.inner.addr_string,
            msg
          );
        } else {
          error_log!(
            "tcp_connector[{}]: listener thread panicked: {:?}",
            &self.inner.addr_string,
            err
          );
        };
      }
    }

    true
  }
}

impl TcpConnector {
  /// Creates a new tcp connector that is listening on the given addr.
  /// Return Err on error.
  /// The TCP listener will listen immediately in a background thread.
  pub fn start(
    addr: impl ToSocketAddrs,
    humpty_server: Arc<HumptyServer>,
    thread_adapter: impl ThreadAdapter,
  ) -> HumptyResult<Self> {
    let mut addr_string = String::new();
    let addr_in_vec = addr.to_socket_addrs()?.collect::<Vec<SocketAddr>>();

    for addr in &addr_in_vec {
      if !addr_string.is_empty() {
        addr_string += ", ";
      }
      addr_string += addr.to_string().as_str();
    }

    let inner = Arc::new(TcpConnectorInner {
      listener: TcpListener::bind(addr)?,
      shutdown_flag: AtomicBool::new(false),
      addr_string,
      humpty_server: humpty_server.clone(),
      addr: addr_in_vec,
      waiter: ConnWait::default(),
    });

    let main_thread = {
      let inner = inner.clone();
      thread_adapter.spawn(Box::new(move || {
        inner.run();
      }))?
    };

    let connector = Self { main_thread: Mutex::new(Some(main_thread)), inner: inner.clone() };

    let weak_inner = Arc::downgrade(&inner);

    humpty_server.add_shutdown_hook(move || {
      if let Some(inner) = weak_inner.upgrade() {
        inner.shutdown()
      }
    });

    Ok(connector)
  }

  /// Create a new TcpConnector.
  /// When this fn returns Ok() the socket is already listening in a background thread.
  /// Returns an io::Error if it was unable to bind to the socket.
  ///
  /// Threads are created using "thread::Builder::new().spawn"
  pub fn start_unpooled(
    addr: impl ToSocketAddrs,
    humpty_server: Arc<HumptyServer>,
  ) -> HumptyResult<Self> {
    Self::start(addr, humpty_server, DefaultThreadAdapter)
  }

  /// This fn will set the shutdown flag and then attempt to open a tcp connection to
  /// the listening server (if it is still running) to wake it up.
  ///
  /// This is prone to failure because our lord and savior the Windows firewall may say no to this.
  /// On linux the netfilter kernel module might say no to this, if configured by a paranoid person
  /// (or someone is using RedHat linux).
  ///
  /// Should this be called on a system with a particularly nasty routing table configuration then
  /// the routing table may send the connection attempt into "nirvana". To ensure this doesn't happen
  /// the connection attempt is made with a timeout of 1s. Since any successfully made connection will be to localhost,
  /// this should be fine to handle this. If the routing table does indeed re-route this connection attempt to
  /// somewhere else that's not our server but is something that is running any kind of tcp server then there is no error
  /// to detect and this might cause join() to block until something successfully connects to our actual server.
  ///
  /// This is the best one can do given rusts standard library on windows.
  /// On Windows the shutdown fn will just call this fn, because there is nothing better that can be done.
  /// On Unix the shutdown fn is recommended as it will not establish a tcp connection to wake up the listener.
  ///
  /// Should wake up by establishing a connection fail then any call to join() will not be blocking
  /// and the background "thread" will keep waiting until eventually someone connects to it to wake it up.
  ///
  pub fn shutdown_by_connecting(&self) {
    self.inner.shutdown_by_connecting();
  }
}

#[cfg(target_os = "windows")]
#[test]
pub fn test_windows_ptr_sanity() {
  use std::os::windows::io::RawSocket;
  assert_eq!(size_of::<RawSocket>(), size_of::<usize>());
}