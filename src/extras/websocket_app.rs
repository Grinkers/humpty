use std::sync::mpsc::{self, channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self};
use std::time::Duration;

use crate::http::request_context::RequestContext;
use crate::websocket::message::WebsocketMessage;
use crate::websocket::stream::{ReadMessageTimeoutResult, WebsocketReceiver, WebsocketSender};
use crate::{error_log, info_log, warn_log};

type WebsocketContext = (WebsocketReceiver, WebsocketSender, String);

/// Provides WebSocket handshake functionality.
/// New connections will be sent to the App
///
pub fn websocket_app_handler(
  hook: Arc<Mutex<Sender<WebsocketContext>>>,
) -> impl Fn(&RequestContext, WebsocketReceiver, WebsocketSender) {
  move |request: &RequestContext, receiver: WebsocketReceiver, sender: WebsocketSender| {
    hook.lock().unwrap().send((receiver, sender, request.peer_address().to_string())).ok();
  }
}
#[derive(Debug)]
#[non_exhaustive]
/// Top level Error for why `run` ran into a fatal crash.
/// Use the `log` feature for debugging/tracing errors (eg. a WS client disconnecting).
pub enum AppError {
  /// Broadcast thread panic
  BroadcastThread,
  /// Exec thread panic
  ExecThread,
}

/// WebSocketApp builder/linker for setup and linking to Humpty
pub struct Linker {
  humpty_link: Arc<Mutex<Sender<WebsocketContext>>>,
  state: State,
}

/// Represents a WebSocket app.
pub struct App {
  state: State,
}

// internal app structure
struct State {
  // A receiver to receive new WebsocketStreams from HumptyServer.
  incoming_streams: Receiver<WebsocketContext>,

  heartbeat: Option<Duration>,

  // A Vec of all the streams for broadcasting.
  send_streams: Arc<Mutex<Vec<Sender<OutgoingMessage>>>>,
  // A sender which is used by handler threads to send messages to clients.
  broadcast_sender: Sender<WebsocketMessage>,
  // A receiver which receives messages from handler threads to forward to clients.
  outgoing_broadcasts: Receiver<WebsocketMessage>,

  // The event handler called when a new client connects.
  connect_handler: Option<Box<dyn EventHandler>>,
  // The event handler called when a client disconnects.
  disconnect_handler: Option<Box<dyn EventHandler>>,
  // The event handler called when a client sends a message.
  message_handler: Option<Box<dyn MessageHandler>>,

  // Shutdown signal for the application.
  shutdown: Option<Receiver<()>>,
}

/// Represents a WebSocketApp handle.
///
/// This is what is passed to the handler in place of the actual stream. It is able to send
/// messages back to the stream or broadcast to all streams within the WebSocketApp.
pub struct Handle {
  addr: String,
  sender: Sender<OutgoingMessage>,
}

/// Represents a global sender which can be used to broadcast messages to all clients.
pub struct BroadcastSender(Sender<WebsocketMessage>);

impl BroadcastSender {
  /// Broadcast a message to all connected clients.
  pub fn broadcast(&self, message: WebsocketMessage) {
    self.0.send(message).ok();
  }
}

/// Represents a message to be sent from the server (humpty) to client(s).
pub enum OutgoingMessage {
  /// A message to be sent to a specific client.
  Message(WebsocketMessage),
  /// A message to be sent to every connected client.
  Broadcast(WebsocketMessage),
}

/// Represents a function able to handle a WebSocket event (a connection or disconnection).
/// It is passed the stream which triggered the event.
///
/// ## Example
/// A basic example of an event handler would be as follows:
/// ```
/// fn connection_handler(stream: &humpty::extras::websocket_app::Handle) {
///     println!("A new client connected! {:?}", stream.peer_addr());
///
///     stream.send(humpty::websocket::message::WebsocketMessage::new_text("Hello, World!"));
/// }
/// ```
pub trait EventHandler: Fn(Handle) + Send + Sync + 'static {}
impl<T> EventHandler for T where T: Fn(Handle) + Send + Sync + 'static {}

/// Represents a function able to handle a message event.
/// It is passed the stream which sent the message.
///
/// ## Example
/// A basic example of a message handler would be as follows:
/// ```
/// use humpty::websocket::message::WebsocketMessage;
/// use humpty::extras::websocket_app::Handle;
/// fn message_handler(handle: Handle, message: WebsocketMessage) {
///    println!("{:?}", message);
///
///    handle.send(WebsocketMessage::new_text("Message received."));
/// }
/// ```
pub trait MessageHandler: Fn(Handle, WebsocketMessage) + Send + Sync + 'static {}
impl<T> MessageHandler for T where T: Fn(Handle, WebsocketMessage) + Send + Sync + 'static {}

impl Default for Linker {
  fn default() -> Self {
    let (connect_hook, incoming_streams) = channel();
    let (broadcast_sender, outgoing_broadcasts) = channel();

    Self {
      humpty_link: Arc::new(Mutex::new(connect_hook)),
      state: State {
        heartbeat: Some(Duration::from_secs(5)),
        send_streams: Default::default(),
        outgoing_broadcasts,
        broadcast_sender,
        incoming_streams,
        connect_handler: None,
        disconnect_handler: None,
        message_handler: None,
        shutdown: None,
      },
    }
  }
}

impl Linker {
  /// Returns the finalized App.
  pub fn finalize(self) -> App {
    App { state: self.state }
  }

  /// Returns a reference to the connection hook of the application.
  /// This is used by the HumptyServer to send new streams to the app.
  pub fn connect_hook(&self) -> Arc<Mutex<Sender<WebsocketContext>>> {
    self.humpty_link.clone()
  }

  /// Returns a new `BroadcastSender`, which can be used to send messages.
  pub fn sender(&self) -> BroadcastSender {
    BroadcastSender(self.state.broadcast_sender.clone())
  }

  /// Set the event handler called when a new client connects.
  pub fn with_connect_handler(mut self, handler: impl EventHandler) -> Self {
    self.state.connect_handler = Some(Box::new(handler));
    self
  }

  /// Set the event handler called when a client disconnects.
  pub fn with_disconnect_handler(mut self, handler: impl EventHandler) -> Self {
    self.state.disconnect_handler = Some(Box::new(handler));
    self
  }

  /// Set the message handler called when a client sends a message.
  pub fn with_message_handler(mut self, handler: impl MessageHandler) -> Self {
    self.state.message_handler = Some(Box::new(handler));
    self
  }

  /// Sets the heartbeat configuration for the app.
  ///
  /// By default, this is 5 seconds.
  /// It is highly recommended to set this reasonably shorter than your `with_connection_timeout`.
  pub fn with_heartbeat(mut self, heartbeat: Duration) -> Self {
    self.state.heartbeat = Some(heartbeat);
    self
  }

  /// Registers a shutdown signal to gracefully shutdown the app
  ///
  /// For a full/consistent shutdown, you must set both
  ///`HumptyBuilder::with_connection_timeout` and `with_heartbeat`
  ///
  /// Threads are fully joined, but won't exit until timeouts/heartbeats.
  pub fn with_shutdown(mut self, shutdown_receiver: Receiver<()>) -> Self {
    self.state.shutdown = Some(shutdown_receiver);
    self
  }
}

impl App {
  /// Start the application on the main thread.
  pub fn run(self) -> Result<(), AppError> {
    let connect_handler = self.state.connect_handler.map(Arc::new);
    let disconnect_handler = self.state.disconnect_handler.map(Arc::new);
    let message_handler = self.state.message_handler.map(Arc::new);
    let streams = self.state.send_streams.clone();

    let timeout = {
      if let Some(hb) = self.state.heartbeat {
        hb
      } else {
        Duration::MAX
      }
    };

    let (shutdown_tx, shutdown_rx) = mpsc::sync_channel(1);
    // broadcast/heartbeat thread
    let broadcast_thread = thread::spawn(move || loop {
      if shutdown_rx.try_recv().is_ok() {
        info_log!("shutdown received in WebSocketApp's broadcast thread");
        break;
      }
      let recv = self.state.outgoing_broadcasts.recv_timeout(timeout);

      // Remove up to one idx per broadcast. They should eventually all be cleaned up because of the heartbeat.
      let mut remove_idx = None;
      match recv {
        Ok(message) => {
          let mut streams = streams.lock().expect("todo");
          for (idx, stream) in streams.iter_mut().enumerate() {
            // convert the broadcast back to message, but for each sender
            if stream.send(OutgoingMessage::Message(message.clone())).is_err() {
              remove_idx = Some(idx);
            }
          }
        }
        // the WebSocketApp has closed
        Err(mpsc::RecvTimeoutError::Disconnected) => break,
        Err(mpsc::RecvTimeoutError::Timeout) => {}
      }
      if let Some(idx) = remove_idx {
        let mut streams = streams.lock().expect("todo");
        if streams.len() > idx {
          streams.remove(idx);
        }
      }
    });

    let exec_thread = thread::spawn(move || {
      let mut threads = Vec::new();
      loop {
        if let Some(sd) = &self.state.shutdown {
          if sd.try_recv().is_ok() {
            info_log!("shutdown received in WebSocketApp");
            break;
          }
        }

        // The HumptyServer has exit, so we have no more work to do
        let recv = self.state.incoming_streams.recv_timeout(timeout);
        let new_stream = match recv {
          Ok(ns) => ns,
          Err(RecvTimeoutError::Timeout) => continue,
          Err(RecvTimeoutError::Disconnected) => break,
        };

        let sender = self.state.broadcast_sender.clone();
        let (message_sender, outgoing_messages) = channel();
        self.state.send_streams.lock().unwrap().push(message_sender.clone());

        let connect_handler = connect_handler.clone();
        let disconnect_handler = disconnect_handler.clone();
        let message_handler = message_handler.clone();

        threads.push(thread::spawn(move || {
          exec(
            new_stream,
            sender,
            message_sender,
            outgoing_messages,
            connect_handler,
            disconnect_handler,
            message_handler,
            timeout,
          );
        }));

        // Iterate over the threads and remove the finished ones
        threads.retain(|handle| !handle.is_finished());
      }

      // have broadcast thread end, if the exec thread ever exits
      let broadcast_shutdown = shutdown_tx.send(());
      if let Err(e) = broadcast_shutdown {
        error_log!("Broadcast Thread unexpectedly halted before signal with {}", e);
        return Err(AppError::BroadcastThread);
      }

      for t in threads {
        let j = t.join();
        if let Err(e) = j {
          warn_log!("{:?} while doing join of `exec` thread.", e);
        }
      }
      Ok(())
    });

    // todo monitor for either thread for panics
    let etj = exec_thread.join();
    match etj {
      Ok(_) => (),
      Err(e) => {
        warn_log!("{:?} while doing join of `exec` thread.", e);
        return Err(AppError::ExecThread);
      }
    };

    let btj = broadcast_thread.join();
    if let Err(e) = btj {
      warn_log!("{:?} while doing join of `exec` thread.", e);
      return Err(AppError::BroadcastThread);
    }
    Ok(())
  }
}

impl Handle {
  /// Create a new handle.
  pub fn new(addr: String, sender: Sender<OutgoingMessage>) -> Self {
    Self { addr, sender }
  }

  /// Send a message to the client.
  pub fn send(&self, message: WebsocketMessage) {
    self.sender.send(OutgoingMessage::Message(message)).ok();
  }

  /// Broadcast a message to all connected clients.
  pub fn broadcast(&self, message: WebsocketMessage) {
    self.sender.send(OutgoingMessage::Broadcast(message)).ok();
  }

  /// Get the address of the stream.
  pub fn peer_addr(&self) -> String {
    self.addr.clone()
  }
}

// todo just while figuring out the structure
#[allow(clippy::too_many_arguments)]
fn exec(
  stream: WebsocketContext,
  broadcast: Sender<WebsocketMessage>,
  message_sender: Sender<OutgoingMessage>,
  outgoing_messages: Receiver<OutgoingMessage>,
  connect_handler: Option<Arc<Box<dyn EventHandler>>>,
  disconnect_handler: Option<Arc<Box<dyn EventHandler>>>,
  message_handler: Option<Arc<Box<dyn MessageHandler>>>,
  timeout: Duration,
) {
  let (mut ws_receiver, ws_sender, addr) = (stream.0, stream.1, stream.2);

  if let Some(ch) = connect_handler {
    let handle = Handle::new(addr.clone(), message_sender.clone());
    (ch)(handle);
  }

  // read thread
  let rt = if let Some(mh) = message_handler {
    let rt = thread::spawn(move || loop {
      match ws_receiver.read_message_timeout(Some(timeout)) {
        Ok(message) => match message {
          ReadMessageTimeoutResult::Message(m) => {
            match m {
              WebsocketMessage::Binary(_) | WebsocketMessage::Text(_) => {
                (mh)(Handle::new(addr.clone(), message_sender.clone()), m)
              }
              WebsocketMessage::Ping => {
                if message_sender.send(OutgoingMessage::Message(WebsocketMessage::Pong)).is_err() {
                  break;
                }
              }
              WebsocketMessage::Pong => (), // do nothing
            }
          }
          ReadMessageTimeoutResult::Timeout | ReadMessageTimeoutResult::Closed => break,
        },
        Err(e) => {
          error_log!("ws_app read: {:?} occurred", &e);
          if let Some(dh) = disconnect_handler {
            (dh)(Handle::new(addr.clone(), message_sender.clone()));
          }
          break;
        }
      }
    });
    Some(rt)
  } else {
    None
  };

  // write thread
  loop {
    match outgoing_messages.recv_timeout(timeout) {
      Ok(m) => match m {
        OutgoingMessage::Message(message) => {
          if ws_sender.send(message).is_err() {
            break;
          }
        }
        OutgoingMessage::Broadcast(message) => {
          if broadcast.send(message).is_err() {
            break;
          }
        }
      },
      Err(mpsc::RecvTimeoutError::Disconnected) => break,
      Err(mpsc::RecvTimeoutError::Timeout) => {
        if ws_sender.ping().is_err() {
          break;
        }
      }
    }
  }

  info_log!("ws_app: exiting exec's write thread");
  if let Some(rt) = rt {
    rt.join().unwrap();
    info_log!("ws_app: exiting exec's read thread");
  }
}