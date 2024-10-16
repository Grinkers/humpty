//! Provides functionality for handling HTTP requests.

use crate::http::address::Address;
use crate::http::cookie::Cookie;
use crate::http::headers::{HeaderType, Headers};
use crate::http::method::Method;

use std::error::Error;
use std::net::SocketAddr;

use crate::stream::Stream;
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::time::Duration;

/// Represents a request to the server.
/// Contains parsed information about the request's data.
#[derive(Clone, Debug)]
pub struct Request {
  /// The method used in making the request, e.g. "GET".
  pub method: Method,
  /// The URI to which the request was made.
  pub uri: String,
  /// The query string of the request.
  pub query: String,
  /// The HTTP version of the request.
  pub version: String,
  /// A list of headers included in the request.
  pub headers: Headers,
  /// The request body, if supplied.
  pub content: Option<Vec<u8>>,
  /// The address from which the request came
  pub address: Address,
}

/// An error which occurred during the parsing of a request.
#[derive(Debug, PartialEq, Eq)]
pub enum RequestError {
  /// The request could not be parsed due to invalid data.
  Request,
  /// The request could not be parsed due to an issue with the stream.
  Stream,
  /// The request could not be parsed since the client disconnected.
  Disconnected,
  /// The request timed out.
  Timeout,
}

trait OptionToRequestResult<T> {
  fn to_error(self, e: RequestError) -> Result<T, RequestError>;
}

impl<T> OptionToRequestResult<T> for Option<T> {
  fn to_error(self, e: RequestError) -> Result<T, RequestError> {
    self.ok_or(e)
  }
}

impl std::fmt::Display for RequestError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "RequestError")
  }
}

impl Error for RequestError {}

impl Request {
  /// Attempts to read and parse one HTTP request from the given reader.
  pub fn from_stream<T>(stream: &mut T, address: SocketAddr) -> Result<Self, RequestError>
  where
    T: Read,
  {
    let mut first_buf: [u8; 1] = [0; 1];
    stream.read_exact(&mut first_buf).map_err(|_| RequestError::Disconnected)?;

    Self::from_stream_inner(stream, address, first_buf[0])
  }

  /// Attempts to read and parse one HTTP request from the given stream, timing out after the timeout.
  pub fn from_stream_with_timeout(
    stream: &mut Stream,
    address: SocketAddr,
    timeout: Duration,
  ) -> Result<Self, RequestError> {
    stream.set_timeout(Some(timeout)).map_err(|_| RequestError::Stream)?;

    let mut first_buf: [u8; 1] = [0; 1];
    stream.read_exact(&mut first_buf).map_err(|e| match e.kind() {
      ErrorKind::TimedOut => RequestError::Timeout,
      ErrorKind::WouldBlock => RequestError::Timeout,
      _ => RequestError::Disconnected,
    })?;

    stream.set_timeout(None).map_err(|_| RequestError::Stream)?;

    Self::from_stream_inner(stream, address, first_buf[0])
  }

  /// Get the cookies from the request.
  pub fn get_cookies(&self) -> Vec<Cookie> {
    self
      .headers
      .get(HeaderType::Cookie)
      .map(|cookies| {
        cookies
          .split(';')
          .filter_map(|cookie| {
            let (k, v) = cookie.split_once('=')?;
            Some(Cookie::new(k.trim(), v.trim()))
          })
          .collect()
      })
      .unwrap_or_default()
  }

  /// Attempts to get a specific cookie from the request.
  pub fn get_cookie(&self, name: impl AsRef<str>) -> Option<Cookie> {
    self.get_cookies().into_iter().find(|cookie| cookie.name == name.as_ref())
  }

  /// Attempts to read and parse one HTTP request from the given reader.
  fn from_stream_inner<T>(
    stream: &mut T,
    address: SocketAddr,
    first_byte: u8,
  ) -> Result<Self, RequestError>
  where
    T: Read,
  {
    let mut reader = BufReader::new(stream);
    let mut start_line_buf: Vec<u8> = Vec::with_capacity(256);
    reader.read_until(0xA, &mut start_line_buf).map_err(|_| RequestError::Stream)?;

    start_line_buf.insert(0, first_byte);

    let start_line_string =
      std::str::from_utf8(&start_line_buf).map_err(|_| RequestError::Request)?;
    let mut start_line = start_line_string.split(' ');

    let method = Method::from_name(start_line.next().to_error(RequestError::Request)?)?;
    let mut uri_iter = start_line.next().to_error(RequestError::Request)?.splitn(2, '?');
    let version = start_line
      .next()
      .to_error(RequestError::Request)?
      .strip_suffix("\r\n")
      .unwrap_or("")
      .to_string();

    safe_assert(!version.is_empty())?;

    let uri = uri_iter.next().unwrap().to_string();
    let query = uri_iter.next().unwrap_or("").to_string();

    let mut headers = Headers::new();

    loop {
      let mut line_buf: Vec<u8> = Vec::with_capacity(256);
      reader.read_until(0xA, &mut line_buf).map_err(|_| RequestError::Stream)?;
      let line = std::str::from_utf8(&line_buf).map_err(|_| RequestError::Request)?;

      if line == "\r\n" {
        break;
      } else {
        safe_assert(line.len() >= 2)?;
        let line_without_crlf = &line[0..line.len() - 2];
        let mut line_parts = line_without_crlf.splitn(2, ':');
        headers.add(
          HeaderType::from(line_parts.next().to_error(RequestError::Request)?),
          line_parts.next().to_error(RequestError::Request)?.trim_start(),
        );
      }
    }

    let address = Address::from_headers(&headers, address).map_err(|_| RequestError::Request)?;

    if let Some(content_length) = headers.get(&HeaderType::ContentLength) {
      let content_length: usize = content_length.parse().map_err(|_| RequestError::Request)?;
      let mut content_buf: Vec<u8> = vec![0u8; content_length];
      reader.read_exact(&mut content_buf).map_err(|_| RequestError::Stream)?;

      Ok(Self { method, uri, query, version, headers, content: Some(content_buf), address })
    } else {
      Ok(Self { method, uri, query, version, headers, content: None, address })
    }
  }
}

/// Asserts that the condition is true, returning a `Result`.
fn safe_assert(condition: bool) -> Result<(), RequestError> {
  match condition {
    true => Ok(()),
    false => Err(RequestError::Request),
  }
}

impl From<Request> for Vec<u8> {
  fn from(req: Request) -> Self {
    let start_line = if req.query.is_empty() {
      format!("{} {} {}", req.method, req.uri, req.version)
    } else {
      format!("{} {}?{} {}", req.method, req.uri, req.query, req.version)
    };

    let headers = req
      .headers
      .iter()
      .map(|h| format!("{}: {}", h.name, h.value))
      .collect::<Vec<String>>()
      .join("\r\n");

    let mut bytes: Vec<u8> = Vec::new();
    bytes.extend(start_line.as_bytes());
    bytes.extend(b"\r\n");
    bytes.extend(headers.as_bytes());
    bytes.extend(b"\r\n\r\n");

    if let Some(content) = req.content {
      bytes.extend(content);
    }

    bytes
  }
}
