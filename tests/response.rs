mod mock_stream;

use humpty::http::cookie::{SameSite, SetCookie};
use humpty::http::headers::{HeaderType, Headers};
use humpty::http::response::Response;
use humpty::http::status::StatusCode;
use mock_stream::MockStream;

use std::collections::VecDeque;
use std::iter::FromIterator;
use std::time::Duration;

#[test]
fn test_response() {
  let response = Response::empty(StatusCode::OK)
    .with_bytes(b"<body>test</body>\r\n")
    .with_header(HeaderType::ContentType, "text/html")
    .with_header(HeaderType::ContentLanguage, "en-GB")
    .with_header(HeaderType::Date, "Thu, 1 Jan 1970 00:00:00 GMT"); // this would never be manually set in prod, but is obviously required for testing

  assert_eq!(response.get_headers().get(&HeaderType::ContentType), Some("text/html"));

  let expected_bytes: Vec<u8> = b"HTTP/1.1 200 OK\r\nDate: Thu, 1 Jan 1970 00:00:00 GMT\r\nContent-Language: en-GB\r\nContent-Type: text/html\r\n\r\n<body>test</body>\r\n".to_vec();
  let bytes: Vec<u8> = response.into();

  assert_eq!(bytes, expected_bytes);
}

#[test]
fn test_cookie_response() {
  let response = Response::empty(StatusCode::OK)
    .with_bytes(b"Hello, world!\r\n")
    .with_cookie(
      SetCookie::new("X-Example-Cookie", "example-value")
        .with_path("/")
        .with_max_age(Duration::from_secs(3600))
        .with_secure(true),
    )
    .with_cookie(
      SetCookie::new("X-Example-Token", "example-token")
        .with_domain("example.com")
        .with_same_site(SameSite::Strict)
        .with_secure(true),
    );

  assert_eq!(
    response.get_headers().get_all(&HeaderType::SetCookie),
    vec![
      "X-Example-Cookie=example-value; Max-Age=3600; Path=/; Secure",
      "X-Example-Token=example-token; Domain=example.com; SameSite=Strict; Secure"
    ]
  );

  let expected_bytes: Vec<u8> =
        b"HTTP/1.1 200 OK\r\nSet-Cookie: X-Example-Cookie=example-value; Max-Age=3600; Path=/; Secure\r\nSet-Cookie: X-Example-Token=example-token; Domain=example.com; SameSite=Strict; Secure\r\n\r\nHello, world!\r\n"
            .to_vec();
  let bytes: Vec<u8> = response.into();

  assert_eq!(bytes, expected_bytes);
}

#[test]
fn test_response_from_stream() {
  let test_data = b"HTTP/1.1 404 Not Found\r\nContent-Length: 51\r\n\r\nThe requested resource was not found on the server.\r\n";
  let mut stream = MockStream::with_data(VecDeque::from_iter(test_data.iter().cloned()));
  let response = Response::from_stream(&mut stream);

  assert!(response.is_ok());

  let response = response.unwrap();
  let expected_body = b"The requested resource was not found on the server.".to_vec();
  assert_eq!(response.body, expected_body);
  assert_eq!(response.version, "HTTP/1.1".to_string());
  assert_eq!(response.status_code, StatusCode::NotFound);

  let mut expected_headers: Headers = Headers::new();
  expected_headers.add(HeaderType::ContentLength, "51");
  assert_eq!(response.headers, expected_headers);
}
