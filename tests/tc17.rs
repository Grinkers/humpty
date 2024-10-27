use crate::mock_stream::MockStream;
use humpty::http::request::HttpVersion;
use humpty::http::request_context::RequestContext;
use humpty::http::response_body::ResponseBody;
use humpty::http::Response;
use humpty::humpty_builder::HumptyBuilder;
use humpty::humpty_error::HumptyResult;
use std::sync::atomic::AtomicU64;

mod mock_stream;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn dummy_route(ctx: &RequestContext) -> HumptyResult<Response> {
  COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
  assert_eq!(HttpVersion::Http11, ctx.request_head().version);
  let mut buf = Vec::new();
  let rt = ctx.request_body().unwrap().read_to_end(&mut buf).unwrap();
  assert_eq!(rt, 5);
  assert_eq!(String::from_utf8_lossy(&buf), "12345");

  Ok(Response::ok(ResponseBody::from_slice("Okay!")))
}

#[test]
pub fn tc17() {
  let server = HumptyBuilder::default().router(|rt| rt.with_route("/dummy", dummy_route)).build();

  let stream = MockStream::with_str("GET /dummy HTTP/1.1\r\nContent-Length: 5\r\n\r\n12345");
  let con = stream.to_stream();
  server.handle_connection(con).unwrap();
  assert_eq!(COUNTER.load(std::sync::atomic::Ordering::SeqCst), 1);
  let data = stream.copy_written_data_to_string();
  assert_eq!(data, "HTTP/1.1 200 OK\r\nConnection: Close\r\nContent-Length: 5\r\n\r\nOkay!");
}