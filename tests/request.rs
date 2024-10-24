mod mock_stream;

use crate::mock_stream::MockStream;
use humpty::http::address::Address;
use humpty::http::cookie::Cookie;
use humpty::http::headers::{HeaderType, Headers};
use humpty::http::method::Method;
use humpty::http::Request;

use humpty::http::request::HttpVersion;
use humpty::stream::IntoConnectionStream;
use std::collections::VecDeque;
use std::iter::FromIterator;

#[test]
fn test_request_from_stream() {
  let test_data = b"GET /testpath?foo=bar HTTP/1.1\r\nHost: localhost\r\n\r\n";
  let stream = MockStream::with_data(VecDeque::from_iter(test_data.iter().cloned()));
  let raw_stream = stream.clone().into_connection_stream();

  let request = Request::from_stream(raw_stream.as_ref(), "1.2.3.4:5678".parse().unwrap());

  let request = request.unwrap();
  let expected_uri: String = "/testpath".into();
  let expected_query: String = "foo=bar".into();
  assert_eq!(request.method, Method::Get);
  assert_eq!(request.path, expected_uri);
  assert_eq!(request.query, expected_query);
  assert_eq!(request.version, HttpVersion::Http11);
  assert!(request.body.is_none());
  assert_eq!(request.address, Address::new("1.2.3.4:5678").unwrap());

  let mut expected_headers: Headers = Headers::new();
  expected_headers.add(HeaderType::Host, "localhost");
  assert_eq!(request.headers, expected_headers);
}

#[test]
fn test_cookie_request() {
  let test_data = b"GET / HTTP/1.1\r\nHost: localhost\r\nCookie: foo=bar; baz=qux\r\n\r\n";
  let stream = MockStream::with_data(VecDeque::from_iter(test_data.iter().cloned()));
  let raw_stream = stream.clone().into_connection_stream();
  let request = Request::from_stream(raw_stream.as_ref(), "1.2.3.4:5678".parse().unwrap()).unwrap();

  let mut expected_cookies = vec![Cookie::new("foo", "bar"), Cookie::new("baz", "qux")];

  assert_eq!(request.get_cookies(), expected_cookies);

  assert_eq!(request.get_cookie("baz"), expected_cookies.pop());
  assert_eq!(request.get_cookie("foo"), expected_cookies.pop());
  assert_eq!(request.get_cookie("sus"), None);
}

#[test]
fn test_proxied_request_from_stream() {
  let test_data =
    b"GET /testpath HTTP/1.1\r\nHost: localhost\r\nX-Forwarded-For: 9.10.11.12,13.14.15.16\r\n\r\n";
  let stream = MockStream::with_data(VecDeque::from_iter(test_data.iter().cloned()));
  let raw_stream = stream.clone().into_connection_stream();

  let request = Request::from_stream(raw_stream.as_ref(), "1.2.3.4:5678".parse().unwrap());

  let request = request.unwrap();
  let expected_uri: String = "/testpath".into();
  assert_eq!(request.method, Method::Get);
  assert_eq!(request.path, expected_uri);
  assert_eq!(request.version, HttpVersion::Http11);
  assert!(request.body.is_none());
  assert_eq!(
    request.address,
    Address {
      origin_addr: "13.14.15.16".parse().unwrap(),
      proxies: vec!["9.10.11.12".parse().unwrap(), "1.2.3.4".parse().unwrap()],
      port: 5678
    }
  );

  let mut expected_headers: Headers = Headers::new();
  expected_headers.add(HeaderType::Host, "localhost");
  expected_headers.add("X-Forwarded-For", "9.10.11.12,13.14.15.16");

  assert_eq!(request.headers, expected_headers);
}
