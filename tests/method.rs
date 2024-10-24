use humpty::http::method::Method;

#[test]
fn test_from_name() {
  assert_eq!(Method::from_name("GET"), Method::Get);
  assert_eq!(Method::from_name("POST"), Method::Post);
  assert_eq!(Method::from_name("PUT"), Method::Put);
  assert_eq!(Method::from_name("DELETE"), Method::Delete);
  assert_eq!(Method::from_name("get"), Method::Custom("get".to_string()));
  assert_eq!(Method::from_name("method"), Method::Custom("method".to_string()));
  assert_eq!(Method::from_name(""), Method::Custom("".to_string()));
}
