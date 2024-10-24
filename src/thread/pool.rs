//! Provides thread pool functionality.
use crate::thread::recovery::{PanicMarker, RecoveryThread};

#[cfg(feature = "log")]
use log::trace;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{Builder, JoinHandle};

/// The number of milliseconds a task can be waiting in the pool before the pool is considered overloaded.
#[cfg(feature = "log")] //This constant is only used in logging for now...
const OVERLOAD_THRESHOLD: u128 = 100;

/// Represents a pool of threads.
pub struct ThreadPool {
  thread_count: usize,
  started: bool,
  threads: Arc<Mutex<Vec<Thread>>>,
  recovery_thread: Option<RecoveryThread>,
  tx: Option<Sender<Message>>,
}

/// Represents a single worker thread in the thread pool
pub struct Thread {
  /// The ID of the thread.
  #[allow(dead_code)]
  pub id: usize,
  /// THe underlying OS thread.
  pub os_thread: Option<JoinHandle<()>>,
}

/// Represents a message between threads.
pub struct Message {
  pub task: Task,

  #[cfg(feature = "log")]
  pub time: std::time::Instant,
}

impl From<Task> for Message {
  #[cfg(feature = "log")]
  fn from(value: Task) -> Self {
    Message { task: value, time: std::time::Instant::now() }
  }
  #[cfg(not(feature = "log"))]
  fn from(value: Task) -> Self {
    Message { task: value }
  }
}

/// Represents a task to run in the thread pool.
pub type Task = Box<dyn FnOnce() + Send + 'static>;

impl ThreadPool {
  /// Creates a new thread pool with the given number of threads.
  ///
  /// ## Panics
  /// This function will panic if the number of threads is zero.
  pub fn new(thread_count: usize) -> Self {
    assert!(thread_count > 0);

    Self {
      thread_count,
      started: false,
      threads: Arc::new(Mutex::new(Vec::new())),
      recovery_thread: None,
      tx: Some(channel().0),
    }
  }

  /// Starts the thread pool.
  pub fn start(&mut self) {
    let (tx, rx): (Sender<Message>, Receiver<Message>) = channel();
    let rx = Arc::new(Mutex::new(rx));
    let mut threads = Vec::with_capacity(self.thread_count);

    let (recovery_tx, recovery_rx): (Sender<Option<usize>>, Receiver<Option<usize>>) = channel();

    for id in 0..self.thread_count {
      threads.push(Thread::new(id, rx.clone(), recovery_tx.clone()))
    }

    self.threads = Arc::new(Mutex::new(threads));
    self.tx = Some(tx);

    let recovery_thread = RecoveryThread::new(recovery_rx, recovery_tx, rx, self.threads.clone());

    self.recovery_thread = Some(recovery_thread);
    self.started = true;
  }

  /// Stops the thread pool.
  pub fn stop(&mut self) {
    self.tx = None;
    if let Some(rt) = self.recovery_thread.take() {
      if rt.0.is_some() {
        let _ = rt.0.unwrap().join();
      }
    }
    self.started = false;
  }

  /// Executes a task in the thread pool.
  ///
  /// ## Panics
  /// This function will panic if the thread pool has not been started.
  pub fn execute<F>(&self, task: F)
  where
    F: FnOnce() + Send + 'static,
  {
    assert!(self.started);

    let boxed_task = Box::new(task) as Task;
    if let Some(tx) = &self.tx {
      tx.send(boxed_task.into()).unwrap();
    };
  }
}

impl Thread {
  /// Creates a new thread.
  pub fn new(
    id: usize,
    rx: Arc<Mutex<Receiver<Message>>>,
    panic_tx: Sender<Option<usize>>,
  ) -> Self {
    let thread = Builder::new()
      .name(format!("{}", id))
      .spawn(move || {
        let panic_marker = PanicMarker(id, panic_tx.clone());

        loop {
          // When the tx pair has been dropped (shutdown initiated), we want to break out.
          let message = match rx.lock() {
            Ok(res) => match res.recv() {
              Ok(res) => res,
              Err(_) => break,
            },
            Err(_) => break,
          };

          #[cfg(feature = "log")]
          if log::Level::Trace <= log::max_level() {
            let time_in_pool = message.time.elapsed().as_millis();

            if time_in_pool > OVERLOAD_THRESHOLD {
              trace!("ThreadPoolOverload");
            }
          }

          (message.task)()
        }

        let _ = panic_tx.send(None); // Shutdown panic_thread
        drop(panic_marker);
      })
      .expect("Thread could not be spawned");

    Self { id, os_thread: Some(thread) }
  }
}

impl Drop for ThreadPool {
  fn drop(&mut self) {
    if let Some(mut recovery_thread) = self.recovery_thread.take() {
      if let Some(thread) = recovery_thread.0.take() {
        thread.join().unwrap();
      }
    }

    for thread in &mut *self.threads.lock().unwrap() {
      if let Some(thread) = thread.os_thread.take() {
        thread.join().unwrap();
      }
    }
  }
}
