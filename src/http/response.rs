//! Provides functionality for handling HTTP responses.

use crate::http::cookie::SetCookie;
use crate::http::headers::{HeaderLike, HeaderType, Headers};
use crate::http::status::StatusCode;

use crate::http::response_body::{ReadAndSeek, ResponseBody};
use crate::stream::ConnectionStreamWrite;
use std::io;
use std::io::Error;
use std::io::ErrorKind;

/// Represents a response from the server.
/// Implements `Into<Vec<u8>>` so can be serialised into bytes to transmit.
///
/// ## Simple Creation
/// ```
/// humpty::http::Response::new(humpty::http::StatusCode::OK, b"Success");
/// ```
///
/// ## Advanced Creation
/// ```
/// humpty::http::Response::empty(humpty::http::StatusCode::OK)
///     .with_body_slice(b"Success")
///     .with_header(humpty::http::headers::HeaderType::ContentType, "text/plain");
/// ```
#[derive(Debug)]
pub struct Response {
  /// The HTTP version of the response.
  pub version: String, //TODO change this to an enum, this can only be Http1.0 or Http1.1 and 99% of time it is http 1.1
  /// The status code of the response, for example 200 OK.
  pub status_code: StatusCode,
  /// A list of the headers included in the response.
  pub headers: Headers,
  /// The body of the response.
  pub body: Option<ResponseBody>,
}

/// An error which occurred during the parsing of a response.
#[derive(Debug, PartialEq, Eq)]
pub enum ResponseError {
  /// The response could not be parsed due to invalid data.
  Response,
  /// The response could not be parsed due to an issue with the stream.
  Stream,
}

impl std::fmt::Display for ResponseError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "ResponseError")
  }
}

impl std::error::Error for ResponseError {}

impl Response {
  /// Creates a new response object with the given status code, bytes and request.
  ///
  /// ## Note about Headers
  /// If you want to add headers to a response, ideally use `Response::empty` and the builder pattern
  ///   so as to not accidentally override important generated headers such as content length and connection.
  pub fn new<T>(status_code: StatusCode, bytes: T) -> Self
  where
    T: AsRef<[u8]>,
  {
    Self {
      version: "HTTP/1.1".to_string(),
      status_code,
      headers: Headers::new(),
      body: Some(ResponseBody::from_slice(&bytes)),
    }
  }

  /// Creates a new response object with the given status code.
  /// Automatically sets the HTTP version to "HTTP/1.1", sets no headers, and creates an empty body.
  pub fn empty(status_code: StatusCode) -> Self {
    Self { version: "HTTP/1.1".to_string(), status_code, headers: Headers::new(), body: None }
  }

  /// Creates a redirect response to the given location.
  pub fn redirect<T>(location: T) -> Self
  where
    T: AsRef<str>,
  {
    Self::empty(StatusCode::MovedPermanently).with_header(HeaderType::Location, location)
  }

  ///Removes the body from the response
  pub fn without_body(mut self) -> Self {
    self.body = None;
    self
  }

  ///Set the body to use with the response
  pub fn with_body(mut self, body: ResponseBody) -> Self {
    self.body = Some(body);
    self
  }

  /// Use the string body as request body
  pub fn with_body_string<T: AsRef<str>>(mut self, body: T) -> Self {
    self.body = Some(ResponseBody::FixedSizeTextData(body.as_ref().to_string()));
    self
  }

  /// Use the binary body as request body
  pub fn with_body_vec(mut self, body: Vec<u8>) -> Self {
    self.body = Some(ResponseBody::from_data(body));
    self
  }

  /// Use the binary body as request body
  pub fn with_body_slice<T: AsRef<[u8]>>(mut self, body: T) -> Self {
    self.body = Some(ResponseBody::from_slice(&body));
    self
  }

  /// Use the file (or something file like) as request body
  /// Note: this call fetches the file size which must not change afterward.
  /// This call uses seek to move the file pointer. Any seeking done prior to this call is ignored.
  /// The actual body will always contain the entire "file"
  pub fn with_body_file<T: ReadAndSeek + 'static>(mut self, body: T) -> io::Result<Self> {
    self.body = Some(ResponseBody::from_file(body)?);
    Ok(self)
  }

  /// Adds the given header to the response.
  /// Returns itself for use in a builder pattern.
  pub fn with_header(mut self, header: impl HeaderLike, value: impl AsRef<str>) -> Self {
    self.headers.add(header, value);
    self
  }

  /// Adds the given cookie to the response in the `Set-Cookie` header.
  /// Returns itself for use in a builder pattern.
  pub fn with_cookie(mut self, cookie: SetCookie) -> Self {
    self.headers.push(cookie.into());
    self
  }

  /// Returns a reference to the response's headers.
  pub fn get_headers(&self) -> &Headers {
    &self.headers
  }

  /// Returns the body as text, if possible.
  pub fn body(&self) -> Option<&ResponseBody> {
    self.body.as_ref()
  }

  ///
  /// Write the request to a streaming output. This consumes the request object.
  ///
  pub fn write_to<T: ConnectionStreamWrite + ?Sized>(mut self, destination: &T) -> io::Result<()> {
    destination.write(self.version.as_bytes())?;
    destination.write(b" ")?;
    destination.write(self.status_code.code_as_utf())?;
    destination.write(b" ")?;
    destination.write(self.status_code.status_line().as_bytes())?;

    for header in self.get_headers().iter() {
      if header.name == HeaderType::ContentLength {
        //TODO we should make it impossible for a response object with this header to be constructed
        return Err(Error::new(
          ErrorKind::Other,
          "Response contains forbidden header Content-Length",
        ));
      }

      if header.name == HeaderType::TransferEncoding {
        //TODO we should make it impossible for a response object with this header to be constructed
        return Err(Error::new(
          ErrorKind::Other,
          "Response contains forbidden header Transfer-Encoding",
        ));
      }

      destination.write(b"\r\n")?;
      //TODO remove this clone
      destination.write(header.name.to_string().as_bytes())?;
      destination.write(b": ")?;
      destination.write(header.value.as_bytes())?;
    }

    if let Some(body) = self.body.as_mut() {
      if body.is_chunked() {
        destination.write(b"\r\nTransfer-Encoding: chunked\r\n\r\n")?;
        body.write_to(destination)?;
        destination.flush()?;
        return Ok(());
      }

      if let Some(len) = body.content_length() {
        destination.write(format!("\r\nContent-Length: {}\r\n\r\n", len).as_bytes())?;
      }

      body.write_to(destination)?;
      destination.flush()?;
      return Ok(());
    }

    destination.write(b"\r\nContent-Length: 0\r\n\r\n")?;
    destination.flush()?;
    Ok(())
  }
}
