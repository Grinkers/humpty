//! Provides functionality for handling HTTP methods.

use std::fmt::Display;

/// Represents an HTTP method.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Method {
  /// The `GET` method.
  Get,
  /// The `POST` method.
  Post,
  /// The `PUT` method.
  Put,
  /// The `DELETE` method.
  Delete,
  /// The `OPTIONS` method.
  Options,
  /// Anything else your heart desires.
  Custom(String),
}

impl Method {
  /// Attempts to convert from the HTTP verb into an enum variant.
  ///
  /// ## Example
  /// ```
  /// let method = humpty::http::method::Method::from_name("GET");
  /// assert_eq!(method, humpty::http::method::Method::Get);
  /// ```
  pub fn from_name(name: &str) -> Self {
    match name {
      "GET" => Self::Get,
      "POST" => Self::Post,
      "PUT" => Self::Put,
      "DELETE" => Self::Delete,
      "OPTIONS" => Self::Options,
      _ => Self::Custom(name.to_string()),
    }
  }
}

impl Display for Method {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(
      f,
      "{}",
      match self {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
        Method::Options => "OPTIONS",
        Method::Custom(name) => name.as_str(),
      }
    )
  }
}
