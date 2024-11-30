use colog::format::{CologStyle, DefaultCologStyle};
use humpty::extras;
use humpty::extras::Connector;
use humpty::http::mime::MimeType;
use humpty::http::request_context::RequestContext;
use humpty::http::Response;
use humpty::humpty_builder::HumptyBuilder;
use humpty::humpty_error::HumptyResult;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;
use log::info;

fn hello(_: &RequestContext) -> HumptyResult<Response> {
  Ok(Response::ok("<html><body><h1>Hello</h1></body></html>", MimeType::TextHtml))
}

fn main() -> HumptyResult<()> {
  _ = colog::default_builder()
    .format(|buf, record| {
      let sep = DefaultCologStyle.line_separator();
      let prefix = DefaultCologStyle.prefix_token(&record.level());
      writeln!(
        buf,
        "{} {:?} {}",
        prefix,
        std::thread::current().id(),
        record.args().to_string().replace('\n', &sep),
      )
    })
    .filter_level(log::LevelFilter::Trace)
    .try_init();

  let humpty_server = HumptyBuilder::builder_arc(|builder| {
    builder
      .router(|router| router.route_any("/*", hello))?
      .with_connection_timeout(Some(Duration::from_secs(5)))?
      .ok()
  })?;

  let c1 = extras::TcpConnector::start("0.0.0.0:28080", humpty_server.clone())?;
  let c2 = extras::TcpConnector::start("0.0.0.0:28081", humpty_server.clone())?;
  let c3 = extras::TcpConnector::start("0.0.0.0:28082", humpty_server.clone())?;

  let mut stream =
    TcpStream::connect_timeout(&SocketAddr::from_str("127.0.0.1:28080")?, Duration::from_secs(30))?;
  stream.set_write_timeout(Some(Duration::from_secs(5)))?;
  stream.write_all("GET / HTTP/1.1\r\n\r\n".as_bytes())?;
  stream.flush()?;
  stream.set_read_timeout(Some(Duration::from_secs(5)))?;
  let mut response = Vec::new();
  stream.read_to_end(&mut response)?;
  assert_eq!(std::str::from_utf8(response.as_slice())?, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: Close\r\nContent-Length: 40\r\n\r\n<html><body><h1>Hello</h1></body></html>");

  let mut stream =
      TcpStream::connect_timeout(&SocketAddr::from_str("127.0.0.1:28081")?, Duration::from_secs(30))?;
  stream.set_write_timeout(Some(Duration::from_secs(5)))?;
  stream.write_all("GET / HTTP/1.1\r\n\r\n".as_bytes())?;
  stream.flush()?;
  stream.set_read_timeout(Some(Duration::from_secs(5)))?;
  let mut response = Vec::new();
  stream.read_to_end(&mut response)?;
  assert_eq!(std::str::from_utf8(response.as_slice())?, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: Close\r\nContent-Length: 40\r\n\r\n<html><body><h1>Hello</h1></body></html>");


  let mut stream =
      TcpStream::connect_timeout(&SocketAddr::from_str("127.0.0.1:28082")?, Duration::from_secs(30))?;
  stream.set_write_timeout(Some(Duration::from_secs(5)))?;
  stream.write_all("GET / HTTP/1.1\r\n\r\n".as_bytes())?;
  stream.flush()?;
  stream.set_read_timeout(Some(Duration::from_secs(5)))?;
  let mut response = Vec::new();
  stream.read_to_end(&mut response)?;
  assert_eq!(std::str::from_utf8(response.as_slice())?, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: Close\r\nContent-Length: 40\r\n\r\n<html><body><h1>Hello</h1></body></html>");

  sleep(Duration::from_secs(5));

  humpty_server.shutdown();
  // Shutting down the humpty server should also shut down all connectors.
  assert!(c1.is_marked_for_shutdown());
  assert!(c2.is_marked_for_shutdown());
  assert!(c3.is_marked_for_shutdown());

  // We should be able to join them...
  c1.join(None);
  c2.join(None);
  c3.join(None);


  // With the connector having finished shutdown(), the sockets can be rebound immediately.
  let _listen1 = TcpListener::bind("0.0.0.0:28080")?;
  let _listen2 = TcpListener::bind("0.0.0.0:28081")?;
  let _listen3 = TcpListener::bind("0.0.0.0:28082")?;

  info!("DONE!");
  Ok(())
}

#[test]
fn run() {
  main().expect("ERROR");
}
