//! 把 `Hub` 接到 TCP 上：每条连接一个任务（读和写在同一个任务里并发，写有超时），`Hub` 在一个任务里（每秒 `tick` 一次）；
//! 同一个端口上不是 WebSocket 升级的请求当 HTTP（排行榜）处理。
//! 背压：每条连接待发的帧有上限，排满了当慢客户端踢掉；写一帧、关连接都最多等 5 秒；同时的连接数有上限。

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::http::{self, MAX_BODY, MAX_HEAD, Response};
use crate::hub::{Config, ConnId, Hub, Out};
use crate::scores::Leaderboard;

/// 请求头、WebSocket 握手最多等多久。
const HEAD_TIMEOUT: Duration = Duration::from_secs(5);
/// 写一帧、关连接最多等多久：对方不读就算断了。
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// WebSocket 一条消息的硬上限：再大的协议层直接断开（`max_frame` 以内的才进 `Hub`，中间的回 `TooLarge`）。
const WS_LIMIT: usize = 64 * 1024;
/// 发往 `Hub` 的消息最多排几条：排满了读连接的任务等着（背压到 TCP）。
const HUB_INBOX: usize = 4096;
/// accept 暂时出错（文件描述符用完之类）后歇多久再接。
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);
/// 请求体太大时最多读掉多少再回 413：读掉了关连接对方才收得到响应。
const DRAIN_LIMIT: usize = 64 * 1024;
/// 读掉这一段最多等多久。
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

type Ws = WebSocketStream<TcpStream>;

enum ToHub {
    Open(ConnId, Handle),
    Binary(ConnId, Vec<u8>),
    Text(ConnId),
    Closed(ConnId),
}

enum ToConn {
    Bytes(Vec<u8>),
    Close,
}

/// `Hub` 手里的一条连接：待发的队列（有上限）、立刻断开的开关。
struct Handle {
    tx: Sender<ToConn>,
    kill: oneshot::Sender<()>,
}

/// 接新连接（测试里换成会出错的）。
pub trait Accept: Send + 'static {
    fn accept(&mut self) -> impl Future<Output = io::Result<TcpStream>> + Send;
}

impl Accept for TcpListener {
    async fn accept(&mut self) -> io::Result<TcpStream> {
        TcpListener::accept(self).await.map(|(s, _)| s)
    }
}

/// 现在有几条连接。
#[derive(Debug, Clone, Default)]
pub struct Live(Arc<AtomicUsize>);

impl Live {
    pub fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

/// 在新线程里跑着的中继。
#[derive(Debug, Clone)]
pub struct Running {
    pub addr: SocketAddr,
    pub live: Live,
}

/// 连接 id：从 1 数，回绕时跳过还在用的。
#[derive(Debug, Default)]
pub(crate) struct Ids {
    next: ConnId,
    used: HashSet<ConnId>,
}

impl Ids {
    pub(crate) fn take(&mut self) -> ConnId {
        loop {
            self.next = self.next.checked_add(1).unwrap_or(1);
            if self.used.insert(self.next) {
                return self.next;
            }
        }
    }

    pub(crate) fn give_back(&mut self, id: ConnId) {
        self.used.remove(&id);
    }
}

/// 连接任务结束时：还回 id 和名额。
struct Slot {
    id: ConnId,
    ids: Arc<Mutex<Ids>>,
    live: Live,
    _permit: OwnedSemaphorePermit,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.ids.lock().unwrap_or_else(|e| e.into_inner()).give_back(self.id);
        self.live.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 在 `listener` 上服务，直到它坏掉。
pub async fn serve(listener: TcpListener, config: Config) -> io::Result<()> {
    serve_on(listener, config, Live::default()).await
}

/// 同 [`serve`]，连接从 `acceptor` 来，连接数记在 `live` 里。
pub async fn serve_on(mut acceptor: impl Accept, config: Config, live: Live) -> io::Result<()> {
    let (hub, inbox) = channel(HUB_INBOX);
    let outbox = config.outbox.max(1);
    let permits = Arc::new(Semaphore::new(config.max_conns.max(1)));
    let locked = config.auth.is_some();
    let hub_logic = Hub::new(config);
    let scores = hub_logic.scores();
    tokio::spawn(run_hub(hub_logic, inbox));
    let ids = Arc::new(Mutex::new(Ids::default()));
    loop {
        let stream = match acceptor.accept().await {
            Ok(stream) => stream,
            Err(e) if listener_broken(&e) => return Err(e),
            Err(e) => {
                log::warn!("accept 出错，歇一下再接: {e}");
                tokio::time::sleep(ACCEPT_PAUSE).await;
                continue;
            }
        };
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            log::warn!("连接数到上限了，新连接直接关掉");
            drop(stream);
            continue;
        };
        let id = ids.lock().unwrap_or_else(|e| e.into_inner()).take();
        live.0.fetch_add(1, Ordering::SeqCst);
        let slot = Slot { id, ids: ids.clone(), live: live.clone(), _permit: permit };
        tokio::spawn(connection(stream, slot, hub.clone(), scores.clone(), outbox, locked));
    }
}

/// 监听的 socket 本身坏了（不是某一条连接、也不是资源暂时不够）。
fn listener_broken(e: &io::Error) -> bool {
    const EBADF: i32 = 9;
    const EINVAL: i32 = 22;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const ENOTSOCK: i32 = 38;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    const ENOTSOCK: i32 = 88;
    matches!(e.raw_os_error(), Some(EBADF | EINVAL | ENOTSOCK))
}

/// 在新线程里起一个中继（测试和嵌进别的程序用）。
pub fn spawn(addr: &str, config: Config) -> io::Result<Running> {
    let listener = std::net::TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    let local = listener.local_addr()?;
    let live = Live::default();
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let counted = live.clone();
    std::thread::Builder::new().name("nge-relay".into()).spawn(move || {
        runtime.block_on(async move {
            match TcpListener::from_std(listener) {
                Ok(listener) => {
                    if let Err(e) = serve_on(listener, config, counted).await {
                        log::error!("中继停了: {e}");
                    }
                }
                Err(e) => log::error!("中继起不来: {e}"),
            }
        })
    })?;
    Ok(Running { addr: local, live })
}

async fn run_hub(mut hub: Hub, mut inbox: Receiver<ToHub>) {
    let start = Instant::now();
    let mut conns: HashMap<ConnId, Handle> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    // 权威房间的固定 tick：落后了（机器卡了一下）就连着补，保持和墙上的时间对齐
    let mut arena = tokio::time::interval(Duration::from_secs_f64(1.0 / f64::from(nge_net::arena::TICK_HZ)));
    loop {
        let out = tokio::select! {
            msg = inbox.recv() => {
                let now = start.elapsed().as_secs_f64();
                match msg {
                    None => return,
                    Some(ToHub::Open(id, handle)) => {
                        conns.insert(id, handle);
                        hub.connect(id, now)
                    }
                    Some(ToHub::Binary(id, bytes)) => hub.frame(id, &bytes, now),
                    Some(ToHub::Text(id)) => hub.text(id, now),
                    Some(ToHub::Closed(id)) => {
                        conns.remove(&id);
                        hub.close(id)
                    }
                }
            }
            _ = tick.tick() => hub.tick(start.elapsed().as_secs_f64()),
            _ = arena.tick() => hub.step(),
        };
        deliver(&mut hub, &mut conns, out);
    }
}

/// 把 `Hub` 要发的放进各条连接的队列；队列满了的是慢客户端：立刻断开，房间里的人收到离开。
fn deliver(hub: &mut Hub, conns: &mut HashMap<ConnId, Handle>, out: Vec<Out>) {
    let mut queue = VecDeque::from(out);
    while let Some(o) = queue.pop_front() {
        match o {
            Out::Send(id, bytes) => {
                let Some(handle) = conns.get(&id) else { continue };
                if let Err(TrySendError::Full(_)) = handle.tx.try_send(ToConn::Bytes(bytes)) {
                    log::warn!("连接 {id} 收得太慢，踢掉");
                    if let Some(handle) = conns.remove(&id) {
                        let _ = handle.kill.send(());
                    }
                    queue.extend(hub.close(id));
                }
            }
            Out::Close(id) => {
                // 排在已有的帧后面关（先让对方收到错误码）；排不进去就直接断
                if let Some(handle) = conns.remove(&id)
                    && handle.tx.try_send(ToConn::Close).is_err()
                {
                    let _ = handle.kill.send(());
                }
            }
        }
    }
}

async fn connection(mut stream: TcpStream, slot: Slot, hub: Sender<ToHub>, scores: Arc<Mutex<Leaderboard>>, outbox: usize, locked: bool) {
    let _ = stream.set_nodelay(true);
    // 先偷看请求头：WebSocket 升级交给 tungstenite（它自己再读一遍），别的当 HTTP
    let head = match tokio::time::timeout(HEAD_TIMEOUT, peek_head(&stream)).await {
        Ok(Ok(Some(head))) => head,
        _ => return,
    };
    if head.websocket {
        websocket(stream, slot.id, hub, outbox).await;
    } else {
        let response = answer(&mut stream, &head, &scores, locked).await;
        let _ = tokio::time::timeout(WRITE_TIMEOUT, async {
            let _ = stream.write_all(&http::render(&response)).await;
            let _ = stream.shutdown().await;
        })
        .await;
    }
}

/// 偷看到完整的请求头为止（不从流里取走）。
async fn peek_head(stream: &TcpStream) -> io::Result<Option<http::Head>> {
    let mut buf = vec![0; MAX_HEAD];
    let mut seen = 0;
    loop {
        let n = stream.peek(&mut buf).await?;
        if n == 0 {
            return Ok(None);
        }
        if let Some(end) = http::head_end(&buf[..n]) {
            return Ok(http::parse_head(&buf[..end]).ok());
        }
        if n == buf.len() {
            return Ok(None);
        }
        // peek 有数据就马上返回：没来新的字节时稍等再看，别空转
        if n == seen {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        seen = n;
    }
}

async fn answer(stream: &mut TcpStream, head: &http::Head, scores: &Mutex<Leaderboard>, locked: bool) -> Response {
    // 先把请求头读掉：没读的数据留在缓冲里时关连接会变成 RST，对方收不到响应
    let Some(start) = read_head(stream).await else { return Response { status: 400, body: r#"{"error":"请求头不对"}"#.into() } };
    if head.content_length > MAX_BODY {
        // 太大的也读掉一段（最多 64 KiB），对方多半收得到 413
        let _ = tokio::time::timeout(DRAIN_TIMEOUT, read_body(stream, start, head.content_length.min(DRAIN_LIMIT))).await;
        return Response { status: 413, body: r#"{"error":"请求体太大"}"#.into() };
    }
    let Some(body) = read_body(stream, start, head.content_length).await else {
        return Response { status: 400, body: r#"{"error":"请求体不完整"}"#.into() };
    };
    let mut board = scores.lock().unwrap_or_else(|e| e.into_inner());
    http::route(head, &body, &mut board, locked)
}

/// 读到 `len` 字节的请求体（`start` 是读请求头时多读的）；断了、超时是 `None`。
async fn read_body(stream: &mut TcpStream, mut body: Vec<u8>, len: usize) -> Option<Vec<u8>> {
    while body.len() < len {
        let mut chunk = [0; 4096];
        match tokio::time::timeout(HEAD_TIMEOUT, stream.read(&mut chunk)).await {
            Ok(Ok(n)) if n > 0 => body.extend_from_slice(&chunk[..n]),
            _ => return None,
        }
    }
    body.truncate(len);
    Some(body)
}

/// 把请求头从流里读掉，返回读多了的（请求体的开头）。
async fn read_head(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0; 1024];
    loop {
        let n = tokio::time::timeout(HEAD_TIMEOUT, stream.read(&mut chunk)).await.ok()?.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = http::head_end(&buf) {
            return Some(buf.split_off(end));
        }
        if buf.len() > MAX_HEAD {
            return None;
        }
    }
}

async fn websocket(stream: TcpStream, id: ConnId, hub: Sender<ToHub>, outbox: usize) {
    let config = WebSocketConfig::default().max_message_size(Some(WS_LIMIT)).max_frame_size(Some(WS_LIMIT));
    let Ok(Ok(ws)) = tokio::time::timeout(HEAD_TIMEOUT, tokio_tungstenite::accept_async_with_config(stream, Some(config))).await else { return };
    let (tx, rx) = channel(outbox);
    let (kill, killed) = oneshot::channel();
    if hub.send(ToHub::Open(id, Handle { tx, kill })).await.is_err() {
        return;
    }
    let (sink, source) = ws.split();
    // 读、写、被踢，哪个先结束连接就结束（`Hub` 丢掉开关不算被踢，等写完排着的再关）
    tokio::select! {
        _ = read_loop(source, id, &hub) => {}
        _ = write_loop(sink, rx) => {}
        Ok(()) = killed => {}
    }
    let _ = hub.send(ToHub::Closed(id)).await;
}

async fn read_loop(mut source: SplitStream<Ws>, id: ConnId, hub: &Sender<ToHub>) {
    while let Some(message) = source.next().await {
        let msg = match message {
            Ok(Message::Binary(b)) => ToHub::Binary(id, b.to_vec()),
            Ok(Message::Text(_)) => ToHub::Text(id),
            Ok(Message::Close(_)) | Err(_) => return,
            Ok(_) => continue,
        };
        if hub.send(msg).await.is_err() {
            return;
        }
    }
}

async fn write_loop(mut sink: SplitSink<Ws, Message>, mut rx: Receiver<ToConn>) {
    while let Some(out) = rx.recv().await {
        match out {
            ToConn::Bytes(b) => {
                if !matches!(tokio::time::timeout(WRITE_TIMEOUT, sink.send(Message::Binary(b.into()))).await, Ok(Ok(()))) {
                    return;
                }
            }
            ToConn::Close => break,
        }
    }
    let _ = tokio::time::timeout(WRITE_TIMEOUT, sink.close()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_wrap_around_and_skip_the_ones_in_use() {
        let mut ids = Ids::default();
        assert_eq!((ids.take(), ids.take()), (1, 2));
        ids.give_back(1);
        ids.next = ConnId::MAX - 1;
        assert_eq!(ids.take(), ConnId::MAX);
        // 回绕：1 空出来了能用，2 还在用跳过
        assert_eq!(ids.take(), 1);
        assert_eq!(ids.take(), 3);
        ids.give_back(2);
        ids.next = ConnId::MAX;
        assert_eq!(ids.take(), 2);
    }

    #[test]
    fn only_a_broken_listener_is_fatal() {
        assert!(listener_broken(&io::Error::from_raw_os_error(9)));
        for e in [io::Error::from_raw_os_error(24), io::Error::from(io::ErrorKind::ConnectionAborted), io::Error::from(io::ErrorKind::Interrupted)] {
            assert!(!listener_broken(&e), "{e}");
        }
    }
}
