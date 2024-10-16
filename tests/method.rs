use humpty::http::method::Method;
use humpty::http::request::RequestError;

#[test]
fn test_from_name() {
  assert_eq!(Method::from_name("GET"), Ok(Method::Get));
  assert_eq!(Method::from_name("POST"), Ok(Method::Post));
  assert_eq!(Method::from_name("PUT"), Ok(Method::Put));
  assert_eq!(Method::from_name("DELETE"), Ok(Method::Delete));
  assert_eq!(Method::from_name("get"), Err(RequestError::Request));
  assert_eq!(Method::from_name("method"), Err(RequestError::Request));
  assert_eq!(Method::from_name(""), Err(RequestError::Request));
}
