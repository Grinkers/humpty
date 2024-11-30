use crate::mock_stream::MockStream;
use humpty::http::headers::HeaderName;
use humpty::http::mime::MimeType;
use humpty::http::request_context::RequestContext;
use humpty::http::Response;
use humpty::humpty_builder::HumptyBuilder;
use humpty::humpty_error::HumptyResult;
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};

mod mock_stream;

static COUNTER: AtomicUsize = AtomicUsize::new(0);
fn filter_set_accept(request: &mut RequestContext) -> HumptyResult<()> {
  if request.request_head().path() == "/" {
    request.request_head_mut().set_header(HeaderName::ContentType, "text/plain")?;
  }
  Ok(())
}
fn dummy_route(ctx: &RequestContext) -> HumptyResult<Response> {
  let mut r = ctx.request_body().unwrap().as_read();
  let mut v = Vec::new();
  r.read_to_end(&mut v)?;

  assert_eq!(String::from_utf8(v).unwrap(), "{}");

  COUNTER.fetch_add(1, Ordering::SeqCst);
  assert_eq!(ctx.request_head().get_content_type().unwrap(), &MimeType::TextPlain);
  Ok(Response::no_content())
}

#[test]
pub fn tc33() {
  let server = HumptyBuilder::builder(|builder| {
    builder
      .router(|rt| {
        rt.get("/*")
          .consumes(MimeType::TextPlain)
          .endpoint(dummy_route)?
          .with_pre_routing_request_filter(filter_set_accept)
      })?
      .with_max_head_buffer_size(512)?
      .ok()
  })
  .expect("ERROR");

  let stream = MockStream::with_str(
    "GET / HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
  );
  let con = stream.to_stream();
  server.handle_connection(con).unwrap();
  let data = stream.copy_written_data_to_string();
  assert_eq!(data, "HTTP/1.1 204 No Content\r\nConnection: Close\r\nContent-Length: 0\r\n\r\n");

  let stream = MockStream::with_str(
    "GET /bla HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
  );
  let con = stream.to_stream();
  server.handle_connection(con).unwrap();
  let data = stream.copy_written_data_to_string();
  assert_eq!(
    data,
    "HTTP/1.1 415 Unsupported Media Type\r\nConnection: Close\r\nContent-Length: 0\r\n\r\n"
  );
  assert_eq!(COUNTER.load(Ordering::SeqCst), 1);
}