//! 房间逻辑（纯逻辑，时间由调用方给，单位秒）：加入、转发、进出通知、心跳超时、人数 / 大小 / 频率上限；
//! 鉴权（配了密钥时 `Hello` 要带 token，见 `nge_net::auth`）、权威房间（`step` 按固定 tick 推进，见 `nge_net::arena`）、
//! 走连接交的成绩（按身份归属）。每个调用返回要发的帧和要断的连接，`server` 照着做。

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use nge_net::arena::delta::InputMsg;
use nge_net::arena::room::ArenaRoom;
use nge_net::auth::{NONCE, Secret};
use nge_net::limit::TokenBucket;
use nge_net::proto::{self, ClientMsg, ErrorCode, HelloAuth, MAX_NAME, MAX_ROOM, Mode, PROTOCOL_VERSION, Peer, PeerId, RelayMsg};

use crate::scores::{Leaderboard, NAME_TAKEN};

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
    /// 同时最多几条连接（WebSocket 和 HTTP 一起算），超出的直接关。
    pub max_conns: usize,
    /// 每条连接最多排几帧待发：排满了当慢客户端踢掉。
    pub outbox: usize,
    /// 鉴权的密钥：有就要求每条连接带 token；`None` 是本机开发用的免鉴权模式（`nge-relay` 要 `--dev` 才给 `None`）。
    pub auth: Option<Secret>,
}

impl Default for Config {
    fn default() -> Self {
        Self { room_size: 8, max_frame: 8 * 1024, rate: 60.0, burst: 120.0, max_dropped: 600, idle_timeout: 10.0, hello_timeout: 5.0, max_conns: 1024, outbox: 256, auth: None }
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
    /// 交成绩用的名字（鉴权时是 token 里的，开发模式是 `Hello` 里的）和账号。
    owner: Option<String>,
    score_name: String,
    nonce: [u8; NONCE],
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
    /// 权威房间：房间名 → 模拟（成员还是记在 `rooms` 里）。
    arenas: BTreeMap<String, ArenaRoom>,
    scores: Arc<Mutex<Leaderboard>>,
    /// `now` 为 0 时的 Unix 秒（判 token 过期）。
    unix_base: u64,
    nonce_seed: u64,
    nonces: u64,
}

impl Hub {
    pub fn new(config: Config) -> Self {
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        Self {
            config,
            conns: HashMap::new(),
            rooms: BTreeMap::new(),
            arenas: BTreeMap::new(),
            scores: Arc::default(),
            unix_base: since.as_secs(),
            nonce_seed: since.as_nanos() as u64,
            nonces: 0,
        }
    }

    /// 改「`now` 为 0 时」的 Unix 秒（测试用）。
    pub fn with_unix_base(self, unix_base: u64) -> Self {
        Self { unix_base, ..self }
    }

    /// 排行榜（HTTP 那边读同一份）。
    pub fn scores(&self) -> Arc<Mutex<Leaderboard>> {
        self.scores.clone()
    }

    /// 新连接（`id` 也是它在房间里的成员 id）：发给它这条连接的随机数。
    pub fn connect(&mut self, id: ConnId, now: f64) -> Vec<Out> {
        let bucket = TokenBucket::new(self.config.rate, self.config.burst);
        self.nonces += 1;
        let nonce = match &self.config.auth {
            Some(secret) => secret.nonce(self.nonce_seed, self.nonces),
            None => {
                let mut n = [0; NONCE];
                n[..8].copy_from_slice(&self.nonces.to_le_bytes());
                n
            }
        };
        self.conns.insert(id, Conn { room: None, name: String::new(), owner: None, score_name: String::new(), nonce, opened: now, last: now, bucket, dropped: 0 });
        vec![reply(id, &RelayMsg::Challenge { nonce, required: self.config.auth.is_some() })]
    }

    /// 过频率限制（二进制帧和文本帧共用一个令牌桶）：`Err` 里是要发的（超频的丢掉不回，连续超出太多踢出）。
    fn admit(&mut self, id: ConnId, now: f64) -> Result<(), Vec<Out>> {
        let max_dropped = self.config.max_dropped;
        let Some(conn) = self.conns.get_mut(&id) else { return Err(vec![]) };
        conn.last = now;
        if !conn.bucket.take(now) {
            conn.dropped += 1;
            return Err(if conn.dropped > max_dropped { self.kick(id, ErrorCode::RateLimited) } else { vec![] });
        }
        conn.dropped = 0;
        Ok(())
    }

    /// 收到一个二进制帧。
    pub fn frame(&mut self, id: ConnId, bytes: &[u8], now: f64) -> Vec<Out> {
        if let Err(out) = self.admit(id, now) {
            return out;
        }
        if bytes.len() > self.config.max_frame {
            return vec![reply(id, &RelayMsg::Error(ErrorCode::TooLarge))];
        }
        match proto::decode_client(bytes, self.config.max_frame) {
            Ok(ClientMsg::Hello { version, room, name, mode, auth }) => self.hello(id, version, &room, &name, mode, auth.as_ref(), now),
            Ok(ClientMsg::Data { to, body }) => self.forward(id, to, body),
            Ok(ClientMsg::Ping) => vec![reply(id, &RelayMsg::Pong)],
            Ok(ClientMsg::Input(input)) => self.input(id, &input),
            Ok(ClientMsg::Score { board, score }) => self.score(id, &board, score),
            Err(_) => vec![reply(id, &RelayMsg::Error(ErrorCode::BadFrame))],
        }
    }

    /// 收到一个文本帧：协议只用二进制。
    pub fn text(&mut self, id: ConnId, now: f64) -> Vec<Out> {
        if let Err(out) = self.admit(id, now) {
            return out;
        }
        vec![reply(id, &RelayMsg::Error(ErrorCode::BadFrame))]
    }

    /// 连接断了。
    pub fn close(&mut self, id: ConnId) -> Vec<Out> {
        let Some(conn) = self.conns.remove(&id) else { return vec![] };
        let Some(room) = conn.room else { return vec![] };
        if let Some(arena) = self.arenas.get_mut(&room) {
            arena.leave(id);
        }
        let members = self.rooms.entry(room.clone()).or_default();
        members.retain(|&m| m != id);
        if members.is_empty() {
            self.rooms.remove(&room);
            self.arenas.remove(&room);
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

    /// 权威房间都走一个 tick（服务端按 `nge_net::arena::TICK_HZ` 调），给每个成员发快照。
    pub fn step(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        for arena in self.arenas.values_mut() {
            out.extend(arena.step().into_iter().map(|(id, snapshot)| reply(id, &RelayMsg::World(snapshot))));
        }
        out
    }

    /// 权威房间（测试、统计用）。
    pub fn arena(&self, room: &str) -> Option<&ArenaRoom> {
        self.arenas.get(room)
    }

    /// 房间里的人（按进来的先后）。
    pub fn members(&self, room: &str) -> Vec<PeerId> {
        self.rooms.get(room).cloned().unwrap_or_default()
    }

    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    fn hello(&mut self, id: ConnId, version: u16, raw_room: &str, name: &str, mode: Mode, auth: Option<&HelloAuth>, now: f64) -> Vec<Out> {
        let Some(conn) = self.conns.get(&id) else { return vec![] };
        if conn.room.is_some() {
            // 已经在房间里了：换房间要重连
            return vec![reply(id, &RelayMsg::Error(ErrorCode::BadFrame))];
        }
        if version != PROTOCOL_VERSION {
            return self.kick(id, ErrorCode::Version);
        }
        // 鉴权：名字和账号都以 token 里的为准
        let (name, owner) = match (&self.config.auth, auth) {
            (None, _) => (name.to_owned(), None),
            (Some(_), None) => return self.kick(id, ErrorCode::Unauthorized),
            (Some(secret), Some(a)) => {
                let unix = self.unix_base.saturating_add(now.max(0.0) as u64);
                match secret.verify(&a.token, &conn.nonce, raw_room, &a.proof, unix) {
                    Ok(claims) => (claims.name, Some(claims.sub)),
                    Err(e) => {
                        log::info!("连接 {id} 鉴权没过: {e}");
                        return self.kick(id, ErrorCode::Unauthorized);
                    }
                }
            }
        };
        let (Some(room), Some(name)) = (proto::clean_text(raw_room, MAX_ROOM), proto::clean_text(&name, MAX_NAME)) else {
            return self.kick(id, ErrorCode::BadHello);
        };
        let members = self.rooms.get(&room).cloned().unwrap_or_default();
        let existing = if self.arenas.contains_key(&room) { Mode::Authority } else { Mode::Relay };
        if !members.is_empty() && existing != mode {
            return self.kick(id, ErrorCode::WrongMode);
        }
        if members.len() >= self.config.room_size {
            return self.kick(id, ErrorCode::RoomFull);
        }
        if mode == Mode::Authority && !self.arenas.entry(room.clone()).or_default().join(id) {
            return self.kick(id, ErrorCode::RoomFull);
        }
        let score_name = name.clone();
        let name = self.unique_name(&members, name);
        let peers = members.iter().filter_map(|m| self.conns.get(m).map(|c| Peer { id: *m, name: c.name.clone() })).collect();
        let max = u16::try_from(self.config.room_size).unwrap_or(u16::MAX);
        let mut out = vec![reply(id, &RelayMsg::Welcome { you: id, peers, max })];
        let joined = proto::encode(&RelayMsg::Joined(Peer { id, name: name.clone() }));
        out.extend(members.iter().map(|&m| Out::Send(m, joined.clone())));
        if let Some(conn) = self.conns.get_mut(&id) {
            conn.room = Some(room.clone());
            conn.name = name;
            conn.owner = owner;
            conn.score_name = score_name;
        }
        self.rooms.entry(room).or_default().push(id);
        out
    }

    fn input(&mut self, id: ConnId, input: &InputMsg) -> Vec<Out> {
        let Some(room) = self.conns.get(&id).and_then(|c| c.room.as_ref()) else {
            return vec![reply(id, &RelayMsg::Error(ErrorCode::NotJoined))];
        };
        match self.arenas.get_mut(room) {
            Some(arena) => {
                arena.input(id, input);
                vec![]
            }
            None => vec![reply(id, &RelayMsg::Error(ErrorCode::WrongMode))],
        }
    }

    /// 交成绩：记在这条连接的身份名下（鉴权时是 token 里的账号和名字）。
    fn score(&mut self, id: ConnId, board: &str, score: f64) -> Vec<Out> {
        let Some(conn) = self.conns.get(&id).filter(|c| c.room.is_some()) else {
            return vec![reply(id, &RelayMsg::Error(ErrorCode::NotJoined))];
        };
        let result = self.scores.lock().unwrap_or_else(|e| e.into_inner()).submit_as(board, conn.owner.as_deref(), &conn.score_name, score);
        match result {
            Ok(rank) => vec![reply(id, &RelayMsg::Ranked { board: board.to_owned(), rank: rank.map(|r| r as u32) })],
            Err(why) if why == NAME_TAKEN => vec![reply(id, &RelayMsg::Error(ErrorCode::NameTaken))],
            Err(_) => vec![reply(id, &RelayMsg::Error(ErrorCode::BadFrame))],
        }
    }

    /// 房间里有人叫这个名字时加编号（`a#2`、`a#3`……），截短了让总长不超过上限。
    fn unique_name(&self, members: &[ConnId], name: String) -> String {
        let taken = |n: &str| members.iter().any(|m| self.conns.get(m).is_some_and(|c| c.name == n));
        if !taken(&name) {
            return name;
        }
        (2..)
            .map(|n| {
                let suffix = format!("#{n}");
                let base: String = name.chars().take(MAX_NAME.saturating_sub(suffix.chars().count())).collect();
                base + &suffix
            })
            .find(|candidate| !taken(candidate))
            .unwrap_or(name)
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
        encode(&ClientMsg::Hello { version: PROTOCOL_VERSION, room: room.into(), name: name.into(), mode: Mode::Relay, auth: None })
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
        let old = encode(&ClientMsg::Hello { version: PROTOCOL_VERSION + 1, room: "r".into(), name: "a".into(), mode: Mode::Relay, auth: None });
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

    #[test]
    fn text_frames_share_the_rate_limit_and_a_text_flood_is_kicked() {
        let mut h = hub();
        join(&mut h, 1, "r", "a");
        join(&mut h, 2, "r", "b");
        // 和二进制帧同一个令牌桶：攒的 4 个用完以后只丢不回
        let replies = (0..10).map(|_| h.text(1, 0.0).len()).sum::<usize>();
        assert_eq!(replies, 4);
        assert_eq!(h.frame(1, &data(None, b"x"), 0.0), vec![], "文本把令牌用光了，二进制也要等");
        let mut last = vec![];
        for _ in 0..30 {
            last = decoded(h.text(1, 0.0));
            if !last.is_empty() {
                break;
            }
        }
        assert_eq!(last, vec![(1, Some(RelayMsg::Error(ErrorCode::RateLimited))), (1, None), (2, Some(RelayMsg::Left(1)))]);
    }

    #[test]
    fn same_names_in_a_room_get_a_number() {
        let mut h = Hub::new(Config { room_size: 8, ..Config::default() });
        join(&mut h, 1, "r", "a");
        assert_eq!(join(&mut h, 2, "r", "a")[1], (1, Some(RelayMsg::Joined(peer(2, "a#2")))));
        assert_eq!(join(&mut h, 3, "r", "a")[1].1, Some(RelayMsg::Joined(peer(3, "a#3"))));
        // 别的房间不算重名
        assert_eq!(join(&mut h, 4, "x", "a"), vec![(4, Some(RelayMsg::Welcome { you: 4, peers: vec![], max: 8 }))]);
        // 长名字截短了再加编号，不超过上限
        let long = "长".repeat(MAX_NAME);
        join(&mut h, 5, "r", &long);
        let got = join(&mut h, 6, "r", &long);
        let Some(RelayMsg::Joined(p)) = &got[1].1 else { panic!("{got:?}") };
        assert_eq!(p.name, format!("{}#2", "长".repeat(MAX_NAME - 2)));
        // 走了的名字空出来
        h.close(2);
        assert_eq!(join(&mut h, 7, "r", "a")[1].1, Some(RelayMsg::Joined(peer(7, "a#2"))));
    }

    const NOW: u64 = 1_800_000_000;

    fn secret() -> Secret {
        Secret::new(b"hub-test-secret-0123456789abcdef".to_vec()).unwrap()
    }

    fn locked() -> Hub {
        Hub::new(Config { room_size: 4, auth: Some(secret()), ..Config::default() }).with_unix_base(NOW)
    }

    fn credential(sub: &str, name: &str, exp: u64) -> nge_net::auth::Credential {
        let claims = nge_net::auth::Claims { sub: sub.into(), name: name.into(), exp };
        nge_net::auth::Credential::parse(&secret().issue(&claims).unwrap()).unwrap()
    }

    /// 连上，拿到随机数。
    fn challenge(h: &mut Hub, id: ConnId) -> [u8; NONCE] {
        match decoded(h.connect(id, 0.0)).as_slice() {
            [(_, Some(RelayMsg::Challenge { nonce, .. }))] => *nonce,
            other => panic!("{other:?}"),
        }
    }

    fn signed_hello(room: &str, mode: Mode, cred: &nge_net::auth::Credential, nonce: &[u8; NONCE]) -> Vec<u8> {
        let auth = HelloAuth { token: cred.token.clone(), proof: cred.prove(nonce, room) };
        encode(&ClientMsg::Hello { version: PROTOCOL_VERSION, room: room.into(), name: "随便写".into(), mode, auth: Some(auth) })
    }

    #[test]
    fn every_connection_gets_its_own_challenge() {
        let mut h = hub();
        let a = decoded(h.connect(1, 0.0));
        let b = decoded(h.connect(2, 0.0));
        assert!(matches!(a[0].1, Some(RelayMsg::Challenge { required: false, .. })), "开发模式不要求鉴权");
        assert_ne!(a, b);
        let mut l = locked();
        assert!(matches!(decoded(l.connect(1, 0.0))[0].1, Some(RelayMsg::Challenge { required: true, .. })));
    }

    #[test]
    fn with_a_secret_only_valid_signed_hellos_get_in() {
        let mut h = locked();
        // 不带 token
        challenge(&mut h, 1);
        assert_eq!(decoded(h.frame(1, &hello("r", "a"), 0.0)), vec![(1, Some(RelayMsg::Error(ErrorCode::Unauthorized))), (1, None)]);
        // 合格的：名字以 token 里的为准
        let cred = credential("acct-1", "小红", NOW + 60);
        let nonce = challenge(&mut h, 2);
        let joined = decoded(h.frame(2, &signed_hello("r", Mode::Relay, &cred, &nonce), 0.0));
        assert!(matches!(joined[0].1, Some(RelayMsg::Welcome { you: 2, .. })), "{joined:?}");
        // 重放：把 2 号的握手原样拿到 3 号连接上发
        challenge(&mut h, 3);
        assert_eq!(decoded(h.frame(3, &signed_hello("r", Mode::Relay, &cred, &nonce), 0.0)), vec![(3, Some(RelayMsg::Error(ErrorCode::Unauthorized))), (3, None)]);
        // 过期：服务端的时间走过了 exp
        let nonce = challenge(&mut h, 4);
        assert_eq!(decoded(h.frame(4, &signed_hello("r", Mode::Relay, &cred, &nonce), 61.0))[0], (4, Some(RelayMsg::Error(ErrorCode::Unauthorized))));
        // 伪造：别的密钥签的
        let fake = nge_net::auth::Credential::parse(&Secret::new(b"not-the-server-secret-0123456789".to_vec()).unwrap().issue(&nge_net::auth::Claims { sub: "acct-1".into(), name: "小红".into(), exp: NOW + 60 }).unwrap()).unwrap();
        let nonce = challenge(&mut h, 5);
        assert_eq!(decoded(h.frame(5, &signed_hello("r", Mode::Relay, &fake, &nonce), 0.0))[0], (5, Some(RelayMsg::Error(ErrorCode::Unauthorized))));
        // 房间里的人看到的是 token 里的名字
        let nonce = challenge(&mut h, 6);
        let other = credential("acct-2", "小明", NOW + 60);
        let joined = decoded(h.frame(6, &signed_hello("r", Mode::Relay, &other, &nonce), 0.0));
        assert_eq!(joined[1], (2, Some(RelayMsg::Joined(peer(6, "小明")))));
        assert_eq!(h.members("r"), vec![2, 6]);
    }

    #[test]
    fn scores_sent_on_the_connection_belong_to_the_account() {
        let mut h = locked();
        let a = credential("acct-1", "小红", NOW + 60);
        let nonce = challenge(&mut h, 1);
        h.frame(1, &signed_hello("r", Mode::Relay, &a, &nonce), 0.0);
        let score = |board: &str, score: f64| encode(&ClientMsg::Score { board: board.into(), score });
        assert_eq!(decoded(h.frame(1, &score("l1", 12.5), 0.1)), vec![(1, Some(RelayMsg::Ranked { board: "l1".into(), rank: Some(1) }))]);
        // 另一个账号拿到了同名的 token：交不了
        let b = credential("acct-2", "小红", NOW + 60);
        let nonce = challenge(&mut h, 2);
        h.frame(2, &signed_hello("r", Mode::Relay, &b, &nonce), 0.0);
        assert_eq!(decoded(h.frame(2, &score("l1", 1.0), 0.1)), vec![(2, Some(RelayMsg::Error(ErrorCode::NameTaken)))]);
        assert_eq!(h.scores().lock().unwrap().top("l1")[0].score, 12.5);
        // 没进房间不能交
        challenge(&mut h, 3);
        assert_eq!(decoded(h.frame(3, &score("l1", 1.0), 0.1)), vec![(3, Some(RelayMsg::Error(ErrorCode::NotJoined)))]);
    }

    #[test]
    fn authority_rooms_step_and_send_snapshots() {
        let mut h = hub();
        let arena = |name: &str| encode(&ClientMsg::Hello { version: PROTOCOL_VERSION, room: "arena".into(), name: name.into(), mode: Mode::Authority, auth: None });
        h.connect(1, 0.0);
        h.connect(2, 0.0);
        assert!(matches!(decoded(h.frame(1, &arena("a"), 0.0))[0].1, Some(RelayMsg::Welcome { .. })));
        h.frame(2, &arena("b"), 0.0);
        let input = encode(&ClientMsg::Input(InputMsg { first: 1, inputs: vec![nge_net::arena::button::RIGHT], ack: None, view: 0 }));
        assert_eq!(h.frame(1, &input, 0.0), vec![], "输入不回、不转发");
        let out = decoded(h.step());
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|(_, m)| matches!(m, Some(RelayMsg::World(w)) if w.tick == 1 && w.entities.len() == 2)), "{out:?}");
        assert!(h.arena("arena").unwrap().world()[&1].vx > 0.0, "按输入走了");
        // 模式对不上：进不去；在中继房间里发输入：报错
        h.connect(3, 0.0);
        assert_eq!(decoded(h.frame(3, &hello("arena", "c"), 0.0)), vec![(3, Some(RelayMsg::Error(ErrorCode::WrongMode))), (3, None)]);
        join(&mut h, 4, "plain", "d");
        assert_eq!(decoded(h.frame(4, &input, 0.0)), vec![(4, Some(RelayMsg::Error(ErrorCode::WrongMode)))]);
        // 走了的角色从世界里去掉，房间空了权威房间也去掉
        h.close(2);
        assert!(!h.arena("arena").unwrap().world().contains_key(&2));
        h.close(1);
        assert!(h.arena("arena").is_none());
        assert!(h.step().is_empty());
    }

}
