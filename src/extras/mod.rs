pub mod builtin_endpoints;

mod connector;

pub(crate) use connector::CONNECTOR_SHUTDOWN_TIMEOUT;
pub use {connector::Connector, connector::ConnectorMeta};

#[cfg(unix)]
mod unix_connector;

#[cfg(unix)]
pub use unix_connector::*;

mod tcp_connector;
pub use tcp_connector::*;

/// Websocket application that spawns 2 threads per connection.
/// It conveniently handles the WS Heartbeats and broadcasts.
mod websocket_broadcaster;
pub use websocket_broadcaster::*;

#[cfg(feature = "tls")]
mod tls_tcp_connector;

#[cfg(feature = "tls")]
pub use tls_tcp_connector::*;

#[cfg(feature = "tls")]
#[cfg(unix)]
mod tls_unix_connector;

#[cfg(feature = "tls")]
#[cfg(unix)]
pub use tls_unix_connector::*;
