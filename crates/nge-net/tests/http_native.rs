//! 原生的 HTTP 后端：测试里起一个只回一次固定字节的服务。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use nge_net::http::{HttpDone, HttpRequest, MAX_RESPONSE, backend};

/// 起一个服务：读完请求头，回 `head` 加 `body`。返回地址。
fn serve_once(head: String, body: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        let mut got = Vec::new();
        while !got.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = s.read(&mut buf).unwrap();
            if n == 0 {
                return;
            }
            got.extend_from_slice(&buf[..n]);
        }
        let _ = s.write_all(head.as_bytes());
        let _ = s.write_all(&body);
    });
    addr
}

fn run(request: HttpRequest) -> HttpDone {
    let mut b = backend();
    b.start(1, request);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(done) = b.poll().pop() {
            return done;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("没回来");
}

#[test]
fn a_body_that_is_not_utf8_is_decoded_lossily() {
    let body = vec![b'o', b'k', 0xff, 0xfe, b'!'];
    let addr = serve_once(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()), body);
    let done = run(HttpRequest::get(format!("http://{addr}/")));
    assert_eq!(done.error, None);
    assert_eq!((done.status, done.body.as_str()), (200, "ok\u{fffd}\u{fffd}!"));
}

#[test]
fn a_response_over_the_limit_is_an_error() {
    let size = MAX_RESPONSE as usize + 10;
    let addr = serve_once(format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"), vec![b'a'; size]);
    let done = run(HttpRequest::get(format!("http://{addr}/")));
    assert!(done.error.as_deref().is_some_and(|e| e.contains("响应超过")), "{done:?}");
    assert!(done.body.is_empty());
}
