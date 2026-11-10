//! 原生的连接：每条连接一个线程，里面一个单线程的 tokio 运行时；和引擎之间只用通道。

use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use super::{Frame, Link, LinkEvent, MAX_MESSAGE};

/// 握手最多等多久。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct NativeLink {
    out: UnboundedSender<Frame>,
    inbox: Mutex<mpsc::Receiver<LinkEvent>>,
}

impl NativeLink {
    pub fn connect(url: &str) -> Self {
        let (out, out_rx) = unbounded_channel();
        let (tx, inbox) = mpsc::channel();
        let url = url.to_owned();
        let spawned = std::thread::Builder::new().name("nge-net".into()).spawn({
            let tx = tx.clone();
            move || match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt.block_on(run(url, out_rx, tx)),
                Err(e) => drop(tx.send(LinkEvent::Closed(format!("起不了网络线程的运行时: {e}")))),
            }
        });
        if let Err(e) = spawned {
            let _ = tx.send(LinkEvent::Closed(format!("起不了网络线程: {e}")));
        }
        Self { out, inbox: Mutex::new(inbox) }
    }
}

impl Link for NativeLink {
    fn send(&mut self, frame: Frame) {
        // 线程已经退出（断了）时发不出去，丢掉
        let _ = self.out.send(frame);
    }

    fn poll(&mut self) -> Vec<LinkEvent> {
        match self.inbox.lock() {
            Ok(inbox) => inbox.try_iter().collect(),
            Err(_) => vec![],
        }
    }
}

async fn run(url: String, mut out: UnboundedReceiver<Frame>, tx: mpsc::Sender<LinkEvent>) {
    let config = WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE)).max_frame_size(Some(MAX_MESSAGE));
    let ws = match tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async_with_config(url.as_str(), Some(config), true)).await {
        Ok(Ok((ws, _))) => ws,
        Ok(Err(e)) => return drop(tx.send(LinkEvent::Closed(format!("连不上 {url}: {e}")))),
        Err(_) => return drop(tx.send(LinkEvent::Closed(format!("连 {url} 超时")))),
    };
    if tx.send(LinkEvent::Open).is_err() {
        return;
    }
    let (mut sink, mut stream) = ws.split();
    let reason = loop {
        tokio::select! {
            frame = out.recv() => {
                let message = match frame {
                    Some(Frame::Binary(b)) => Message::Binary(b.into()),
                    Some(Frame::Text(t)) => Message::Text(t.into()),
                    // 引擎那边把连接丢掉了：好好关掉
                    None => {
                        let _ = sink.close().await;
                        return;
                    }
                };
                if let Err(e) = sink.send(message).await {
                    break format!("发送失败: {e}");
                }
            }
            message = stream.next() => {
                let event = match message {
                    Some(Ok(Message::Binary(b))) => LinkEvent::Frame(Frame::Binary(b.to_vec())),
                    Some(Ok(Message::Text(t))) => LinkEvent::Frame(Frame::Text(t.as_str().to_owned())),
                    Some(Ok(Message::Close(frame))) => break frame.map_or_else(|| "对方关闭了连接".to_owned(), |f| format!("对方关闭了连接: {}", f.reason)),
                    // ping / pong 由 tungstenite 自己回
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => break format!("连接出错: {e}"),
                    None => break "连接断了".to_owned(),
                };
                if tx.send(event).is_err() {
                    return;
                }
            }
        }
    };
    let _ = tx.send(LinkEvent::Closed(reason));
}
