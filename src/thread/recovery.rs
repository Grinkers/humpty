//! Provides functionality for recovering from thread panics.
use crate::thread::pool::{Message, Thread};

#[cfg(feature = "log")]
use log::trace;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{panicking, spawn, JoinHandle};

/// Marker struct to detect thread panics.
pub struct PanicMarker(pub usize, pub Sender<Option<usize>>);

/// Manages the recovery thread.
pub struct RecoveryThread(pub Option<JoinHandle<()>>);

impl RecoveryThread {
  /// Creates and starts a new recovery thread.
  pub fn new(
    rx: Receiver<Option<usize>>, // None indicates the RecoveryThread should exit its main loop and exit.
    tx: Sender<Option<usize>>,
    task_rx: Arc<Mutex<Receiver<Message>>>,
    threads: Arc<Mutex<Vec<Thread>>>,
  ) -> Self {
    let thread = spawn(move || {
      'outer: loop {
        for panicking_thread in &rx {
          let panicking_thread = if let Some(pt) = panicking_thread {
            pt
          } else {
            break 'outer;
          };

          let mut threads = threads.lock().unwrap();

          // End the OS thread that panicked.
          if let Some(thread) = threads[panicking_thread].os_thread.take() {
            thread.join().ok();
          }

          // Start a new thread with the same ID.
          let restarted_thread = Thread::new(panicking_thread, task_rx.clone(), tx.clone());

          // Put the new thread in the old thread's place.
          threads[panicking_thread] = restarted_thread;

          #[cfg(feature = "log")]
          trace!("Thread {} was restarted", panicking_thread);
        }
      }
    });

    Self(Some(thread))
  }
}

impl Drop for PanicMarker {
  fn drop(&mut self) {
    if panicking() {
      self.1.send(Some(self.0)).ok();
    }
  }
}
