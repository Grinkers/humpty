//! Provides a number of useful handlers for Humpty apps.

use crate::http::headers::HeaderType;
use crate::http::response_body::ResponseBody;
use crate::http::{Request, Response, StatusCode};

use std::fs::{metadata, File};
use std::io;
use std::io::ErrorKind;
use std::path::PathBuf;
use crate::percent::PercentDecode;

const INDEX_FILES: [&str; 2] = ["index.html", "index.htm"];

/// A located file or directory path.
pub enum LocatedPath {
  /// A directory was located.
  Directory,
  /// A file was located at the given path.
  File(PathBuf),
}


fn try_file_open(path: &PathBuf) -> io::Result<Response> {
  File::open(path).and_then(ResponseBody::from_file).map(Response::ok).or_else(|e| {
    if e.kind() == ErrorKind::NotFound {
      Ok(Response::not_found())
    } else {
      Err(e)
    }
  })
}

/// Serve the specified file, or a default error 404 if not found.
pub fn serve_file(file_path: &'static str) -> impl Fn(Request) -> io::Result<Response> {
  let path_buf = PathBuf::from(file_path);

  move |_| try_file_open(&path_buf)
}

/// Treat the request URI as a file path relative to the given directory and serve files from there.
///
/// ## Examples
/// - directory path of `.` will serve files relative to the current directory
/// - directory path of `./static` will serve files from the static directory but with their whole URI,
///     for example a request to `/images/ferris.png` will map to the file `./static/images/ferris.png`.
///
/// This is **not** equivalent to `serve_dir`, as `serve_dir` respects index files within nested directories.
pub fn serve_as_file_path(
  directory_path: &'static str,
) -> impl Fn(Request) -> io::Result<Response> {
  move |request: Request| {
    let directory_path = directory_path.strip_suffix('/').unwrap_or(directory_path);
    let file_path = request.path.strip_prefix('/').unwrap_or(&request.path);
    let path = format!("{}/{}", directory_path, file_path);

    let path_buf = PathBuf::from(path);

    try_file_open(&path_buf)
  }
}

/// Serves a directory of files.
///
/// Respects index files with the following rules:
///   - requests to `/directory` will return either the file `directory`, 301 redirect to `/directory/` if it is a directory, or return 404
///   - requests to `/directory/` will return either the file `/directory/index.html` or `/directory/index.htm`, or return 404
pub fn serve_dir(directory_path: &'static str) -> impl Fn(Request, &str) -> io::Result<Response> {
  move |request: Request, route| {
    let route_without_wildcard = route.strip_suffix('*').unwrap_or(route);
    let uri_without_route =
      request.path.strip_prefix(route_without_wildcard).unwrap_or(&request.path);

    let located = try_find_path(directory_path, uri_without_route, &INDEX_FILES);

    if let Some(located) = located {
      match located {
        LocatedPath::Directory => Ok(
          Response::empty(StatusCode::MovedPermanently)
            .with_header(HeaderType::Location, format!("{}/", &request.path)),
        ),
        LocatedPath::File(path) => try_file_open(&path),
      }
    } else {
      Ok(Response::empty(StatusCode::NotFound))
    }
  }
}

/// Attempts to find a given path.
/// If the path itself is not found, attempts to find index files within it.
/// If these are not found, returns `None`.
pub fn try_find_path(
  directory: &str,
  request_path: &str,
  index_files: &[&str],
) -> Option<LocatedPath> {
  let request_path = String::from_utf8(request_path.percent_decode()?).ok()?;

  // Avoid path traversal exploits
  if request_path.contains("..") || request_path.contains(':') {
    return None;
  }

  let request_path = request_path.trim_start_matches('/');
  let directory = directory.trim_end_matches('/');

  if request_path.ends_with('/') || request_path.is_empty() {
    for filename in index_files {
      let path = format!("{}/{}{}", directory, request_path, *filename);
      if let Ok(meta) = metadata(&path) {
        if meta.is_file() {
          return Some(LocatedPath::File(PathBuf::from(path).canonicalize().unwrap()));
        }
      }
    }
  } else {
    let path = format!("{}/{}", directory, request_path);

    if let Ok(meta) = metadata(&path) {
      if meta.is_file() {
        return Some(LocatedPath::File(PathBuf::from(path).canonicalize().unwrap()));
      } else if meta.is_dir() {
        return Some(LocatedPath::Directory);
      }
    }
  }

  None
}

/// Redirects requests to the given location with status code 301.
pub fn redirect(location: &'static str) -> impl Fn(Request) -> io::Result<Response> {
  move |_| Ok(Response::redirect(location))
}
