//! Provides asynchronous WebSocket functionality.

use crate::websocket::handler::async_websocket_handler;
use crate::websocket::message::Message;
use crate::websocket::ping::Heartbeat;
use crate::websocket::restion::Restion;
use crate::websocket::stream::WebsocketStream;

use crate::thread::pool::ThreadPool;
use crate::App;

use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{sleep, spawn};
use std::time::{Duration, Instant};

/// Represents an asynchronous WebSocket app.
pub struct AsyncWebsocketApp {
  /// Represents the link to a Humpty application.
  ///
  /// This may be:
  /// - `HumptyLink::Internal`, in which case the app uses its own internal Humpty application
  /// - `HumptyLink::External`, in which case the app is linked to an external Humpty application and receives connections through a channel
  ///
  /// Each enum variant has corresponding fields for the configuration.
  humpty_link: HumptyLink,
  /// The internal thread pool of the application.
  thread_pool: ThreadPool,
  /// The amount of time between polling.
  poll_interval: Option<Duration>,
  /// Ping configuration.
  heartbeat: Option<Heartbeat>,
  /// A hashmap with the addresses as the keys and the actual streams as the values.
  streams: HashMap<String, WebsocketStream>,
  /// A receiver which is sent new streams to add to the hashmap.
  incoming_streams: Receiver<WebsocketStream>,
  /// A receiver which receives messages from handler threads to forward to clients.
  outgoing_messages: Receiver<OutgoingMessage>,
  /// A sender which is used by handler threads to send messages to clients.
  message_sender: Sender<OutgoingMessage>,
  /// The event handler called when a new client connects.
  on_connect: Option<Box<dyn EventHandler>>,
  /// The event handler called when a client disconnects.
  on_disconnect: Option<Box<dyn EventHandler>>,
  /// The event handler called when a client sends a message.
  on_message: Option<Box<dyn MessageHandler>>,
  /// Shutdown signal for the application.
  shutdown: Option<Receiver<()>>,
}

/// Represents an asynchronous WebSocket stream.
///
/// This is what is passed to the handler in place of the actual stream. It is able to send
///   messages back to the stream using the sender and the stream is identified by its address.
pub struct AsyncStream {
  addr: String,
  sender: Sender<OutgoingMessage>,
  connected: bool,
}

/// Represents a global sender which can send messages to clients without waiting for events.
pub struct AsyncSender(Sender<OutgoingMessage>);

/// Represents a message to be sent from the server to a client.
pub enum OutgoingMessage {
  /// A message to be sent to a specific client.
  Message(String, Message),
  /// A message to be sent to every connected client.
  Broadcast(Message),
}

/// Represents the link to a Humpty application.
///
/// This may be:
/// - `HumptyLink::Internal`, in which case the app uses its own internal Humpty application
/// - `HumptyLink::External`, in which case the app is linked to an external Humpty application and receives connections through a channel
///
/// Each enum variant has corresponding fields for the configuration.
pub enum HumptyLink {
  /// The app uses its own internal Humpty application.
  Internal(Box<App>, SocketAddr),
  /// The app is linked to an external Humpty application and receives connections through a channel.
  External(Arc<Mutex<Sender<WebsocketStream>>>),
}

/// Represents a function able to handle a WebSocket event (a connection or disconnection).
/// It is passed the stream which triggered the event.
///
/// ## Example
/// A basic example of an event handler would be as follows:
/// ```
/// fn connection_handler(stream: &humpty::websocket::AsyncStream) {
///     println!("A new client connected! {:?}", stream.peer_addr());
///
///     stream.send(humpty::websocket::Message::new("Hello, World!"));
/// }
/// ```
pub trait EventHandler: Fn(AsyncStream) + Send + Sync + 'static {}
impl<T> EventHandler for T where T: Fn(AsyncStream) + Send + Sync + 'static {}

/// Represents a function able to handle a message event.
/// It is passed the stream which sent the message.
///
/// ## Example
/// A basic example of a message handler would be as follows:
/// ```
/// fn message_handler(stream: &humpty::websocket::AsyncStream, message: humpty::websocket::Message) {
///    println!("A message was received from {:?}: {}", stream.peer_addr(), message.text().unwrap());
///
///    stream.send(humpty::websocket::Message::new("Message received."));
/// }
/// ```
pub trait MessageHandler: Fn(AsyncStream, Message) + Send + Sync + 'static {}
impl<T> MessageHandler for T where T: Fn(AsyncStream, Message) + Send + Sync + 'static {}

impl Default for AsyncWebsocketApp {
  fn default() -> Self {
    let (connect_hook, incoming_streams) = channel();
    let connect_hook = Arc::new(Mutex::new(connect_hook));

    let (message_sender, outgoing_messages) = channel();

    let humpty_app =
      App::new_with_config(1).with_websocket_route("/*", async_websocket_handler(connect_hook));

    Self {
      humpty_link: HumptyLink::Internal(
        Box::new(humpty_app),
        "0.0.0.0:8080".to_socket_addrs().unwrap().next().unwrap(),
      ),

      poll_interval: Some(Duration::from_millis(10)),
      heartbeat: None,
      thread_pool: ThreadPool::new(32),
      streams: Default::default(),
      incoming_streams,
      outgoing_messages,
      message_sender,
      on_connect: None,
      on_disconnect: None,
      on_message: None,
      shutdown: None,
    }
  }
}

impl AsyncWebsocketApp {
  /// Creates a new asynchronous WebSocket app with a custom configuration.
  ///
  /// - `handler_threads`: The size of the handler thread pool.
  /// - `connection_threads`: The size of the connection handler thread pool (the underlying Humpty app).
  pub fn new_with_config(handler_threads: usize, connection_threads: usize) -> Self {
    let (connect_hook, incoming_streams) = channel();
    let connect_hook = Arc::new(Mutex::new(connect_hook));

    let humpty_app = App::new_with_config(connection_threads)
      .with_websocket_route("/*", async_websocket_handler(connect_hook));

    Self {
      humpty_link: HumptyLink::Internal(
        Box::new(humpty_app),
        "0.0.0.0:8080".to_socket_addrs().unwrap().next().unwrap(),
      ),
      thread_pool: ThreadPool::new(handler_threads),
      incoming_streams,
      ..Default::default()
    }
  }

  /// Creates a new asynchronous WebSocket app without creating a Humpty application.
  ///
  /// This is useful if you want to use the app as part of a Humpty application, or if you want to use TLS.
  ///
  /// You'll need to manually link the app to a Humpty application using the `connect_hook`.
  pub fn new_unlinked() -> Self {
    let (connect_hook, incoming_streams) = channel();
    let connect_hook = Arc::new(Mutex::new(connect_hook));

    Self { humpty_link: HumptyLink::External(connect_hook), incoming_streams, ..Default::default() }
  }

  /// Creates a new asynchronous WebSocket app with a custom configuration, without creating a Humpty application.
  ///
  /// This is useful if you want to use the app as part of a Humpty application, or if you want to use TLS.
  ///
  /// You'll need to manually link the app to a Humpty application using the `connect_hook`.
  ///
  /// - `handler_threads`: The size of the handler thread pool.
  pub fn new_unlinked_with_config(handler_threads: usize) -> Self {
    let (connect_hook, incoming_streams) = channel();
    let connect_hook = Arc::new(Mutex::new(connect_hook));

    let (message_sender, outgoing_messages) = channel();

    Self {
      humpty_link: HumptyLink::External(connect_hook),
      thread_pool: ThreadPool::new(handler_threads),
      incoming_streams,
      outgoing_messages,
      message_sender,
      ..Default::default()
    }
  }

  /// Returns a reference to the connection hook of the application.
  /// This is used by Humpty Core to send new streams to the app.
  ///
  /// If the app is uses an internal Humpty application, this will return `None`.
  pub fn connect_hook(&self) -> Option<Arc<Mutex<Sender<WebsocketStream>>>> {
    match &self.humpty_link {
      HumptyLink::External(connect_hook) => Some(connect_hook.clone()),
      _ => None,
    }
  }

  /// Returns a new `AsyncSender`, which can be used to send messages.
  pub fn sender(&self) -> AsyncSender {
    AsyncSender(self.message_sender.clone())
  }

  /// Set the event handler called when a new client connects.
  pub fn on_connect(&mut self, handler: impl EventHandler) {
    self.on_connect = Some(Box::new(handler));
  }

  /// Set the event handler called when a client disconnects.
  pub fn on_disconnect(&mut self, handler: impl EventHandler) {
    self.on_disconnect = Some(Box::new(handler));
  }

  /// Set the message handler called when a client sends a message.
  pub fn on_message(&mut self, handler: impl MessageHandler) {
    self.on_message = Some(Box::new(handler));
  }

  /// Set the event handler called when a new client connects.
  /// Returns itself for use in a builder pattern.
  pub fn with_connect_handler(mut self, handler: impl EventHandler) -> Self {
    self.on_connect(handler);
    self
  }

  /// Set the event handler called when a client disconnects.
  /// Returns itself for use in a builder pattern.
  pub fn with_disconnect_handler(mut self, handler: impl EventHandler) -> Self {
    self.on_disconnect(handler);
    self
  }

  /// Set the message handler called when a client sends a message.
  /// Returns itself for use in a builder pattern.
  pub fn with_message_handler(mut self, handler: impl MessageHandler) -> Self {
    self.on_message(handler);
    self
  }

  /// Set the address to run the application on.
  /// Returns itself for use in a builder pattern.
  ///
  /// This function has no effect if the app does not manage its own internal Humpty application.
  pub fn with_address<T>(mut self, address: T) -> Self
  where
    T: ToSocketAddrs,
  {
    self.humpty_link = match self.humpty_link {
      HumptyLink::Internal(app, _) => {
        let address = address.to_socket_addrs().unwrap().next().unwrap();
        HumptyLink::Internal(app, address)
      }
      HumptyLink::External(connect_hook) => HumptyLink::External(connect_hook),
    };
    self
  }

  /// Sets the polling interval of the async app.
  ///
  /// By default, this is 10ms, meaning the app will check for new events 100 times a second.
  pub fn with_polling_interval(mut self, interval: Option<Duration>) -> Self {
    self.poll_interval = interval;
    self
  }

  /// Sets the heartbeat configuration for the async app.
  ///
  /// By default, this is off, meaning the app will not send heartbeats. If your application needs to detect
  ///   disconnections which occur suddenly, as in without sending a "close" frame, you should set this up.
  ///   It is particularly useful for detecting disconnections caused by network issues, which would not be ordinarily
  ///   detected by the client.
  pub fn with_heartbeat(mut self, heartbeat: Heartbeat) -> Self {
    self.heartbeat = Some(heartbeat);
    self
  }

  /// Start the application on the main thread.
  pub fn run(mut self) {
    // Ensure that the underlying Humpty application is running if it is internal.
    if let HumptyLink::Internal(app, addr) = self.humpty_link {
      spawn(move || app.run(addr).unwrap());
    }

    self.thread_pool.start();

    let connect_handler = self.on_connect.map(Arc::new);
    let disconnect_handler = self.on_disconnect.map(Arc::new);
    let message_handler = self.on_message.map(Arc::new);

    let mut last_ping = Instant::now();

    loop {
      if let Some(ref s) = self.shutdown {
        if s.try_recv().is_ok() {
          break;
        }
      }

      let keys: Vec<String> = self.streams.keys().cloned().collect();

      // Calculate whether a ping should be sent this iteration.
      let will_ping = self
        .heartbeat
        .as_ref()
        .map(|config| {
          let will_ping = last_ping.elapsed() >= config.interval;

          if will_ping {
            last_ping = Instant::now();
          }

          will_ping
        })
        .unwrap_or(false);

      // Check for messages and status on each stream.
      for addr in keys {
        'inner: loop {
          let stream = self.streams.get_mut(&addr).unwrap();

          match stream.recv_nonblocking() {
            Restion::Ok(message) => {
              if let Some(handler) = &message_handler {
                let async_stream = AsyncStream::new(addr.clone(), self.message_sender.clone());

                let cloned_handler = handler.clone();

                self.thread_pool.execute(move || (cloned_handler)(async_stream, message));
              }
            }
            Restion::Err(_) => {
              if let Some(handler) = &disconnect_handler {
                let async_stream =
                  AsyncStream::disconnected(addr.clone(), self.message_sender.clone());

                let cloned_handler = handler.clone();

                self.thread_pool.execute(move || (cloned_handler)(async_stream));
              }

              self.streams.remove(&addr);
              break 'inner;
            }
            Restion::None => break 'inner,
          }
        }

        if let Some(stream) = self.streams.get_mut(&addr) {
          // If the stream has timed out without sending a close frame, process it as a disconnection.
          if let Some(ping) = &self.heartbeat {
            if stream.last_pong.elapsed() >= ping.timeout {
              if let Some(handler) = &disconnect_handler {
                let async_stream =
                  AsyncStream::disconnected(addr.clone(), self.message_sender.clone());

                let cloned_handler = handler.clone();

                self.thread_pool.execute(move || (cloned_handler)(async_stream));
              }

              self.streams.remove(&addr);
              continue;
            }
          }

          // If a ping is due, send one.
          if will_ping {
            stream.ping().ok();
          }
        }
      }

      // Add any streams awaiting connection.
      for (addr, stream) in
        self.incoming_streams.try_iter().filter_map(|s| s.peer_addr().map(|a| (a, s)).ok())
      {
        if let Some(handler) = &connect_handler {
          let async_stream = AsyncStream::new(addr.clone(), self.message_sender.clone());

          let cloned_handler = handler.clone();

          self.thread_pool.execute(move || {
            (cloned_handler)(async_stream);
          });
        }

        self.streams.insert(addr, stream);
      }

      for message in self.outgoing_messages.try_iter() {
        match message {
          OutgoingMessage::Message(addr, message) => {
            if let Some(stream) = self.streams.get_mut(&addr) {
              // Ignore errors with sending for now, and deal with them in the next iteration.
              stream.send(message).ok();
            }
          }
          OutgoingMessage::Broadcast(message) => {
            let frame = message.to_frame();
            for stream in self.streams.values_mut() {
              // Ignore errors with sending for now, and deal with them in the next iteration.
              stream.send_raw(&frame).ok();
            }
          }
        }
      }

      if let Some(interval) = self.poll_interval {
        sleep(interval);
      }
    }
    self.thread_pool.stop();
  }

  /// Registers a shutdown signal to gracefully shutdown the app
  pub fn with_shutdown(mut self, shutdown_receiver: Receiver<()>) -> Self {
    self.shutdown = Some(shutdown_receiver);
    self
  }
}

impl AsyncStream {
  /// Create a new asynchronous stream.
  pub fn new(addr: String, sender: Sender<OutgoingMessage>) -> Self {
    Self { addr, sender, connected: true }
  }

  /// Create a new disconnected asynchronous stream.
  /// This is used for getting the address of a disconnected stream.
  pub fn disconnected(addr: String, sender: Sender<OutgoingMessage>) -> Self {
    Self { addr, sender, connected: false }
  }

  /// Send a message to the client.
  pub fn send(&self, message: Message) {
    assert!(self.connected);
    self.sender.send(OutgoingMessage::Message(self.addr.clone(), message)).ok();
  }

  /// Broadcast a message to all connected clients.
  pub fn broadcast(&self, message: Message) {
    self.sender.send(OutgoingMessage::Broadcast(message)).ok();
  }

  /// Get the address of the stream.
  pub fn peer_addr(&self) -> String {
    self.addr.clone()
  }
}

impl AsyncSender {
  /// Send a message to the client identified by the socket address.
  pub fn send(&self, address: String, message: Message) {
    self.0.send(OutgoingMessage::Message(address, message)).ok();
  }

  /// Broadcast a message to all connected clients.
  pub fn broadcast(&self, message: Message) {
    self.0.send(OutgoingMessage::Broadcast(message)).ok();
  }
}
