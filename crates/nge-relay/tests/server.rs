//! 中继服务本身的健壮性：accept 出错、连接数上限、只连不读的慢客户端、踢出后连接真的断开、过大的 HTTP 请求体。

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::TcpStream as StdStream;
use std::time::{Duration, Instant};

use nge_net::proto::{self, ClientMsg, ErrorCode, PROTOCOL_VERSION, PeerId, RelayMsg};
use nge_relay::server::{Accept, Live, serve_on};
use nge_relay::Config;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::{self, Message, WebSocket};

/// 先吐出 `errors`，之后照常 accept。
struct Flaky {
    inner: TcpListener,
    errors: VecDeque<io::Error>,
}

impl Accept for Flaky {
    async fn accept(&mut self) -> io::Result<TcpStream> {
        if let Some(e) = self.errors.pop_front() {
            return Err(e);
        }
        self.inner.accept().await.map(|(s, _)| s)
    }
}

/// 在自己的运行时里跑 `serve_on`，返回运行时（留着它服务才活着）、地址和连接数。
fn start(errors: Vec<io::Error>, config: Config) -> (tokio::runtime::Runtime, String, Live, std::sync::mpsc::Receiver<io::Result<()>>) {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let inner = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = inner.local_addr().unwrap().to_string();
    let live = Live::default();
    let (done, ended) = std::sync::mpsc::channel();
    let flaky = Flaky { inner, errors: errors.into() };
    let l = live.clone();
    rt.spawn(async move {
        let _ = done.send(serve_on(flaky, config, l).await);
    });
    (rt, addr, live, ended)
}

fn get_scores(addr: &str) -> io::Result<String> {
    let mut s = StdStream::connect(addr)?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    s.write_all(b"GET /scores?board=x HTTP/1.1\r\nHost: x\r\n\r\n")?;
    let mut out = String::new();
    s.read_to_string(&mut out)?;
    Ok(out)
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("等不到: {what}");
}

#[test]
fn transient_accept_errors_do_not_stop_the_relay() {
    // EMFILE、ECONNABORTED、被打断：记下来、歇一下接着 accept
    let errors = vec![io::Error::from_raw_os_error(24), io::Error::from(io::ErrorKind::ConnectionAborted), io::Error::from(io::ErrorKind::Interrupted)];
    let (_rt, addr, _, ended) = start(errors, Config::default());
    let r = get_scores(&addr).unwrap();
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(ended.try_recv().is_err(), "服务还在");
}

#[test]
fn a_broken_listener_stops_serving() {
    // EBADF：监听的 socket 本身坏了
    let (_rt, _, _, ended) = start(vec![io::Error::from_raw_os_error(9)], Config::default());
    let result = ended.recv_timeout(Duration::from_secs(3)).expect("serve 应该返回");
    assert!(result.is_err());
}

#[test]
fn connections_over_the_limit_are_closed_and_the_relay_lives_on() {
    let (_rt, addr, live, _) = start(vec![], Config { max_conns: 2, ..Config::default() });
    // 两条只连不说话的占满名额
    let a = StdStream::connect(&addr).unwrap();
    let _b = StdStream::connect(&addr).unwrap();
    wait_until("占满两条", || live.count() == 2);
    let mut extra = StdStream::connect(&addr).unwrap();
    extra.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut buf = [0; 16];
    // 读到关闭（0 或者被重置），不是等到超时
    let r = extra.read(&mut buf);
    assert!(matches!(&r, Ok(0)) || r.as_ref().is_err_and(|e| e.kind() == io::ErrorKind::ConnectionReset), "超过上限的直接关掉: {r:?}");
    assert_eq!(live.count(), 2);
    // 走了一条，新的又能进
    drop(a);
    wait_until("空出一条", || live.count() == 1);
    assert!(get_scores(&addr).unwrap().starts_with("HTTP/1.1 200"));
}

type Ws = WebSocket<StdStream>;

/// 原始的 WebSocket 客户端：进房间，返回自己的 id。
fn join(addr: &str, room: &str, name: &str) -> (Ws, PeerId) {
    let stream = StdStream::connect(addr).unwrap();
    let (mut ws, _) = tungstenite::client(format!("ws://{addr}/"), stream).unwrap();
    let hello = proto::encode(&ClientMsg::Hello { version: PROTOCOL_VERSION, room: room.into(), name: name.into() });
    ws.send(Message::Binary(hello.into())).unwrap();
    loop {
        if let Message::Binary(b) = ws.read().unwrap()
            && let Ok(RelayMsg::Welcome { you, .. }) = proto::decode_relay(&b, usize::MAX)
        {
            return (ws, you);
        }
    }
}

/// 一直读到 `f` 认出想要的（读超时、断开都算没等到）。
fn read_until(ws: &mut Ws, f: impl FnMut(&RelayMsg) -> bool) -> bool {
    read_for(ws, Duration::from_secs(5), f)
}

/// 同 [`read_until`]，最多读 `wait` 这么久。
fn read_for(ws: &mut Ws, wait: Duration, mut f: impl FnMut(&RelayMsg) -> bool) -> bool {
    // 对方已经关了时 macOS 上设超时会报 EINVAL：照样往下读，读到的是断开
    let _ = ws.get_mut().set_read_timeout(Some(Duration::from_millis(200)));
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        match ws.read() {
            Ok(Message::Binary(b)) => {
                if proto::decode_relay(&b, usize::MAX).is_ok_and(|m| f(&m)) {
                    return true;
                }
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(_) => return false,
        }
    }
    false
}

#[test]
fn a_client_that_never_reads_is_kicked_and_the_others_carry_on() {
    let config = Config { outbox: 32, rate: 1e6, burst: 1e6, ..Config::default() };
    let (_rt, addr, live, _) = start(vec![], config);
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let (mut sender, _) = join(&addr, "r", "发");
    let (mut reader, _) = join(&addr, "r", "收");
    let (_slow, slow) = join(&addr, "r", "慢");
    wait_until("三个都在", || live.count() == 3);
    let stop = Arc::new(AtomicBool::new(false));
    // 发的一直发大帧；慢的那个从来不读，写不出去、待发的排满了就被踢
    let body = proto::encode(&ClientMsg::Data { to: None, body: vec![7; 4000] });
    let flooding = stop.clone();
    let flood = std::thread::spawn(move || {
        // 每秒大约 8 MB：读着的跟得上，不读的几下就堆满
        while !flooding.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_micros(500));
            if sender.send(Message::Binary(body.clone().into())).is_err() {
                return None;
            }
        }
        Some(sender)
    });
    // 收的那个一直在读：看到慢的离开，自己没被拖住
    let reading = stop.clone();
    let read = std::thread::spawn(move || {
        let mut saw = false;
        while !reading.load(Ordering::SeqCst) {
            saw |= read_for(&mut reader, Duration::from_millis(300), |m| *m == RelayMsg::Left(slow));
        }
        (saw, reader)
    });
    wait_until("慢的连接的任务退出", || live.count() == 2);
    std::thread::sleep(Duration::from_millis(300));
    stop.store(true, Ordering::SeqCst);
    let sender = flood.join().unwrap();
    let (saw, mut reader) = read.join().unwrap();
    assert!(saw, "收的那个收到慢的离开");
    assert!(sender.is_some(), "发的那个还连着");
    assert_eq!(live.count(), 2, "发的、收的都还在");
    reader.send(Message::Binary(proto::encode(&ClientMsg::Ping).into())).unwrap();
    assert!(read_until(&mut reader, |m| *m == RelayMsg::Pong), "收的那个照常能用");
}

#[test]
fn a_kicked_member_is_really_disconnected() {
    let config = Config { rate: 5.0, burst: 5.0, max_dropped: 10, ..Config::default() };
    let (_rt, addr, live, _) = start(vec![], config);
    let (mut other, _) = join(&addr, "r", "a");
    let (mut flooder, id) = join(&addr, "r", "b");
    wait_until("两个都在", || live.count() == 2);
    for _ in 0..40 {
        let _ = flooder.send(Message::Text("x".into()));
    }
    assert!(read_until(&mut flooder, |m| *m == RelayMsg::Error(ErrorCode::RateLimited)));
    assert!(read_until(&mut other, |m| *m == RelayMsg::Left(id)));
    wait_until("被踢的连接的任务退出", || live.count() == 1);
    // 被踢的那头读到的是关闭
    assert!(!read_until(&mut flooder, |_| true));
}

#[test]
fn an_oversized_http_body_still_gets_a_413() {
    let (_rt, addr, _, _) = start(vec![], Config::default());
    let body = vec![b'x'; 32 * 1024];
    let mut s = StdStream::connect(&addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    s.write_all(format!("POST /scores HTTP/1.1\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes()).unwrap();
    s.write_all(&body).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 413"), "{out}");
}
