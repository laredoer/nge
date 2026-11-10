//! 测试用的假连接：不走网络，测试直接往里塞中继的消息、看客户端发了什么。

use std::sync::{Arc, Mutex};

use crate::link::{Connector, Frame, Link, LinkEvent};
use crate::proto::{self, ClientMsg, Payload, RelayMsg};

#[derive(Default)]
pub struct Wire {
    pub sent: Vec<Frame>,
    pub incoming: Vec<LinkEvent>,
    pub dropped: bool,
}

struct FakeLink(Arc<Mutex<Wire>>);

impl Link for FakeLink {
    fn send(&mut self, frame: Frame) {
        lock(&self.0).sent.push(frame);
    }
    fn poll(&mut self) -> Vec<LinkEvent> {
        std::mem::take(&mut lock(&self.0).incoming)
    }
}

impl Drop for FakeLink {
    fn drop(&mut self) {
        lock(&self.0).dropped = true;
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 假中继：每建一条连接记下地址和它的线。
#[derive(Clone, Default)]
pub struct FakeRelay(Arc<Mutex<Vec<(String, Arc<Mutex<Wire>>)>>>);

impl FakeRelay {
    pub fn connector(&self) -> Connector {
        let links = self.0.clone();
        Box::new(move |url: &str| {
            let wire = Arc::new(Mutex::new(Wire::default()));
            lock(&links).push((url.to_owned(), wire.clone()));
            Box::new(FakeLink(wire)) as Box<dyn Link>
        })
    }

    /// 建过几条连接。
    pub fn count(&self) -> usize {
        lock(&self.0).len()
    }

    pub fn url(&self, i: usize) -> String {
        lock(&self.0)[i].0.clone()
    }

    pub fn dropped(&self, i: usize) -> bool {
        lock(&lock(&self.0)[i].1).dropped
    }

    fn wire(&self) -> Arc<Mutex<Wire>> {
        lock(&self.0).last().expect("还没有连接").1.clone()
    }

    /// 往最后一条连接里塞一个事件。
    pub fn push(&self, event: LinkEvent) {
        lock(&self.wire()).incoming.push(event);
    }

    pub fn relay(&self, msg: RelayMsg) {
        self.push(LinkEvent::Frame(Frame::Binary(proto::encode(&msg))));
    }

    /// 别人发来的包。
    pub fn data(&self, from: u32, payload: &Payload) {
        self.relay(RelayMsg::Data { from, body: proto::encode(payload) });
    }

    /// 最后一条连接上客户端发过的消息（取走）。
    pub fn take_sent(&self) -> Vec<ClientMsg> {
        let wire = self.wire();
        let frames = std::mem::take(&mut lock(&wire).sent);
        frames
            .iter()
            .filter_map(|f| match f {
                Frame::Binary(b) => proto::decode_client(b, usize::MAX).ok(),
                Frame::Text(_) => None,
            })
            .collect()
    }

    /// 取走发过的包（`Data` 里的）。
    pub fn take_payloads(&self) -> Vec<(Option<u32>, Payload)> {
        self.take_sent()
            .into_iter()
            .filter_map(|m| match m {
                ClientMsg::Data { to, body } => proto::decode_payload(&body).ok().map(|p| (to, p)),
                _ => None,
            })
            .collect()
    }
}
