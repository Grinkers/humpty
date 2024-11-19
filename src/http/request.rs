//! Provides functionality for handling HTTP requests.

use crate::http::cookie::Cookie;
use crate::http::headers::{Header, HeaderLike, HeaderName, Headers};
use crate::http::method::Method;

use crate::http::mime::{AcceptMime, MimeType, QValue};
use crate::humpty_error::{HumptyError, HumptyResult, RequestHeadParsingError, UserError};
use crate::stream::ConnectionStream;
use crate::util::unwrap_some;
use crate::warn_log;
use std::fmt::{Display, Formatter};

/// Enum for http versions humpty supports.
#[derive(Clone, Debug, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
#[non_exhaustive] //Not sure but I don't want to close the door on http 2 shut!
pub enum HttpVersion {
  /// Earliest http version. Has no concept of request bodies or headers. to trigger a request run `echo -ne 'GET /path/goes/here\r\n' | nc 127.0.0.1 8080`
  /// Responses are just the body, no headers, no nothing.
  Http09,
  /// First actually usable http version. Has headers, bodies, etc. but notably 1 connection per request and thus no transfer encoding
  Http10,
  /// Most recent 1.X version, has all features.
  Http11,
}

impl HttpVersion {
  /// returns the printable name of the http version.
  /// This is not always equivalent to how its appears in binary on the status line.
  pub fn as_str(&self) -> &'static str {
    match self {
      HttpVersion::Http09 => "HTTP/0.9",
      HttpVersion::Http10 => "HTTP/1.0",
      HttpVersion::Http11 => "HTTP/1.1",
    }
  }
  /// returns the network bytes in the status line for the http version.
  pub fn as_net_str(&self) -> &'static str {
    match self {
      HttpVersion::Http09 => "",
      HttpVersion::Http10 => "HTTP/1.0",
      HttpVersion::Http11 => "HTTP/1.1",
    }
  }
}

impl Display for HttpVersion {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    match self {
      HttpVersion::Http09 => f.write_str("HTTP/0.9"),
      HttpVersion::Http10 => f.write_str("HTTP/1.0"),
      HttpVersion::Http11 => f.write_str("HTTP/1.1"),
    }
  }
}

impl HttpVersion {
  /// Tries to parse the http version part of the status line to a http version.
  /// empty string is treated as http09 because http09 doesn't have a version in its status line.
  /// Returns input on error.
  pub fn try_from_net_str<T: AsRef<str>>(value: T) -> Result<Self, T> {
    match value.as_ref() {
      "HTTP/1.0" => Ok(HttpVersion::Http10),
      "HTTP/1.1" => Ok(HttpVersion::Http11),
      "" => Ok(HttpVersion::Http09),
      _ => Err(value),
    }
  }

  /// Tries to parse the http version from the printable name. This was most likely returned by a call to `HttpVersion::as_str`
  pub fn try_from_str<T: AsRef<str>>(value: T) -> Result<Self, T> {
    match value.as_ref() {
      "HTTP/1.0" => Ok(HttpVersion::Http10),
      "HTTP/1.1" => Ok(HttpVersion::Http11),
      "HTTP/0.9" => Ok(HttpVersion::Http09),
      _ => Err(value),
    }
  }
}

/// Represents a request to the server.
/// Contains parsed information about the request's data.
//TODO make it harder/impossible to put this struct into an illegal state.
#[derive(Clone, Debug)]
pub struct RequestHead {
  /// The method used in making the request, e.g. "GET".
  method: Method,

  /// The HTTP version of the request.
  version: HttpVersion,

  /// The status line as is.
  /// For example "GET /index.html HTTP1.1"
  /// the crlf has been stripped already!
  status_line: String,

  /// The path to which the request was made.
  path: String,

  /// The raw query string of the request.
  query: String,

  accept: Vec<AcceptMime>,

  // Vec of query parameters, key=value in order of appearance.
  //TODO implement this
  //pub query_params: Vec<(String, String)>,
  /// A list of headers included in the request.
  headers: Headers,
}

fn parse_status_line(start_line_buf: &Vec<u8>) -> HumptyResult<&str> {
  // TODO this must be US-ASCII not utf-8!
  for n in start_line_buf {
    // https://en.wikipedia.org/wiki/Percent-encoding#Types_of_URI_characters
    // plus space char which we check later...
    match *n {
      //RFC 3986 section 2.2 Reserved Characters
      // TODO some of these chars are not valid for the status line... the status line is not the URI!
      b'!' => {}
      b'#' => {}
      b'$' => {}
      b'&' => {}
      b'\'' => {}
      b'(' => {}
      b')' => {}
      b'*' => {}
      b'+' => {}
      b',' => {}
      b'/' => {}
      b':' => {}
      b';' => {}
      b'=' => {}
      b'?' => {}
      b'@' => {}
      b'[' => {}
      b']' => {}
      // RFC 3986 section 2.3 Unreserved Characters
      b'-' => {}
      b'.' => {}
      b'_' => {}
      b'~' => {}
      //Other Stuff
      b'%' => {}
      b' ' => {}
      b'\r' => {} // TODO we should check this later... this is only allowed as the second to last char...
      b'\n' => {} // TODO we should check this later... this is only allowed as the last char...
      other => {
        if other.is_ascii_alphanumeric() {
          continue;
        }
        return Err(RequestHeadParsingError::StatusLineContainsInvalidBytes.into());
      }
    }
  }

  // We could use the unsafe variant here without issue to prevent double validation our validation is stricter than str validation.
  Ok(
    std::str::from_utf8(start_line_buf)
      .map_err(|_| RequestHeadParsingError::StatusLineContainsInvalidBytes)?,
  )
}

impl RequestHead {
  /// Attempts to read and parse one HTTP request from the given reader.
  pub fn new(stream: &dyn ConnectionStream) -> HumptyResult<Self> {
    let mut start_line_buf: Vec<u8> = Vec::with_capacity(256);
    //TODO fix ddos potential here, limit read to 64k or some other reasonable size.
    //Possible attack on this is to just write ~Mem amount of data and then just keep
    //drip feeding us 1 byte of data every so often to deny memory to actual requests.
    let count = stream.read_until(0xA, &mut start_line_buf)?;

    if count == 0 {
      return Err(RequestHeadParsingError::EofBeforeReadingAnyBytes.into());
    }

    let start_line_string = parse_status_line(&start_line_buf)?;

    let status_line =
      start_line_string.strip_suffix("\r\n").ok_or(RequestHeadParsingError::StatusLineNoCRLF)?;

    let mut start_line = status_line.split(' ');

    let method = Method::from(unwrap_some(start_line.next()));

    let mut uri_iter =
      start_line.next().ok_or(RequestHeadParsingError::StatusLineNoWhitespace)?.splitn(2, '?');

    let version = start_line
      .next()
      .map(HttpVersion::try_from_net_str)
      .unwrap_or(Ok(HttpVersion::Http09)) //Http 0.9 has no suffix
      .map_err(|version| RequestHeadParsingError::HttpVersionNotSupported(version.to_string()))?;

    if start_line.next().is_some() {
      return Err(HumptyError::from(RequestHeadParsingError::StatusLineTooManyWhitespaces));
    }

    let raw_path = uri_iter.next().unwrap();

    let path = urlencoding::decode(raw_path)
      .map_err(|_| {
        HumptyError::from(RequestHeadParsingError::PathInvalidUrlEncoding(raw_path.to_string()))
      })?
      .to_string();

    let query = uri_iter.next().unwrap_or("").to_string();

    let mut headers = Headers::new();

    if version == HttpVersion::Http09 {
      if method != Method::Get {
        return Err(HumptyError::from(RequestHeadParsingError::MethodNotSupportedByHttpVersion(
          version, method,
        )));
      }

      return Ok(Self {
        method,
        path,
        query,
        version,
        headers,
        accept: vec![AcceptMime::from_mime(MimeType::TextHtml, QValue::default())], // Http 0.9 only accepts html.
        status_line: status_line.to_string(),
      });
    }

    loop {
      let mut line_buf: Vec<u8> = Vec::with_capacity(256);
      stream.read_until(0xA, &mut line_buf)?;
      let line = std::str::from_utf8(&line_buf)
        .map_err(|_| RequestHeadParsingError::HeaderLineIsNotUsAscii)?;

      if line == "\r\n" {
        break;
      }

      let line = line.strip_suffix("\r\n").ok_or(RequestHeadParsingError::HeaderLineNoCRLF)?;

      let mut line_parts = line.splitn(2, ": ");
      let name = unwrap_some(line_parts.next()).trim();

      if name.is_empty() {
        return Err(HumptyError::from(RequestHeadParsingError::HeaderNameEmpty));
      }

      let value = line_parts.next().ok_or(RequestHeadParsingError::HeaderValueMissing)?.trim();

      if value.is_empty() {
        return Err(HumptyError::from(RequestHeadParsingError::HeaderValueEmpty));
      }

      headers.add(HeaderName::from(name), value);
    }

    let accept_hdr = headers.get(HeaderName::Accept).unwrap_or("*/*"); //TODO This is probably also wrong.
    let accept = AcceptMime::parse(accept_hdr);
    if accept.is_none() {
      // TODO should this be a hard error?
      warn_log!(
        "Request to '{}' has invalid Accept header '{}' will assume 'Accept: */*'",
        path.as_str(),
        accept_hdr
      );
    }

    let accept = accept.unwrap_or_else(|| vec![AcceptMime::default()]);

    Ok(Self { method, path, query, version, headers, accept, status_line: status_line.to_string() })
  }

  /// get the http version this request was made in by the client.
  pub fn version(&self) -> HttpVersion {
    self.version
  }

  /// Returns the raw status line.
  pub fn raw_status_line(&self) -> &str {
    self.status_line.as_str()
  }

  /// Returns the path the request will be routed to
  /// This should not contain any url encoding.
  pub fn path(&self) -> &str {
    self.path.as_str()
  }

  /// Sets the path the request will be routed to.
  /// This should not contain any url encoding.
  pub fn set_path(&mut self, path: impl ToString) {
    self.path = path.to_string();
  }

  /// Gets the raw query string.
  #[deprecated] // Will be replaced by parsed query parameters.
  pub fn raw_query(&self) -> &str {
    self.query.as_str()
  }

  /// gets the method of the request.
  pub fn method(&self) -> &Method {
    &self.method
  }

  /// Changes the method of the request
  pub fn set_method(&mut self, method: Method) {
    self.method = method;
  }

  /// Get the cookies from the request.
  pub fn get_cookies(&self) -> Vec<Cookie> {
    self
      .headers
      .get(HeaderName::Cookie)
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

  /// Manipulates the accept header values.
  /// This also overwrites the actual accept header!
  pub fn set_accept(&mut self, types: Vec<AcceptMime>) {
    let hdr_value = AcceptMime::elements_to_header_value(&types);
    self.headers.set(HeaderName::Accept, hdr_value);
    self.accept = types;
  }

  /// Returns the acceptable mime types
  pub fn get_accept(&self) -> &[AcceptMime] {
    self.accept.as_slice()
  }

  /// Returns an iterator over all headers.
  pub fn get_all_headers(&self) -> impl Iterator<Item = &Header> {
    self.headers.iter()
  }

  /// Returns the first header or None
  pub fn get_header(&self, name: impl HeaderLike) -> Option<&str> {
    self.headers.get(name)
  }

  /// Returns the all header values of empty Vec.
  pub fn get_headers(&self, name: impl HeaderLike) -> Vec<&str> {
    self.headers.get_all(name)
  }

  /// Removes all instances of a particular header.
  pub fn remove_header(&mut self, name: impl HeaderLike) -> HumptyResult<()> {
    let hdr = name.to_header();
    match &hdr {
      HeaderName::Accept => {
        self.accept = vec![AcceptMime::default()];
        self.headers.set(hdr, "*/*");
        Ok(())
      }
      HeaderName::TransferEncoding => {
        UserError::ImmutableRequestHeaderRemoved(HeaderName::TransferEncoding).into()
      }
      HeaderName::ContentLength => {
        UserError::ImmutableRequestHeaderRemoved(HeaderName::ContentLength).into()
      }
      _ => {
        self.headers.remove(hdr);
        Ok(())
      }
    }
  }

  /// Sets the header value.
  /// Some header values cannot be modified through this fn and attempting to change them are a noop.
  pub fn set_header(&mut self, name: impl HeaderLike, value: impl AsRef<str>) -> HumptyResult<()> {
    let hdr = name.to_header();
    let hdr_value = value.as_ref();
    match &hdr {
      HeaderName::Accept => {
        if let Some(accept) = AcceptMime::parse(hdr_value) {
          self.accept = accept;
          self.headers.set(hdr, value);
          return Ok(());
        }

        UserError::IllegalAcceptHeaderValueSet(hdr_value.to_string()).into()
      }
      HeaderName::TransferEncoding => UserError::ImmutableRequestHeaderModified(
        HeaderName::TransferEncoding,
        hdr_value.to_string(),
      )
      .into(),
      HeaderName::ContentLength => {
        UserError::ImmutableRequestHeaderModified(HeaderName::ContentLength, hdr_value.to_string())
          .into()
      }
      _ => {
        self.headers.set(hdr, value);
        Ok(())
      }
    }
  }

  /// Adds a new header value to the headers. This can be the first value with the given key or an additional value.
  pub fn add_header(&mut self, name: impl HeaderLike, value: impl AsRef<str>) -> HumptyResult<()> {
    let hdr = name.to_header();
    let hdr_value = value.as_ref();
    match &hdr {
      HeaderName::Accept => {
        if let Some(accept) = AcceptMime::parse(hdr_value) {
          if let Some(old_value) = self.headers.try_set(hdr, hdr_value) {
            return UserError::MultipleAcceptHeaderValuesSet(
              old_value.to_string(),
              hdr_value.to_string(),
            )
            .into();
          }
          self.accept = accept;
          return Ok(());
        }
        UserError::IllegalAcceptHeaderValueSet(hdr_value.to_string()).into()
      }
      HeaderName::TransferEncoding => UserError::ImmutableRequestHeaderModified(
        HeaderName::TransferEncoding,
        hdr_value.to_string(),
      )
      .into(),
      HeaderName::ContentLength => {
        UserError::ImmutableRequestHeaderModified(HeaderName::ContentLength, hdr_value.to_string())
          .into()
      }
      _ => {
        self.headers.add(hdr, value);
        Ok(())
      }
    }
  }
}
