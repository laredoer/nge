//! 把 `Hub` 接到 TCP 上：每条连接一个任务读写，`Hub` 在一个任务里（每秒 `tick` 一次）；
//! 同一个端口上不是 WebSocket 升级的请求当 HTTP（排行榜）处理。

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::http::{self, MAX_BODY, MAX_HEAD, Response};
use crate::hub::{Config, ConnId, Hub, Out};
use crate::scores::Leaderboard;

/// 请求头最多等多久。
const HEAD_TIMEOUT: Duration = Duration::from_secs(5);
/// WebSocket 一条消息的硬上限：再大的协议层直接断开（`max_frame` 以内的才进 `Hub`，中间的回 `TooLarge`）。
const WS_LIMIT: usize = 64 * 1024;

enum ToHub {
    Open(ConnId, UnboundedSender<ToConn>),
    Binary(ConnId, Vec<u8>),
    Text(ConnId),
    Closed(ConnId),
}

enum ToConn {
    Bytes(Vec<u8>),
    Close,
}

/// 在 `listener` 上服务，直到出错。
pub async fn serve(listener: TcpListener, config: Config) -> io::Result<()> {
    let (hub, inbox) = unbounded_channel();
    tokio::spawn(run_hub(Hub::new(config), inbox));
    let scores = Arc::new(Mutex::new(Leaderboard::default()));
    let mut next: ConnId = 1;
    loop {
        let (stream, _) = listener.accept().await?;
        let id = next;
        next = next.checked_add(1).unwrap_or(1);
        tokio::spawn(connection(stream, id, hub.clone(), scores.clone()));
    }
}

/// 在新线程里起一个中继（测试和嵌进别的程序用），返回实际听着的地址。
pub fn spawn(addr: &str, config: Config) -> io::Result<SocketAddr> {
    let listener = std::net::TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    let local = listener.local_addr()?;
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    std::thread::Builder::new().name("nge-relay".into()).spawn(move || {
        runtime.block_on(async move {
            match TcpListener::from_std(listener) {
                Ok(listener) => {
                    if let Err(e) = serve(listener, config).await {
                        log::error!("中继停了: {e}");
                    }
                }
                Err(e) => log::error!("中继起不来: {e}"),
            }
        })
    })?;
    Ok(local)
}

async fn run_hub(mut hub: Hub, mut inbox: UnboundedReceiver<ToHub>) {
    let start = Instant::now();
    let mut conns: HashMap<ConnId, UnboundedSender<ToConn>> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        let out = tokio::select! {
            msg = inbox.recv() => {
                let now = start.elapsed().as_secs_f64();
                match msg {
                    None => return,
                    Some(ToHub::Open(id, tx)) => {
                        conns.insert(id, tx);
                        hub.connect(id, now);
                        vec![]
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
        };
        for o in out {
            match o {
                Out::Send(id, bytes) => {
                    if let Some(tx) = conns.get(&id) {
                        let _ = tx.send(ToConn::Bytes(bytes));
                    }
                }
                Out::Close(id) => {
                    if let Some(tx) = conns.remove(&id) {
                        let _ = tx.send(ToConn::Close);
                    }
                }
            }
        }
    }
}

async fn connection(mut stream: TcpStream, id: ConnId, hub: UnboundedSender<ToHub>, scores: Arc<Mutex<Leaderboard>>) {
    let _ = stream.set_nodelay(true);
    // 先偷看请求头：WebSocket 升级交给 tungstenite（它自己再读一遍），别的当 HTTP
    let head = match tokio::time::timeout(HEAD_TIMEOUT, peek_head(&stream)).await {
        Ok(Ok(Some(head))) => head,
        _ => return,
    };
    if head.websocket {
        websocket(stream, id, hub).await;
    } else {
        let response = answer(&mut stream, &head, &scores).await;
        let _ = stream.write_all(&http::render(&response)).await;
        let _ = stream.shutdown().await;
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

async fn answer(stream: &mut TcpStream, head: &http::Head, scores: &Mutex<Leaderboard>) -> Response {
    // 先把请求头读掉：没读的数据留在缓冲里时关连接会变成 RST，对方收不到响应
    let Some(end) = read_head(stream).await else { return Response { status: 400, body: r#"{"error":"请求头不对"}"#.into() } };
    if head.content_length > MAX_BODY {
        return Response { status: 413, body: r#"{"error":"请求体太大"}"#.into() };
    }
    let mut body = end;
    while body.len() < head.content_length {
        let mut chunk = [0; 1024];
        match tokio::time::timeout(HEAD_TIMEOUT, stream.read(&mut chunk)).await {
            Ok(Ok(n)) if n > 0 => body.extend_from_slice(&chunk[..n]),
            _ => return Response { status: 400, body: r#"{"error":"请求体不完整"}"#.into() },
        }
    }
    body.truncate(head.content_length);
    let mut board = scores.lock().unwrap_or_else(|e| e.into_inner());
    http::route(head, &body, &mut board)
}

/// 把请求头从流里读掉，返回读多了的（请求体的开头）。
async fn read_head(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0; 1024];
    loop {
        let n = stream.read(&mut chunk).await.ok()?;
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

async fn websocket(stream: TcpStream, id: ConnId, hub: UnboundedSender<ToHub>) {
    let config = WebSocketConfig::default().max_message_size(Some(WS_LIMIT)).max_frame_size(Some(WS_LIMIT));
    let Ok(ws) = tokio_tungstenite::accept_async_with_config(stream, Some(config)).await else { return };
    let (tx, mut outbox) = unbounded_channel();
    if hub.send(ToHub::Open(id, tx)).is_err() {
        return;
    }
    let (mut sink, mut source) = ws.split();
    loop {
        tokio::select! {
            message = source.next() => match message {
                Some(Ok(Message::Binary(b))) => { let _ = hub.send(ToHub::Binary(id, b.to_vec())); }
                Some(Ok(Message::Text(_))) => { let _ = hub.send(ToHub::Text(id)); }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            out = outbox.recv() => match out {
                Some(ToConn::Bytes(b)) => {
                    if sink.send(Message::Binary(b.into())).await.is_err() {
                        break;
                    }
                }
                Some(ToConn::Close) | None => {
                    let _ = sink.close().await;
                    break;
                }
            },
        }
    }
    let _ = hub.send(ToHub::Closed(id));
}
