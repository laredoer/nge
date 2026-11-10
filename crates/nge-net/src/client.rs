//! 客户端状态机：连中继、握手、心跳、断线后按退避重连、维护房间成员。时间由调用方给（秒），连接由注入的 `connector` 建。

use std::collections::BTreeMap;

use crate::backoff::Backoff;
use crate::limit::TokenBucket;
use crate::link::{Connector, Frame, Link, LinkEvent, MAX_MESSAGE};
use crate::proto::{self, ClientMsg, MAX_JSON, MAX_KEY, MAX_NAME, MAX_ROOM, PROTOCOL_VERSION, Payload, Peer, PeerId, RelayMsg};

/// 多久发一次心跳（秒）。
pub const PING_INTERVAL: f64 = 2.0;
/// 多久没收到任何东西算断了（秒）。
pub const SILENCE_TIMEOUT: f64 = 10.0;
/// 用户消息每秒最多几条、最多攒几条。
pub const MESSAGE_RATE: f64 = 30.0;
pub const MESSAGE_BURST: f64 = 30.0;

/// 连到哪：中继地址、房间、自己的名字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub url: String,
    pub room: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    /// 没要连。
    #[default]
    Offline,
    /// 在连，或者断了等着重连。
    Connecting,
    /// 进了房间。
    Online,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    Status(Status),
    Joined(Peer),
    Left(Peer),
    Payload { from: PeerId, payload: Payload },
    /// 断线、中继报错的原因（给日志和界面看）。
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    NotOnline,
    /// 发得太快，这条丢掉了。
    RateLimited,
    /// 类型或数据不合规（太长、有控制字符）。
    Invalid,
}

pub struct NetClient {
    connector: Connector,
    target: Option<Target>,
    link: Option<Box<dyn Link>>,
    status: Status,
    me: Option<PeerId>,
    peers: BTreeMap<PeerId, String>,
    backoff: Backoff,
    /// 断线后什么时候再连；`None` 是马上连。
    retry_at: Option<f64>,
    last_heard: f64,
    last_ping: f64,
    messages: TokenBucket,
    events: Vec<ClientEvent>,
}

impl NetClient {
    pub fn new(connector: Connector) -> Self {
        Self {
            connector,
            target: None,
            link: None,
            status: Status::Offline,
            me: None,
            peers: BTreeMap::new(),
            backoff: Backoff::default().with_seed(seed()),
            retry_at: None,
            last_heard: 0.0,
            last_ping: 0.0,
            messages: TokenBucket::new(MESSAGE_RATE, MESSAGE_BURST),
            events: Vec::new(),
        }
    }

    /// 换退避的设置（测试里去掉抖动）。
    pub fn with_backoff(self, backoff: Backoff) -> Self {
        Self { backoff, ..self }
    }

    /// 要连的目标：`None` 断开；和现在的不一样时断开旧的，下一次 `update` 马上连新的。
    pub fn set_target(&mut self, target: Option<Target>) {
        if target == self.target {
            return;
        }
        self.drop_link();
        self.target = target;
        self.backoff.reset();
        self.retry_at = None;
        self.set_status(if self.target.is_some() { Status::Connecting } else { Status::Offline });
    }

    /// 每帧调一次：收连接上的事件、发心跳、到点重连。返回这段时间的事件。
    pub fn update(&mut self, now: f64) -> Vec<ClientEvent> {
        if let Some(target) = &self.target
            && self.link.is_none()
            && self.retry_at.is_none_or(|at| now >= at)
        {
            self.link = Some((self.connector)(&target.url));
            self.retry_at = None;
            self.last_heard = now;
            self.set_status(Status::Connecting);
        }
        let events = self.link.as_mut().map(|l| l.poll()).unwrap_or_default();
        for event in events {
            match event {
                LinkEvent::Open => self.hello(now),
                LinkEvent::Frame(frame) => {
                    self.last_heard = now;
                    self.receive(frame);
                }
                LinkEvent::Closed(reason) => {
                    self.lost(reason, now);
                    break;
                }
            }
        }
        if self.link.is_some() {
            if now - self.last_heard > SILENCE_TIMEOUT {
                self.lost(format!("{SILENCE_TIMEOUT} 秒没收到中继的消息"), now);
            } else if self.status == Status::Online && now - self.last_ping >= PING_INTERVAL {
                self.last_ping = now;
                self.send(&ClientMsg::Ping);
            }
        }
        std::mem::take(&mut self.events)
    }

    /// 发一个包给房间里的人（`to` 是 `None` 时给所有人）；没在线时返回 false。
    pub fn send_payload(&mut self, payload: &Payload, to: Option<PeerId>) -> bool {
        if self.status != Status::Online {
            return false;
        }
        self.send(&ClientMsg::Data { to, body: proto::encode(payload) });
        true
    }

    /// 发一条用户消息：限频、限长。
    pub fn send_message(&mut self, kind: &str, json: &str, to: Option<PeerId>, now: f64) -> Result<(), SendError> {
        if self.status != Status::Online {
            return Err(SendError::NotOnline);
        }
        if proto::clean_text(kind, MAX_KEY).is_none_or(|k| k != kind) || json.len() > MAX_JSON {
            return Err(SendError::Invalid);
        }
        if !self.messages.take(now) {
            return Err(SendError::RateLimited);
        }
        self.send_payload(&Payload::Message { kind: kind.to_owned(), json: json.to_owned() }, to);
        Ok(())
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn me(&self) -> Option<PeerId> {
        self.me
    }

    pub fn target(&self) -> Option<&Target> {
        self.target.as_ref()
    }

    pub fn peers(&self) -> Vec<Peer> {
        self.peers.iter().map(|(&id, name)| Peer { id, name: name.clone() }).collect()
    }

    pub fn has_peer(&self, id: PeerId) -> bool {
        self.peers.contains_key(&id)
    }

    fn send(&mut self, msg: &ClientMsg) {
        if let Some(link) = &mut self.link {
            link.send(Frame::Binary(proto::encode(msg)));
        }
    }

    fn set_status(&mut self, status: Status) {
        if self.status != status {
            self.status = status;
            self.events.push(ClientEvent::Status(status));
        }
    }

    /// 连上了：报上房间和名字（超长的截断，中继还会再查）。
    fn hello(&mut self, now: f64) {
        let Some(target) = &self.target else { return };
        let cut = |s: &str, max: usize| s.trim().chars().take(max).collect::<String>();
        let hello = ClientMsg::Hello { version: PROTOCOL_VERSION, room: cut(&target.room, MAX_ROOM), name: cut(&target.name, MAX_NAME) };
        self.last_heard = now;
        self.send(&hello);
    }

    fn receive(&mut self, frame: Frame) {
        let bytes = match frame {
            Frame::Binary(b) => b,
            Frame::Text(_) => return log::warn!("中继发来了文本帧，忽略"),
        };
        let msg = match proto::decode_relay(&bytes, MAX_MESSAGE) {
            Ok(m) => m,
            Err(e) => return log::warn!("中继的消息{e}，忽略"),
        };
        match msg {
            RelayMsg::Welcome { you, peers, .. } => {
                self.me = Some(you);
                self.backoff.reset();
                self.peers = peers.iter().filter(|p| p.id != you).map(|p| (p.id, p.name.clone())).collect();
                self.set_status(Status::Online);
                let joined = self.peers();
                self.events.extend(joined.into_iter().map(ClientEvent::Joined));
            }
            RelayMsg::Joined(peer) => {
                if Some(peer.id) != self.me && self.peers.insert(peer.id, peer.name.clone()).is_none() {
                    self.events.push(ClientEvent::Joined(peer));
                }
            }
            RelayMsg::Left(id) => {
                if let Some(name) = self.peers.remove(&id) {
                    self.events.push(ClientEvent::Left(Peer { id, name }));
                }
            }
            RelayMsg::Data { from, body } => {
                if !self.peers.contains_key(&from) {
                    return;
                }
                match proto::decode_payload(&body) {
                    Ok(payload) => self.events.push(ClientEvent::Payload { from, payload }),
                    Err(e) => log::warn!("成员 {from} 发来的消息{e}，忽略"),
                }
            }
            RelayMsg::Pong => {}
            RelayMsg::Error(code) => self.events.push(ClientEvent::Error(format!("中继报错: {code:?}"))),
        }
    }

    /// 丢掉连接，房间里的人都算离开。
    fn drop_link(&mut self) {
        self.link = None;
        self.me = None;
        let left = std::mem::take(&mut self.peers);
        self.events.extend(left.into_iter().map(|(id, name)| ClientEvent::Left(Peer { id, name })));
    }

    /// 断了：按退避等一会儿再连。
    fn lost(&mut self, reason: String, now: f64) {
        self.drop_link();
        self.set_status(Status::Connecting);
        self.events.push(ClientEvent::Error(reason));
        self.retry_at = Some(now + self.backoff.next_delay());
    }
}

/// 退避抖动的种子：每个客户端不一样，免得一起断的又一起连回来。
fn seed() -> u64 {
    let nanos = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let here = &nanos as *const u64 as u64;
    nanos ^ here.rotate_left(32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{ErrorCode, Snapshot, encode};
    use crate::testing::FakeRelay;

    fn client(fake: &FakeRelay) -> NetClient {
        NetClient::new(fake.connector()).with_backoff(Backoff::new(0.5, 10.0).without_jitter())
    }

    fn target(room: &str) -> Target {
        Target { url: "ws://relay".into(), room: room.into(), name: "小明".into() }
    }

    fn bob() -> Peer {
        Peer { id: 2, name: "bob".into() }
    }

    /// 连上并进房间（房间里已经有 bob），时间 0。
    fn online(fake: &FakeRelay) -> NetClient {
        let mut c = client(fake);
        c.set_target(Some(target("r")));
        c.update(0.0);
        fake.push(LinkEvent::Open);
        c.update(0.0);
        fake.relay(RelayMsg::Welcome { you: 1, peers: vec![bob()], max: 8 });
        c.update(0.0);
        c
    }

    #[test]
    fn handshake_sends_hello_and_goes_online_on_welcome() {
        let fake = FakeRelay::default();
        let mut c = client(&fake);
        assert_eq!(c.update(0.0), vec![], "没有目标时什么都不做");
        assert_eq!(fake.count(), 0);
        c.set_target(Some(target("r")));
        assert_eq!(c.update(0.0), vec![ClientEvent::Status(Status::Connecting)]);
        assert_eq!(fake.url(0), "ws://relay");
        fake.push(LinkEvent::Open);
        c.update(0.1);
        assert_eq!(fake.take_sent(), vec![ClientMsg::Hello { version: PROTOCOL_VERSION, room: "r".into(), name: "小明".into() }]);
        fake.relay(RelayMsg::Welcome { you: 1, peers: vec![bob()], max: 8 });
        assert_eq!(c.update(0.2), vec![ClientEvent::Status(Status::Online), ClientEvent::Joined(bob())]);
        assert_eq!((c.status(), c.me(), c.peers()), (Status::Online, Some(1), vec![bob()]));
    }

    #[test]
    fn members_join_and_leave_and_data_from_members_is_delivered() {
        let fake = FakeRelay::default();
        let mut c = online(&fake);
        let carol = Peer { id: 3, name: "carol".into() };
        fake.relay(RelayMsg::Joined(carol.clone()));
        let snapshot = Payload::Snapshot(Snapshot { seq: 1, t: 5, items: vec![] });
        fake.relay(RelayMsg::Data { from: 3, body: encode(&snapshot) });
        // 不在房间里的人发的、解不出来的都不要
        fake.relay(RelayMsg::Data { from: 9, body: encode(&snapshot) });
        fake.relay(RelayMsg::Data { from: 3, body: vec![0xff, 0xff] });
        fake.relay(RelayMsg::Left(2));
        fake.relay(RelayMsg::Left(42));
        assert_eq!(
            c.update(1.0),
            vec![ClientEvent::Joined(carol.clone()), ClientEvent::Payload { from: 3, payload: snapshot }, ClientEvent::Left(bob())]
        );
        assert_eq!(c.peers(), vec![carol]);
    }

    #[test]
    fn a_dropped_connection_reports_everyone_left_and_reconnects_with_backoff() {
        let fake = FakeRelay::default();
        let mut c = online(&fake);
        fake.push(LinkEvent::Closed("断了".into()));
        let events = c.update(1.0);
        assert_eq!(&events[..2], &[ClientEvent::Left(bob()), ClientEvent::Status(Status::Connecting)]);
        assert!(matches!(&events[2], ClientEvent::Error(e) if e.contains("断了")));
        assert!(c.peers().is_empty() && c.me().is_none());
        c.update(1.4);
        assert_eq!(fake.count(), 1, "0.5 秒内不重连");
        c.update(1.5);
        assert_eq!(fake.count(), 2, "到点重连");
        // 又没连上：这次等 1 秒
        fake.push(LinkEvent::Closed("连不上".into()));
        c.update(1.6);
        c.update(2.5);
        assert_eq!(fake.count(), 2);
        c.update(2.6);
        assert_eq!(fake.count(), 3);
        // 连上了：退避归零
        fake.push(LinkEvent::Open);
        fake.relay(RelayMsg::Welcome { you: 5, peers: vec![], max: 8 });
        c.update(2.7);
        assert_eq!(c.status(), Status::Online);
        fake.push(LinkEvent::Closed("又断了".into()));
        c.update(3.0);
        c.update(3.5);
        assert_eq!(fake.count(), 4, "归零后又是 0.5 秒");
    }

    #[test]
    fn pings_every_two_seconds_and_gives_up_after_ten_silent_seconds() {
        let fake = FakeRelay::default();
        let mut c = online(&fake);
        c.update(1.0);
        c.update(2.0);
        c.update(3.9);
        c.update(4.0);
        let pings = fake.take_sent().iter().filter(|m| **m == ClientMsg::Ping).count();
        assert_eq!(pings, 2);
        fake.relay(RelayMsg::Pong);
        c.update(5.0);
        c.update(14.9);
        assert_eq!(c.status(), Status::Online, "5 秒时还收到过 Pong");
        let events = c.update(15.1);
        assert!(events.contains(&ClientEvent::Status(Status::Connecting)), "{events:?}");
        assert!(fake.dropped(fake.count() - 1), "旧连接丢掉了");
    }

    #[test]
    fn a_connection_that_never_opens_times_out_too() {
        let fake = FakeRelay::default();
        let mut c = client(&fake);
        c.set_target(Some(target("r")));
        c.update(0.0);
        c.update(10.5);
        assert!(fake.dropped(fake.count() - 1));
    }

    #[test]
    fn changing_the_room_reconnects_and_none_goes_offline() {
        let fake = FakeRelay::default();
        let mut c = online(&fake);
        c.set_target(Some(target("r")));
        assert_eq!(c.update(1.0), vec![], "同一个目标不重连");
        c.set_target(Some(target("other")));
        let events = c.update(1.0);
        assert_eq!(events, vec![ClientEvent::Left(bob()), ClientEvent::Status(Status::Connecting)]);
        assert_eq!(fake.count(), 2, "马上连新房间");
        assert!(fake.dropped(0));
        fake.push(LinkEvent::Open);
        c.update(1.1);
        assert!(matches!(&fake.take_sent()[0], ClientMsg::Hello { room, .. } if room == "other"));
        c.set_target(None);
        assert_eq!(c.update(1.2), vec![ClientEvent::Status(Status::Offline)]);
        c.update(100.0);
        assert_eq!(fake.count(), 2, "离线不重连");
    }

    #[test]
    fn relay_errors_are_reported() {
        let fake = FakeRelay::default();
        let mut c = client(&fake);
        c.set_target(Some(target("r")));
        c.update(0.0);
        fake.push(LinkEvent::Open);
        fake.relay(RelayMsg::Error(ErrorCode::RoomFull));
        let events = c.update(0.1);
        assert!(matches!(events.last(), Some(ClientEvent::Error(e)) if e.contains("RoomFull")), "{events:?}");
    }

    #[test]
    fn messages_are_rate_limited_and_checked() {
        let fake = FakeRelay::default();
        let mut c = client(&fake);
        assert_eq!(c.send_message("chat", "1", None, 0.0), Err(SendError::NotOnline));
        let mut c = online(&fake);
        let sent = (0..40).filter(|_| c.send_message("chat", "1", None, 1.0).is_ok()).count();
        assert_eq!(sent, MESSAGE_BURST as usize);
        assert_eq!(c.send_message("chat", "1", None, 1.0), Err(SendError::RateLimited));
        assert!(c.send_message("chat", "1", Some(2), 1.1).is_ok(), "过一会儿又能发");
        assert_eq!(c.send_message("", "1", None, 5.0), Err(SendError::Invalid));
        assert_eq!(c.send_message(&"k".repeat(MAX_KEY + 1), "1", None, 5.0), Err(SendError::Invalid));
        assert_eq!(c.send_message("chat", &"x".repeat(MAX_JSON + 1), None, 5.0), Err(SendError::Invalid));
        let last = fake.take_sent().pop().unwrap();
        assert!(matches!(last, ClientMsg::Data { to: Some(2), .. }));
    }

    #[test]
    fn hello_name_and_room_are_trimmed_to_the_limits() {
        let fake = FakeRelay::default();
        let mut c = client(&fake);
        c.set_target(Some(Target { url: "ws://x".into(), room: "r".repeat(100), name: "n".repeat(100) }));
        c.update(0.0);
        fake.push(LinkEvent::Open);
        c.update(0.0);
        let ClientMsg::Hello { room, name, .. } = &fake.take_sent()[0] else { panic!() };
        assert_eq!((room.chars().count(), name.chars().count()), (MAX_ROOM, MAX_NAME));
    }
}
