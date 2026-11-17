//! 原生连接：测试里起一个回声的 WebSocket 服务（随机端口），收发文本和二进制帧、对方关闭、连不上。

use std::net::TcpListener as StdListener;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use nge_net::link::{Frame, Link, LinkEvent, connect};

/// 起一个回声服务：收到 "bye" 就关掉连接。返回端口。
fn echo_server() -> u16 {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                    while let Some(Ok(m)) = ws.next().await {
                        if m.to_text().is_ok_and(|t| t == "bye") {
                            let _ = ws.close(None).await;
                            return;
                        }
                        if m.is_text() || m.is_binary() {
                            ws.send(m).await.unwrap();
                        }
                    }
                });
            }
        });
    });
    port
}

/// 一直 poll 到拿够 `n` 个事件（最多等 5 秒）。
fn wait(link: &mut Box<dyn Link>, n: usize) -> Vec<LinkEvent> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    while events.len() < n && Instant::now() < deadline {
        events.extend(link.poll());
        std::thread::sleep(Duration::from_millis(5));
    }
    events
}

#[test]
fn sends_and_receives_text_and_binary_frames() {
    let port = echo_server();
    let mut link = connect(&format!("ws://127.0.0.1:{port}"));
    // 连上之前发的排着，连上后按顺序发
    link.send(Frame::Text("你好".into()));
    link.send(Frame::Binary(vec![1, 2, 3]));
    let events = wait(&mut link, 3);
    assert_eq!(
        events,
        vec![LinkEvent::Open, LinkEvent::Frame(Frame::Text("你好".into())), LinkEvent::Frame(Frame::Binary(vec![1, 2, 3]))]
    );
    link.send(Frame::Text("bye".into()));
    let events = wait(&mut link, 1);
    assert!(matches!(&events[..], [LinkEvent::Closed(_)]), "{events:?}");
}

#[test]
fn an_unreachable_server_reports_closed() {
    // 拿一个空闲端口再放掉：没人在听
    let port = StdListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut link = connect(&format!("ws://127.0.0.1:{port}"));
    let events = wait(&mut link, 1);
    assert!(matches!(&events[..], [LinkEvent::Closed(why)] if why.contains("连不上")), "{events:?}");
}

#[test]
fn polling_never_blocks() {
    let port = echo_server();
    let mut link = connect(&format!("ws://127.0.0.1:{port}"));
    let start = Instant::now();
    for _ in 0..1000 {
        link.poll();
        link.send(Frame::Binary(vec![0]));
    }
    assert!(start.elapsed() < Duration::from_millis(200), "{:?}", start.elapsed());
}
