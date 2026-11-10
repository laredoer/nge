//! 原生的连接：每条连接一个线程，里面一个单线程的 tokio 运行时；和引擎之间只用通道。
//! 读和写并发，写有超时，待发的有上限；`Link` 丢掉后线程最多再过 `CLOSE_GRACE` 退出（卡在连接、写的时候也一样）。

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio::sync::oneshot;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use super::{Frame, Link, LinkEvent, MAX_MESSAGE};

/// 握手最多等多久。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 写一帧最多等多久：对方不读就当断了。
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// `Link` 丢掉以后最多再花多久好好关（发 Close）。
const CLOSE_GRACE: Duration = Duration::from_secs(1);
/// 最多排几帧待发：排满了当断了。
const OUTBOX: usize = 256;

static THREADS: AtomicUsize = AtomicUsize::new(0);

/// 还在跑的连接线程数（测试看线程有没有退出）。
pub fn live_threads() -> usize {
    THREADS.load(Ordering::SeqCst)
}

struct Counted;

impl Counted {
    fn new() -> Self {
        THREADS.fetch_add(1, Ordering::SeqCst);
        Counted
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        THREADS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct NativeLink {
    /// 断了（自己判的，比如待发的排满了）以后是 `None`。
    out: Option<Sender<Frame>>,
    inbox: Mutex<mpsc::Receiver<LinkEvent>>,
    /// 自己判断断了的原因：下一次 `poll` 报出去。
    failed: Option<String>,
    /// 已经报过 `Closed`：之后什么都不报。
    done: bool,
    /// 丢掉时通知线程。
    _dropped: oneshot::Sender<()>,
}

impl NativeLink {
    pub fn connect(url: &str) -> Self {
        let (out, out_rx) = channel(OUTBOX);
        let (tx, inbox) = mpsc::channel();
        let (dropped, dropped_rx) = oneshot::channel();
        let url = url.to_owned();
        let spawned = std::thread::Builder::new().name("nge-net".into()).spawn({
            let tx = tx.clone();
            let counted = Counted::new();
            move || {
                let _counted = counted;
                match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt.block_on(run(url, out_rx, tx, dropped_rx)),
                    Err(e) => drop(tx.send(LinkEvent::Closed(format!("起不了网络线程的运行时: {e}")))),
                }
            }
        });
        if let Err(e) = spawned {
            let _ = tx.send(LinkEvent::Closed(format!("起不了网络线程: {e}")));
        }
        Self { out: Some(out), inbox: Mutex::new(inbox), failed: None, done: false, _dropped: dropped }
    }
}

impl Link for NativeLink {
    fn send(&mut self, frame: Frame) {
        let Some(out) = &self.out else { return };
        // 线程已经退出（断了）时发不出去，丢掉；排满了是对方收不动，当断了（丢掉发送端，线程去关连接）
        if let Err(TrySendError::Full(_)) = out.try_send(frame) {
            self.out = None;
            self.failed = Some(format!("待发的超过 {OUTBOX} 帧，对方收不动"));
        }
    }

    fn poll(&mut self) -> Vec<LinkEvent> {
        if self.done {
            return vec![];
        }
        let mut events: Vec<LinkEvent> = match self.inbox.lock() {
            Ok(inbox) => inbox.try_iter().collect(),
            Err(_) => vec![],
        };
        if let Some(why) = self.failed.take() {
            events.push(LinkEvent::Closed(why));
        }
        // Closed 之后的都不要（自己判断断了和线程报的可能都有）
        if let Some(end) = events.iter().position(|e| matches!(e, LinkEvent::Closed(_))) {
            events.truncate(end + 1);
            self.done = true;
            self.out = None;
        }
        events
    }
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn run(url: String, out: Receiver<Frame>, tx: mpsc::Sender<LinkEvent>, mut dropped: oneshot::Receiver<()>) {
    let config = WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE)).max_frame_size(Some(MAX_MESSAGE));
    let connecting = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async_with_config(url.as_str(), Some(config), true));
    let ws = tokio::select! {
        r = connecting => match r {
            Ok(Ok((ws, _))) => ws,
            Ok(Err(e)) => return drop(tx.send(LinkEvent::Closed(format!("连不上 {url}: {e}")))),
            Err(_) => return drop(tx.send(LinkEvent::Closed(format!("连 {url} 超时")))),
        },
        // 还没连上 Link 就丢了
        _ = &mut dropped => return,
    };
    if tx.send(LinkEvent::Open).is_err() {
        return;
    }
    let (sink, stream) = ws.split();
    // 读、写哪个先结束连接就结束；Link 丢掉后给写的一点时间好好关，到点直接断
    let reason = tokio::select! {
        reason = read_loop(stream, &tx) => reason,
        reason = write_loop(sink, out) => reason,
        _ = async { let _ = (&mut dropped).await; tokio::time::sleep(CLOSE_GRACE).await } => None,
    };
    if let Some(reason) = reason {
        let _ = tx.send(LinkEvent::Closed(reason));
    }
}

/// 收到的交给引擎；返回断开的原因（引擎那边已经不收了是 `None`）。
async fn read_loop(mut stream: SplitStream<Ws>, tx: &mpsc::Sender<LinkEvent>) -> Option<String> {
    loop {
        let event = match stream.next().await {
            Some(Ok(Message::Binary(b))) => LinkEvent::Frame(Frame::Binary(b.to_vec())),
            Some(Ok(Message::Text(t))) => LinkEvent::Frame(Frame::Text(t.as_str().to_owned())),
            Some(Ok(Message::Close(frame))) => return Some(frame.map_or_else(|| "对方关闭了连接".to_owned(), |f| format!("对方关闭了连接: {}", f.reason))),
            // ping / pong 由 tungstenite 自己回
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Some(format!("连接出错: {e}")),
            None => return Some("连接断了".to_owned()),
        };
        if tx.send(event).is_err() {
            return None;
        }
    }
}

/// 按顺序发；引擎那边丢掉发送端时好好关掉（`None`）。
async fn write_loop(mut sink: SplitSink<Ws, Message>, mut out: Receiver<Frame>) -> Option<String> {
    while let Some(frame) = out.recv().await {
        let message = match frame {
            Frame::Binary(b) => Message::Binary(b.into()),
            Frame::Text(t) => Message::Text(t.into()),
        };
        match tokio::time::timeout(WRITE_TIMEOUT, sink.send(message)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Some(format!("发送失败: {e}")),
            Err(_) => return Some(format!("{} 秒没发出去，对方收不动", WRITE_TIMEOUT.as_secs())),
        }
    }
    let _ = tokio::time::timeout(CLOSE_GRACE, sink.close()).await;
    None
}
