//! Provides functionality for handling HTTP status codes.

use crate::util::three_digit_to_utf;

/// Represents an HTTP status code.
///
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusCode {
  /// `100 Continue`: Continue with request.
  Continue,
  /// `101 Switching Protocols`: Protocol upgrade.
  SwitchingProtocols,
  /// `200 OK`: Request succeeded.
  OK,
  /// `201 Created`: Resource created.
  Created,
  /// `202 Accepted`: Request received, but not yet acted upon.
  Accepted,
  /// `203 Non-Authoritative Information`: Request processed, but response is from another source.
  NonAuthoritative,
  /// `204 No Content`: There is no content to send for this request.
  NoContent,
  /// `205 Reset Content`: Indicates that the document which sent this request should be reset.
  ResetContent,
  /// `206 Partial Content`: This response only contains part of a resource.
  PartialContent,
  /// `300 Multiple Choice`: The request has multiple possible responses.
  MultipleChoices,
  /// `301 Moved Permanently`: The resource has moved permanently to a new location.
  MovedPermanently,
  /// `302 Found`: The resource has moved temporarily to a new location.
  Found,
  /// `303 See Other`: The resource can be found under a different URI.
  SeeOther,
  /// `304 Not Modified`: The resource has not been modified since the last request.
  NotModified,
  /// `305 Use Proxy`: The requested resource must be accessed through a proxy.
  UseProxy,
  /// `307 Temporary Redirect`: The resource has moved temporarily to a new location.
  TemporaryRedirect,
  /// `400 Bad Request`: The request could not be understood by the server.
  BadRequest,
  /// `401 Unauthorized`: The request requires user authentication.
  Unauthorized,
  /// `403 Forbidden`: The client is not allowed to access this content.
  Forbidden,
  /// `404 Not Found`: The server can not find the requested resource.
  NotFound,
  /// `405 Method Not Allowed`: The method specified in the request is not allowed for the resource.
  MethodNotAllowed,
  /// `406 Not Acceptable`: No content that meets the criteria is available.
  NotAcceptable,
  /// `407 Proxy Authentication Required`: The client must first authenticate itself with a proxy.
  ProxyAuthenticationRequired,
  /// `408 Request Timeout`: The server timed out waiting for the request.
  RequestTimeout,
  /// `409 Conflict`: The request could not be completed because of a conflict with the server's current state.
  Conflict,
  /// `410 Gone`: The requested resource is no longer available.
  Gone,
  /// `411 Length Required`: The request did not specify the length of its content.
  LengthRequired,
  /// `412 Precondition Failed`: The server does not meet one of the client's preconditions.
  PreconditionFailed,
  /// `413 Payload Too Large`: The request is larger than the server is willing or able to process.
  RequestEntityTooLarge,
  /// `414 URI Too Long`: The URI provided was too long for the server to process.
  RequestURITooLong,
  /// `415 Unsupported Media Type`: The request entity has a media type which the server or resource does not support.
  UnsupportedMediaType,
  /// `416 Requested Range Not Satisfiable`: The range specified in the `Range` header cannot be fulfilled.
  RequestedRangeNotSatisfiable,
  /// `417 Expectation Failed`: The expectation given in the `Expect` header could not be met by the server.
  ExpectationFailed,
  /// `500 Internal Server Error`: The server encountered an unexpected error which prevented it from fulfilling the request.
  InternalServerError,
  /// `501 Not Implemented`: The server does not support the functionality required to fulfill the request.
  NotImplemented,
  /// `502 Bad Gateway`: The server, while acting as a gateway or proxy, received an invalid response from the upstream server.
  BadGateway,
  /// `503 Service Unavailable`: The server is temporarily unable to handle the request.
  ServiceUnavailable,
  /// `504 Gateway Timeout`: The server, while acting as a gateway or proxy, did not receive a timely response from the upstream server.
  GatewayTimeout,
  /// `505 HTTP Version Not Supported`: The server does not support the HTTP protocol version used in the request.
  VersionNotSupported,

  /// User defined status code, some applications need non-standard custom status codes.
  CustomStr(u16, [u8; 3], &'static str),
  /// User defined status code, some applications need non-standard custom status codes.
  CustomString(u16, [u8; 3], String),
}

impl StatusCode {
  /// Creates a custom Status code from a static message and code.
  /// Codes with more or less than 3 digits or status lines with invalid content will silently turn into
  /// Internal server error. This method is intended to be called from const code so you
  /// can put your custom codes into const variables.
  ///
  pub const fn from_custom(code: u16, status_line: &'static str) -> Self {
    //TODO verify status_line doesnt contain funny characters here?
    if code < 100 || code > 999 {
      return Self::InternalServerError;
    }

    Self::CustomStr(code, three_digit_to_utf(code), status_line)
  }

  /// Creates a custom Status code from a dynamic (possibly heap allocated) message and code.
  /// # Returns
  /// None: Codes with more or less than 3 digits or status lines with invalid content
  ///
  pub fn from_custom_string<T: ToString>(code: u16, status_line: &T) -> Option<Self> {
    let status_line = status_line.to_string();
    //TODO verify status_line doesnt contain funny characters here?
    if !(100..=999).contains(&code) {
      return None;
    }

    if let Some(well_known) = Self::from_well_known_code(code) {
      if well_known.status_line() == status_line.as_str() {
        return Some(well_known);
      }
    }

    Some(Self::CustomString(code, three_digit_to_utf(code), status_line))
  }

  /// This fn returns a status code representing a well known code that is specified in the RFC for http.
  /// If the code is not well known then the fn returns the same value it would return for "500 Internal Server Error"
  ///
  pub const fn from_well_known_code_or_500(code: u16) -> Self {
    match code {
      100 => StatusCode::Continue,
      101 => StatusCode::SwitchingProtocols,
      200 => StatusCode::OK,
      201 => StatusCode::Created,
      202 => StatusCode::Accepted,
      203 => StatusCode::NonAuthoritative,
      204 => StatusCode::NoContent,
      205 => StatusCode::ResetContent,
      206 => StatusCode::PartialContent,
      300 => StatusCode::MultipleChoices,
      301 => StatusCode::MovedPermanently,
      302 => StatusCode::Found,
      303 => StatusCode::SeeOther,
      304 => StatusCode::NotModified,
      305 => StatusCode::UseProxy,
      307 => StatusCode::TemporaryRedirect,
      400 => StatusCode::BadRequest,
      401 => StatusCode::Unauthorized,
      403 => StatusCode::Forbidden,
      404 => StatusCode::NotFound,
      405 => StatusCode::MethodNotAllowed,
      406 => StatusCode::NotAcceptable,
      407 => StatusCode::ProxyAuthenticationRequired,
      408 => StatusCode::RequestTimeout,
      409 => StatusCode::Conflict,
      410 => StatusCode::Gone,
      411 => StatusCode::LengthRequired,
      412 => StatusCode::PreconditionFailed,
      413 => StatusCode::RequestEntityTooLarge,
      414 => StatusCode::RequestURITooLong,
      415 => StatusCode::UnsupportedMediaType,
      416 => StatusCode::RequestedRangeNotSatisfiable,
      417 => StatusCode::ExpectationFailed,
      501 => StatusCode::NotImplemented,
      502 => StatusCode::BadGateway,
      503 => StatusCode::ServiceUnavailable,
      504 => StatusCode::GatewayTimeout,
      505 => StatusCode::VersionNotSupported,
      _ => StatusCode::InternalServerError,
    }
  }

  /// This fn returns a status code representing a well known code that is specified in the RFC for http.
  /// # Returns
  /// None: the code was not well known.
  ///
  pub fn from_well_known_code(code: u16) -> Option<Self> {
    let well_known = Self::from_well_known_code_or_500(code);
    if well_known.code() != code {
      return None;
    }

    Some(well_known)
  }

  /// Returns the status line as a &str
  pub fn status_line(&self) -> &str {
    match self {
      StatusCode::Continue => "Continue",
      StatusCode::SwitchingProtocols => "Switching Protocols",
      StatusCode::OK => "OK",
      StatusCode::Created => "Created",
      StatusCode::Accepted => "Accepted",
      StatusCode::NonAuthoritative => "Non-Authoritative Information",
      StatusCode::NoContent => "No Content",
      StatusCode::ResetContent => "Reset Content",
      StatusCode::PartialContent => "Partial Content",
      StatusCode::MultipleChoices => "Multiple Choices",
      StatusCode::MovedPermanently => "Moved Permanently",
      StatusCode::Found => "Found",
      StatusCode::SeeOther => "See Other",
      StatusCode::NotModified => "Not Modified",
      StatusCode::UseProxy => "Use Proxy",
      StatusCode::TemporaryRedirect => "Temporary Redirect",
      StatusCode::BadRequest => "Bad Request",
      StatusCode::Unauthorized => "Unauthorized",
      StatusCode::Forbidden => "Forbidden",
      StatusCode::NotFound => "Not Found",
      StatusCode::MethodNotAllowed => "Method Not Allowed",
      StatusCode::NotAcceptable => "Not Acceptable",
      StatusCode::ProxyAuthenticationRequired => "Proxy Authentication Required",
      StatusCode::RequestTimeout => "Request Timeout",
      StatusCode::Conflict => "Conflict",
      StatusCode::Gone => "Gone",
      StatusCode::LengthRequired => "Length Required",
      StatusCode::PreconditionFailed => "Precondition Failed",
      StatusCode::RequestEntityTooLarge => "Request Entity Too Large",
      StatusCode::RequestURITooLong => "Request-URI Too Long",
      StatusCode::UnsupportedMediaType => "Unsupported Media Type",
      StatusCode::RequestedRangeNotSatisfiable => "Requested Range Not Satisfiable",
      StatusCode::ExpectationFailed => "Expectation Failed",
      StatusCode::InternalServerError => "Internal Server Error",
      StatusCode::NotImplemented => "Not Implemented",
      StatusCode::BadGateway => "Bad Gateway",
      StatusCode::ServiceUnavailable => "Service Unavailable",
      StatusCode::GatewayTimeout => "Gateway Timeout",
      StatusCode::VersionNotSupported => "HTTP Version Not Supported",
      StatusCode::CustomStr(_, _, str) => str,
      StatusCode::CustomString(_, _, str) => str.as_str(),
    }
  }

  /// Returns the 3-digit code as 3 byte utf-8 representation.
  pub const fn code_as_utf(&self) -> &[u8; 3] {
    match self {
      StatusCode::Continue => b"100",
      StatusCode::SwitchingProtocols => b"101",
      StatusCode::OK => b"200",
      StatusCode::Created => b"201",
      StatusCode::Accepted => b"202",
      StatusCode::NonAuthoritative => b"203",
      StatusCode::NoContent => b"204",
      StatusCode::ResetContent => b"205",
      StatusCode::PartialContent => b"206",
      StatusCode::MultipleChoices => b"300",
      StatusCode::MovedPermanently => b"301",
      StatusCode::Found => b"302",
      StatusCode::SeeOther => b"303",
      StatusCode::NotModified => b"304",
      StatusCode::UseProxy => b"305",
      StatusCode::TemporaryRedirect => b"307",
      StatusCode::BadRequest => b"400",
      StatusCode::Unauthorized => b"401",
      StatusCode::Forbidden => b"403",
      StatusCode::NotFound => b"404",
      StatusCode::MethodNotAllowed => b"405",
      StatusCode::NotAcceptable => b"406",
      StatusCode::ProxyAuthenticationRequired => b"407",
      StatusCode::RequestTimeout => b"408",
      StatusCode::Conflict => b"409",
      StatusCode::Gone => b"410",
      StatusCode::LengthRequired => b"411",
      StatusCode::PreconditionFailed => b"412",
      StatusCode::RequestEntityTooLarge => b"413",
      StatusCode::RequestURITooLong => b"414",
      StatusCode::UnsupportedMediaType => b"415",
      StatusCode::RequestedRangeNotSatisfiable => b"416",
      StatusCode::ExpectationFailed => b"417",
      StatusCode::InternalServerError => b"500",
      StatusCode::NotImplemented => b"501",
      StatusCode::BadGateway => b"502",
      StatusCode::ServiceUnavailable => b"503",
      StatusCode::GatewayTimeout => b"504",
      StatusCode::VersionNotSupported => b"505",
      StatusCode::CustomStr(_, code, _) => code,
      StatusCode::CustomString(_, code, _) => code,
    }
  }

  /// Returns the code as u16. This value is guaranteed to be in >= 100 <= 999 range.
  pub const fn code(&self) -> u16 {
    match self {
      StatusCode::Continue => 100,
      StatusCode::SwitchingProtocols => 101,
      StatusCode::OK => 200,
      StatusCode::Created => 201,
      StatusCode::Accepted => 202,
      StatusCode::NonAuthoritative => 203,
      StatusCode::NoContent => 204,
      StatusCode::ResetContent => 205,
      StatusCode::PartialContent => 206,
      StatusCode::MultipleChoices => 300,
      StatusCode::MovedPermanently => 301,
      StatusCode::Found => 302,
      StatusCode::SeeOther => 303,
      StatusCode::NotModified => 304,
      StatusCode::UseProxy => 305,
      StatusCode::TemporaryRedirect => 307,
      StatusCode::BadRequest => 400,
      StatusCode::Unauthorized => 401,
      StatusCode::Forbidden => 403,
      StatusCode::NotFound => 404,
      StatusCode::MethodNotAllowed => 405,
      StatusCode::NotAcceptable => 406,
      StatusCode::ProxyAuthenticationRequired => 407,
      StatusCode::RequestTimeout => 408,
      StatusCode::Conflict => 409,
      StatusCode::Gone => 410,
      StatusCode::LengthRequired => 411,
      StatusCode::PreconditionFailed => 412,
      StatusCode::RequestEntityTooLarge => 413,
      StatusCode::RequestURITooLong => 414,
      StatusCode::UnsupportedMediaType => 415,
      StatusCode::RequestedRangeNotSatisfiable => 416,
      StatusCode::ExpectationFailed => 417,
      StatusCode::InternalServerError => 500,
      StatusCode::NotImplemented => 501,
      StatusCode::BadGateway => 502,
      StatusCode::ServiceUnavailable => 503,
      StatusCode::GatewayTimeout => 504,
      StatusCode::VersionNotSupported => 505,
      StatusCode::CustomStr(code, _, _) => *code,
      StatusCode::CustomString(code, _, _) => *code,
    }
  }
}
