//! 原生连接的线程：丢掉 `Link` 后线程在有限时间内退出；对方不读时 `send` 不卡、待发的有上限。
//! 线程数是全进程的，这个文件里的测试一个一个跑。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use nge_net::link::{Frame, Link, LinkEvent, connect, live_threads};

static SERIAL: Mutex<()> = Mutex::new(());

/// 起一个握完手就再也不读的服务。返回端口。
fn deaf_server() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                    held.push(ws);
                }
            }
        });
    });
    port
}

fn wait_open(link: &mut Box<dyn Link>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if link.poll().contains(&LinkEvent::Open) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("没连上");
}

fn wait_threads(n: usize, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if live_threads() == n {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn dropping_a_link_ends_its_thread() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let port = deaf_server();
    let before = live_threads();
    let mut link = connect(&format!("ws://127.0.0.1:{port}"));
    wait_open(&mut link);
    assert_eq!(live_threads(), before + 1);
    drop(link);
    assert!(wait_threads(before, Duration::from_secs(3)), "线程还在: {}", live_threads());
    // 连不上的也一样
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    drop(connect(&format!("ws://127.0.0.1:{dead}")));
    assert!(wait_threads(before, Duration::from_secs(3)));
}

#[test]
fn a_peer_that_never_reads_cannot_hang_the_link() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let port = deaf_server();
    let before = live_threads();
    let mut link = connect(&format!("ws://127.0.0.1:{port}"));
    wait_open(&mut link);
    // 一直往里塞：send 不卡；待发的排满了马上当断了（不用等写超时）
    let start = Instant::now();
    let mut closed = None;
    while closed.is_none() && start.elapsed() < Duration::from_secs(2) {
        link.send(Frame::Binary(vec![0; 32 * 1024]));
        closed = link.poll().into_iter().find_map(|e| if let LinkEvent::Closed(why) = e { Some(why) } else { None });
    }
    let why = closed.expect("2 秒内应该因为排满了当断了");
    assert!(why.contains("待发"), "{why}");
    assert!(link.poll().is_empty(), "Closed 之后没有别的事件");
    drop(link);
    assert!(wait_threads(before, Duration::from_secs(3)), "线程还在: {}", live_threads());
}

#[test]
fn a_write_that_never_finishes_times_out() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let port = deaf_server();
    let mut link = connect(&format!("ws://127.0.0.1:{port}"));
    wait_open(&mut link);
    // 没排满，但对方不读，写着写着卡住：5 秒后当断了
    for _ in 0..200 {
        link.send(Frame::Binary(vec![0; 60 * 1024]));
    }
    let deadline = Instant::now() + Duration::from_secs(9);
    let mut closed = None;
    while closed.is_none() && Instant::now() < deadline {
        closed = link.poll().into_iter().find_map(|e| if let LinkEvent::Closed(why) = e { Some(why) } else { None });
        std::thread::sleep(Duration::from_millis(20));
    }
    let why = closed.expect("写卡住了应该当断了");
    assert!(why.contains("没发出去"), "{why}");
}
