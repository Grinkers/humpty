//! Humpty is a very fast, robust and flexible HTTP/1.1 web server crate which allows you to develop web applications in Rust. With no dependencies, it is very quick to compile and produces very small binaries, as well as being very resource-efficient.

#![warn(missing_docs)]

pub mod handlers;
pub mod http;
pub mod monitor;
pub mod websocket;

mod app;
mod handler_traits;
mod krauss;
mod percent;
mod route;
pub mod stream;
mod thread;
mod util;

pub use app::App;
pub use route::SubApp;
