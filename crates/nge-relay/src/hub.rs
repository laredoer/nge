//! 房间逻辑（纯逻辑，时间由调用方给，单位秒）：加入、转发、进出通知、心跳超时、人数 / 大小 / 频率上限。
//! 每个调用返回要发的帧和要断的连接，`server` 照着做。

use std::collections::{BTreeMap, HashMap};

use nge_net::limit::TokenBucket;
use nge_net::proto::{self, ClientMsg, ErrorCode, MAX_NAME, MAX_ROOM, PROTOCOL_VERSION, Peer, PeerId, RelayMsg};

pub type ConnId = u32;

#[derive(Debug, Clone)]
pub struct Config {
    /// 一个房间最多几个人。
    pub room_size: usize,
    /// 一帧最多几个字节。
    pub max_frame: usize,
    /// 每条连接每秒几帧、最多攒几帧。
    pub rate: f64,
    pub burst: f64,
    /// 连续超出频率多少帧就断开。
    pub max_dropped: u32,
    /// 进了房间以后多久没收到东西就踢出。
    pub idle_timeout: f64,
    /// 连上以后多久之内要发 `Hello`。
    pub hello_timeout: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self { room_size: 8, max_frame: 8 * 1024, rate: 60.0, burst: 120.0, max_dropped: 600, idle_timeout: 10.0, hello_timeout: 5.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    Send(ConnId, Vec<u8>),
    /// 断开这条连接（`Hub` 已经把它忘了）。
    Close(ConnId),
}

struct Conn {
    room: Option<String>,
    name: String,
    opened: f64,
    last: f64,
    bucket: TokenBucket,
    dropped: u32,
}

pub struct Hub {
    config: Config,
    conns: HashMap<ConnId, Conn>,
    /// 房间名 → 成员（按进来的先后）。
    rooms: BTreeMap<String, Vec<ConnId>>,
}

impl Hub {
    pub fn new(config: Config) -> Self {
        Self { config, conns: HashMap::new(), rooms: BTreeMap::new() }
    }

    /// 新连接（`id` 也是它在房间里的成员 id）。
    pub fn connect(&mut self, id: ConnId, now: f64) {
        let bucket = TokenBucket::new(self.config.rate, self.config.burst);
        self.conns.insert(id, Conn { room: None, name: String::new(), opened: now, last: now, bucket, dropped: 0 });
    }

    /// 收到一个二进制帧。
    pub fn frame(&mut self, id: ConnId, bytes: &[u8], now: f64) -> Vec<Out> {
        let max_dropped = self.config.max_dropped;
        let Some(conn) = self.conns.get_mut(&id) else { return vec![] };
        conn.last = now;
        if !conn.bucket.take(now) {
            conn.dropped += 1;
            return if conn.dropped > max_dropped { self.kick(id, ErrorCode::RateLimited) } else { vec![] };
        }
        conn.dropped = 0;
        if bytes.len() > self.config.max_frame {
            return vec![reply(id, &RelayMsg::Error(ErrorCode::TooLarge))];
        }
        match proto::decode_client(bytes, self.config.max_frame) {
            Ok(ClientMsg::Hello { version, room, name }) => self.hello(id, version, &room, &name),
            Ok(ClientMsg::Data { to, body }) => self.forward(id, to, body),
            Ok(ClientMsg::Ping) => vec![reply(id, &RelayMsg::Pong)],
            Err(_) => vec![reply(id, &RelayMsg::Error(ErrorCode::BadFrame))],
        }
    }

    /// 收到一个文本帧：协议只用二进制。
    pub fn text(&mut self, id: ConnId, now: f64) -> Vec<Out> {
        let Some(conn) = self.conns.get_mut(&id) else { return vec![] };
        conn.last = now;
        vec![reply(id, &RelayMsg::Error(ErrorCode::BadFrame))]
    }

    /// 连接断了。
    pub fn close(&mut self, id: ConnId) -> Vec<Out> {
        let Some(conn) = self.conns.remove(&id) else { return vec![] };
        let Some(room) = conn.room else { return vec![] };
        let members = self.rooms.entry(room.clone()).or_default();
        members.retain(|&m| m != id);
        if members.is_empty() {
            self.rooms.remove(&room);
            return vec![];
        }
        let left = proto::encode(&RelayMsg::Left(id));
        members.iter().map(|&m| Out::Send(m, left.clone())).collect()
    }

    /// 定时调：踢掉超时的。
    pub fn tick(&mut self, now: f64) -> Vec<Out> {
        let mut expired: Vec<(ConnId, bool)> = self
            .conns
            .iter()
            .filter_map(|(&id, c)| match c.room {
                None => (now - c.opened > self.config.hello_timeout).then_some((id, false)),
                Some(_) => (now - c.last > self.config.idle_timeout).then_some((id, true)),
            })
            .collect();
        expired.sort();
        let mut out = Vec::new();
        for (id, joined) in expired {
            if joined {
                out.extend(self.kick(id, ErrorCode::Timeout));
            } else {
                self.conns.remove(&id);
                out.push(Out::Close(id));
            }
        }
        out
    }

    /// 房间里的人（按进来的先后）。
    pub fn members(&self, room: &str) -> Vec<PeerId> {
        self.rooms.get(room).cloned().unwrap_or_default()
    }

    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    fn hello(&mut self, id: ConnId, version: u16, room: &str, name: &str) -> Vec<Out> {
        if self.conns.get(&id).is_some_and(|c| c.room.is_some()) {
            // 已经在房间里了：换房间要重连
            return vec![reply(id, &RelayMsg::Error(ErrorCode::BadFrame))];
        }
        if version != PROTOCOL_VERSION {
            return self.kick(id, ErrorCode::Version);
        }
        let (Some(room), Some(name)) = (proto::clean_text(room, MAX_ROOM), proto::clean_text(name, MAX_NAME)) else {
            return self.kick(id, ErrorCode::BadHello);
        };
        let members = self.rooms.get(&room).cloned().unwrap_or_default();
        if members.len() >= self.config.room_size {
            return self.kick(id, ErrorCode::RoomFull);
        }
        let peers = members.iter().filter_map(|m| self.conns.get(m).map(|c| Peer { id: *m, name: c.name.clone() })).collect();
        let max = u16::try_from(self.config.room_size).unwrap_or(u16::MAX);
        let mut out = vec![reply(id, &RelayMsg::Welcome { you: id, peers, max })];
        let joined = proto::encode(&RelayMsg::Joined(Peer { id, name: name.clone() }));
        out.extend(members.iter().map(|&m| Out::Send(m, joined.clone())));
        if let Some(conn) = self.conns.get_mut(&id) {
            conn.room = Some(room.clone());
            conn.name = name;
        }
        self.rooms.entry(room).or_default().push(id);
        out
    }

    fn forward(&mut self, id: ConnId, to: Option<PeerId>, body: Vec<u8>) -> Vec<Out> {
        let Some(room) = self.conns.get(&id).and_then(|c| c.room.as_ref()) else {
            return vec![reply(id, &RelayMsg::Error(ErrorCode::NotJoined))];
        };
        let members = self.rooms.get(room).map(Vec::as_slice).unwrap_or_default();
        let bytes = proto::encode(&RelayMsg::Data { from: id, body });
        members.iter().filter(|&&m| m != id && to.is_none_or(|t| t == m)).map(|&m| Out::Send(m, bytes.clone())).collect()
    }

    /// 报个错再断开，房间里的人收到离开。
    fn kick(&mut self, id: ConnId, code: ErrorCode) -> Vec<Out> {
        let mut out = vec![reply(id, &RelayMsg::Error(code)), Out::Close(id)];
        out.extend(self.close(id));
        out
    }
}

fn reply(id: ConnId, msg: &RelayMsg) -> Out {
    Out::Send(id, proto::encode(msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nge_net::proto::encode;

    fn hub() -> Hub {
        Hub::new(Config { room_size: 3, max_frame: 256, rate: 10.0, burst: 5.0, max_dropped: 20, ..Config::default() })
    }

    fn hello(room: &str, name: &str) -> Vec<u8> {
        encode(&ClientMsg::Hello { version: PROTOCOL_VERSION, room: room.into(), name: name.into() })
    }

    fn data(to: Option<PeerId>, body: &[u8]) -> Vec<u8> {
        encode(&ClientMsg::Data { to, body: body.to_vec() })
    }

    /// 解出发给谁的什么，断开的记成 `None`。
    fn decoded(out: Vec<Out>) -> Vec<(ConnId, Option<RelayMsg>)> {
        out.into_iter()
            .map(|o| match o {
                Out::Send(id, b) => (id, Some(proto::decode_relay(&b, usize::MAX).unwrap())),
                Out::Close(id) => (id, None),
            })
            .collect()
    }

    fn join(h: &mut Hub, id: ConnId, room: &str, name: &str) -> Vec<(ConnId, Option<RelayMsg>)> {
        h.connect(id, 0.0);
        decoded(h.frame(id, &hello(room, name), 0.0))
    }

    fn peer(id: PeerId, name: &str) -> Peer {
        Peer { id, name: name.into() }
    }

    #[test]
    fn joining_welcomes_you_and_tells_the_others() {
        let mut h = hub();
        assert_eq!(join(&mut h, 1, "r", "a"), vec![(1, Some(RelayMsg::Welcome { you: 1, peers: vec![], max: 3 }))]);
        assert_eq!(
            join(&mut h, 2, "r", " b "),
            vec![(2, Some(RelayMsg::Welcome { you: 2, peers: vec![peer(1, "a")], max: 3 })), (1, Some(RelayMsg::Joined(peer(2, "b"))))]
        );
        // 别的房间互不打扰
        assert_eq!(join(&mut h, 3, "other", "c"), vec![(3, Some(RelayMsg::Welcome { you: 3, peers: vec![], max: 3 }))]);
        assert_eq!(h.members("r"), vec![1, 2]);
    }

    #[test]
    fn data_is_forwarded_to_the_others_or_to_one() {
        let mut h = hub();
        join(&mut h, 1, "r", "a");
        join(&mut h, 2, "r", "b");
        join(&mut h, 3, "r", "c");
        join(&mut h, 4, "x", "d");
        let all = decoded(h.frame(1, &data(None, b"hi"), 0.1));
        let body = Some(RelayMsg::Data { from: 1, body: b"hi".to_vec() });
        assert_eq!(all, vec![(2, body.clone()), (3, body.clone())]);
        assert_eq!(decoded(h.frame(1, &data(Some(3), b"hi"), 0.1)), vec![(3, body)]);
        assert_eq!(decoded(h.frame(1, &data(Some(4), b"hi"), 0.1)), vec![], "不在同一个房间的不给");
        assert_eq!(decoded(h.frame(1, &data(Some(1), b"hi"), 0.1)), vec![], "不发给自己");
    }

    #[test]
    fn leaving_tells_the_others_and_empty_rooms_go_away() {
        let mut h = hub();
        join(&mut h, 1, "r", "a");
        join(&mut h, 2, "r", "b");
        assert_eq!(decoded(h.close(1)), vec![(2, Some(RelayMsg::Left(1)))]);
        assert_eq!(decoded(h.close(1)), vec![], "关两次不重复报");
        assert_eq!(h.room_count(), 1);
        h.close(2);
        assert_eq!(h.room_count(), 0);
    }

    #[test]
    fn a_full_room_turns_you_away() {
        let mut h = hub();
        for (id, name) in [(1, "a"), (2, "b"), (3, "c")] {
            join(&mut h, id, "r", name);
        }
        assert_eq!(join(&mut h, 4, "r", "d"), vec![(4, Some(RelayMsg::Error(ErrorCode::RoomFull))), (4, None)]);
        assert_eq!(h.members("r"), vec![1, 2, 3]);
        // 走了一个就能进
        h.close(2);
        assert!(matches!(join(&mut h, 5, "r", "e")[0], (5, Some(RelayMsg::Welcome { .. }))));
    }

    #[test]
    fn bad_hellos_are_turned_away() {
        let mut h = hub();
        h.connect(1, 0.0);
        let old = encode(&ClientMsg::Hello { version: PROTOCOL_VERSION + 1, room: "r".into(), name: "a".into() });
        assert_eq!(decoded(h.frame(1, &old, 0.0)), vec![(1, Some(RelayMsg::Error(ErrorCode::Version))), (1, None)]);
        for (room, name) in [("", "a"), ("r", "  "), ("r", "a\u{0}"), (&*"r".repeat(MAX_ROOM + 1), "a"), ("r", &*"n".repeat(MAX_NAME + 1))] {
            h.connect(2, 0.0);
            assert_eq!(decoded(h.frame(2, &hello(room, name), 0.0)), vec![(2, Some(RelayMsg::Error(ErrorCode::BadHello))), (2, None)], "{room:?} {name:?}");
        }
        assert_eq!(h.room_count(), 0);
    }

    #[test]
    fn data_before_hello_is_refused_and_ping_gets_pong() {
        let mut h = hub();
        h.connect(1, 0.0);
        assert_eq!(decoded(h.frame(1, &data(None, b"x"), 0.0)), vec![(1, Some(RelayMsg::Error(ErrorCode::NotJoined)))]);
        assert_eq!(decoded(h.frame(1, &encode(&ClientMsg::Ping), 0.0)), vec![(1, Some(RelayMsg::Pong))]);
        assert_eq!(decoded(h.frame(1, &[0xff, 0xff, 0xff], 0.0)), vec![(1, Some(RelayMsg::Error(ErrorCode::BadFrame)))]);
        assert_eq!(decoded(h.text(1, 0.0)), vec![(1, Some(RelayMsg::Error(ErrorCode::BadFrame)))]);
    }

    #[test]
    fn oversized_frames_are_refused_but_the_connection_stays() {
        let mut h = hub();
        join(&mut h, 1, "r", "a");
        join(&mut h, 2, "r", "b");
        assert_eq!(decoded(h.frame(1, &data(None, &[0; 300]), 0.1)), vec![(1, Some(RelayMsg::Error(ErrorCode::TooLarge)))]);
        assert_eq!(decoded(h.frame(1, &data(None, &[0; 10]), 0.1)).len(), 1, "之后照常转发");
    }

    #[test]
    fn floods_are_dropped_and_a_long_flood_is_kicked() {
        let mut h = hub();
        join(&mut h, 1, "r", "a");
        join(&mut h, 2, "r", "b");
        // 攒了 5 个（Hello 用掉了 1 个）：同一时刻只放过 4 个
        let forwarded = (0..10).map(|_| h.frame(1, &data(None, b"x"), 0.0).len()).sum::<usize>();
        assert_eq!(forwarded, 4);
        // 过 0.1 秒补一个
        assert_eq!(h.frame(1, &data(None, b"x"), 0.1).len(), 1);
        // 一直猛发：连续丢掉 20 个以后踢出，房间里的人收到离开
        let mut last = vec![];
        for _ in 0..30 {
            last = decoded(h.frame(1, &data(None, b"x"), 0.1));
            if !last.is_empty() {
                break;
            }
        }
        assert_eq!(last, vec![(1, Some(RelayMsg::Error(ErrorCode::RateLimited))), (1, None), (2, Some(RelayMsg::Left(1)))]);
        assert_eq!(h.members("r"), vec![2]);
    }

    #[test]
    fn silent_members_and_never_joining_connections_time_out() {
        let mut h = hub();
        join(&mut h, 1, "r", "a");
        join(&mut h, 2, "r", "b");
        h.connect(3, 0.0);
        assert_eq!(decoded(h.tick(4.9)), vec![]);
        assert_eq!(decoded(h.tick(5.1)), vec![(3, None)], "5 秒不 Hello 断开");
        h.frame(2, &encode(&ClientMsg::Ping), 8.0);
        assert_eq!(decoded(h.tick(10.0)), vec![]);
        assert_eq!(decoded(h.tick(10.1)), vec![(1, Some(RelayMsg::Error(ErrorCode::Timeout))), (1, None), (2, Some(RelayMsg::Left(1)))]);
        assert_eq!(decoded(h.tick(18.1)).len(), 2, "2 号 8 秒时说过话，18 秒后踢");
        assert_eq!(h.room_count(), 0);
    }
}
