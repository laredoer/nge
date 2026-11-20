//! 线上的消息格式：客户端 ↔ 中继（`ClientMsg` / `RelayMsg`），客户端之间（`Payload`，放在 `Data.body` 里由中继原样转发）。
//! 都是 serde + postcard：枚举的序号就是消息类型，整数是变长的。解码先看长度，再解，最后校验，坏数据不往外交。

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::arena::delta::{InputMsg, WorldSnapshot, validate_input, validate_snapshot};
use crate::auth::{MAX_TOKEN, NONCE, PROOF};

/// 协议版本：`Hello` 里带着，中继对不上就回 `Error(Version)`。
pub const PROTOCOL_VERSION: u16 = 2;
/// 名字最多几个字符。
pub const MAX_NAME: usize = 32;
/// 房间名最多几个字符。
pub const MAX_ROOM: usize = 64;
/// 快照条目的 key、消息类型最多几个字符。
pub const MAX_KEY: usize = 32;
/// 一个快照最多几条。
pub const MAX_ITEMS: usize = 16;
/// 用户消息的 JSON 最多几个字节。
pub const MAX_JSON: usize = 4096;
/// 坐标的绝对值上限。
pub const MAX_COORD: f32 = 1.0e6;
/// `Welcome` 里最多几个成员。
pub const MAX_PEERS: usize = 64;

pub type PeerId = u32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub id: PeerId,
    pub name: String,
}

/// 房间的模式：中继只转发（各自广播自己），权威由服务端跑模拟（客户端只发输入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Relay,
    Authority,
}

/// `Hello` 里的鉴权：token 和对这条连接的随机数、房间名的证明（见 `auth`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloAuth {
    pub token: String,
    pub proof: Vec<u8>,
}

/// 客户端发给中继。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// 第一条：进哪个房间、叫什么、什么模式；服务端要鉴权时带上 token。
    Hello { version: u16, room: String, name: String, mode: Mode, auth: Option<HelloAuth> },
    /// 转发：`to` 是 `None` 时给房间里的其他人。
    Data { to: Option<PeerId>, body: Vec<u8> },
    Ping,
    /// 权威房间里的输入。
    Input(InputMsg),
    /// 交成绩：记在这条连接的身份（鉴权时是账号）名下。
    Score { board: String, score: f64 },
}

/// 中继发给客户端。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RelayMsg {
    /// 进了房间：自己的 id、已经在里面的人、房间人数上限。
    Welcome { you: PeerId, peers: Vec<Peer>, max: u16 },
    Joined(Peer),
    Left(PeerId),
    Data { from: PeerId, body: Vec<u8> },
    Pong,
    Error(ErrorCode),
    /// 连上就发：这条连接的随机数，`required` 时 `Hello` 要带鉴权。
    Challenge { nonce: [u8; NONCE], required: bool },
    /// 权威房间的快照。
    World(WorldSnapshot),
    /// 交的成绩排第几（没进榜、不比以前好是 `None`）。
    Ranked { board: String, rank: Option<u32> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    /// 协议版本对不上。
    Version,
    /// `Hello` 的房间名或名字不合规。
    BadHello,
    RoomFull,
    /// 帧超过大小上限。
    TooLarge,
    /// 解不出来，或者是文本帧。
    BadFrame,
    /// 还没 `Hello` 就发了别的。
    NotJoined,
    /// 太久没收到东西。
    Timeout,
    /// 发得太快。
    RateLimited,
    /// 要鉴权却没带、token 过期 / 伪造 / 重放。
    Unauthorized,
    /// 房间已经是另一种模式，或者在中继房间里发了输入。
    WrongMode,
    /// 名字归别的账号了（交成绩时）。
    NameTaken,
    /// 排行榜的名字归属表满了，新名字交不了成绩。
    ScoresFull,
    /// 这个账号建的新榜够多了。
    TooManyBoards,
}

/// 客户端之间的消息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Payload {
    Snapshot(Snapshot),
    /// TSX 发的消息：类型和 JSON 文本。
    Message { kind: String, json: String },
}

/// 一个客户端的实体快照。`t` 是发送方的时钟（毫秒），`seq` 每发一次加一。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub seq: u32,
    pub t: u32,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub key: String,
    pub x: f32,
    pub y: f32,
    /// 弧度。
    pub rotation: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    TooLarge(usize),
    Malformed(String),
    Invalid(&'static str),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge(n) => write!(f, "消息太大（{n} 字节）"),
            Self::Malformed(e) => write!(f, "解不出来: {e}"),
            Self::Invalid(why) => write!(f, "数据不合规: {why}"),
        }
    }
}

impl std::error::Error for DecodeError {}

pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    // 协议类型都是普通数据结构，序列化进 Vec 不会失败
    postcard::to_stdvec(msg).unwrap_or_default()
}

pub fn decode_client(bytes: &[u8], max: usize) -> Result<ClientMsg, DecodeError> {
    let msg: ClientMsg = decode(bytes, max)?;
    // Hello 的房间名、名字由中继按自己的规则清理（不合规回 BadHello），这里只管结构
    match &msg {
        ClientMsg::Hello { auth: Some(a), .. } => check(a.token.len() <= MAX_TOKEN && a.proof.len() <= PROOF, "鉴权太长")?,
        ClientMsg::Input(m) => validate_input(m).map_err(DecodeError::Invalid)?,
        ClientMsg::Score { board, score } => check(board.len() <= 4 * MAX_ROOM && score.is_finite(), "成绩不合规")?,
        _ => {}
    }
    Ok(msg)
}

pub fn decode_relay(bytes: &[u8], max: usize) -> Result<RelayMsg, DecodeError> {
    let msg: RelayMsg = decode(bytes, max)?;
    match &msg {
        RelayMsg::Welcome { peers, .. } => {
            check(peers.len() <= MAX_PEERS, "成员太多")?;
            peers.iter().try_for_each(check_peer)?;
        }
        RelayMsg::Joined(peer) => check_peer(peer)?,
        RelayMsg::World(w) => validate_snapshot(w).map_err(DecodeError::Invalid)?,
        RelayMsg::Ranked { board, .. } => check(board.len() <= 4 * MAX_ROOM, "榜名太长")?,
        _ => {}
    }
    Ok(msg)
}

pub fn decode_payload(bytes: &[u8]) -> Result<Payload, DecodeError> {
    let payload: Payload = decode(bytes, usize::MAX)?;
    match &payload {
        Payload::Snapshot(s) => {
            check(s.items.len() <= MAX_ITEMS, "快照条目太多")?;
            for item in &s.items {
                check(is_key(&item.key), "快照的 key 不合规")?;
                check(item.x.is_finite() && item.y.is_finite() && item.rotation.is_finite(), "坐标不是有限的数")?;
                check(item.x.abs() <= MAX_COORD && item.y.abs() <= MAX_COORD, "坐标越界")?;
            }
        }
        Payload::Message { kind, json } => {
            check(is_key(kind), "消息类型不合规")?;
            check(json.len() <= MAX_JSON, "消息太长")?;
        }
    }
    Ok(payload)
}

/// 名字、房间名：去掉两头空白，不能是空的、不能有控制字符（Cc）和看不见的格式字符（Cf：零宽、方向控制、BOM 等，
/// 能拿来做出看着和别人一样的名字），不超过 `max` 个字符。
pub fn clean_text(s: &str, max: usize) -> Option<String> {
    use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};
    let s = s.trim();
    let hidden = |c: char| c.is_control() || c.general_category() == GeneralCategory::Format;
    (!s.is_empty() && s.chars().count() <= max && !s.chars().any(hidden)).then(|| s.to_owned())
}

fn is_key(s: &str) -> bool {
    clean_text(s, MAX_KEY).is_some_and(|c| c == s)
}

fn check_peer(peer: &Peer) -> Result<(), DecodeError> {
    check(clean_text(&peer.name, MAX_NAME).is_some(), "成员名字不合规")
}

fn check(ok: bool, why: &'static str) -> Result<(), DecodeError> {
    if ok { Ok(()) } else { Err(DecodeError::Invalid(why)) }
}

fn decode<T: DeserializeOwned>(bytes: &[u8], max: usize) -> Result<T, DecodeError> {
    if bytes.len() > max {
        return Err(DecodeError::TooLarge(bytes.len()));
    }
    let (msg, rest) = postcard::take_from_bytes(bytes).map_err(|e| DecodeError::Malformed(e.to_string()))?;
    if !rest.is_empty() {
        return Err(DecodeError::Malformed(format!("后面多了 {} 字节", rest.len())));
    }
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(items: Vec<Item>) -> Payload {
        Payload::Snapshot(Snapshot { seq: 7, t: 1234, items })
    }

    fn item(key: &str, x: f32, y: f32) -> Item {
        Item { key: key.into(), x, y, rotation: 0.5 }
    }

    #[test]
    fn every_message_roundtrips() {
        let client = [
            ClientMsg::Hello { version: PROTOCOL_VERSION, room: "r".into(), name: "小明".into(), mode: Mode::Relay, auth: None },
            ClientMsg::Hello { version: PROTOCOL_VERSION, room: "r".into(), name: "n".into(), mode: Mode::Authority, auth: Some(HelloAuth { token: "v1.a.b".into(), proof: vec![1; PROOF] }) },
            ClientMsg::Input(InputMsg { first: 9, inputs: vec![1, 2], ack: Some(8), view: 5 }),
            ClientMsg::Score { board: "b".into(), score: 1.5 },
            ClientMsg::Data { to: Some(3), body: vec![1, 2, 3] },
            ClientMsg::Data { to: None, body: vec![] },
            ClientMsg::Ping,
        ];
        for m in client {
            assert_eq!(decode_client(&encode(&m), 1024), Ok(m));
        }
        let relay = [
            RelayMsg::Welcome { you: 1, peers: vec![Peer { id: 2, name: "b".into() }], max: 8 },
            RelayMsg::Joined(Peer { id: 3, name: "c".into() }),
            RelayMsg::Left(3),
            RelayMsg::Data { from: 2, body: vec![9] },
            RelayMsg::Pong,
            RelayMsg::Error(ErrorCode::RoomFull),
            RelayMsg::Error(ErrorCode::Unauthorized),
            RelayMsg::Challenge { nonce: [3; NONCE], required: true },
            RelayMsg::World(WorldSnapshot { tick: 3, base: Some(2), entities: vec![], removed: vec![1] }),
            RelayMsg::Ranked { board: "b".into(), rank: Some(2) },
        ];
        for m in relay {
            assert_eq!(decode_relay(&encode(&m), 1024), Ok(m));
        }
        let p = snapshot(vec![item("player", 1.5, -2.0)]);
        assert_eq!(decode_payload(&encode(&p)), Ok(p));
        let m = Payload::Message { kind: "chat".into(), json: r#"{"a":1}"#.into() };
        assert_eq!(decode_payload(&encode(&m)), Ok(m));
    }

    #[test]
    fn snapshots_are_compact() {
        // 一个角色的快照：比同样内容的 JSON 小得多
        let bytes = encode(&snapshot(vec![item("player", 123.25, 45.5)]));
        assert!(bytes.len() < 30, "{} 字节", bytes.len());
    }

    #[test]
    fn oversized_frames_are_rejected_before_parsing() {
        let m = ClientMsg::Data { to: None, body: vec![0; 100] };
        let bytes = encode(&m);
        assert_eq!(decode_client(&bytes, 50), Err(DecodeError::TooLarge(bytes.len())));
        assert!(decode_client(&bytes, bytes.len()).is_ok(), "正好等于上限可以");
    }

    #[test]
    fn garbage_and_truncated_frames_are_errors() {
        assert!(matches!(decode_client(&[0xff, 0xff, 0xff], 1024), Err(DecodeError::Malformed(_))));
        let bytes = encode(&RelayMsg::Welcome { you: 1, peers: vec![Peer { id: 2, name: "bob".into() }], max: 8 });
        assert!(decode_relay(&bytes[..bytes.len() - 2], 1024).is_err());
        assert!(decode_payload(&[]).is_err());
        // 后面多出来的字节也不行
        let mut extra = encode(&ClientMsg::Ping);
        extra.push(0);
        assert!(decode_client(&extra, 1024).is_err());
    }

    #[test]
    fn non_finite_and_out_of_range_coordinates_are_rejected() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 2.0e6, -2.0e6] {
            assert!(matches!(decode_payload(&encode(&snapshot(vec![item("p", bad, 0.0)]))), Err(DecodeError::Invalid(_))), "x = {bad}");
            assert!(matches!(decode_payload(&encode(&snapshot(vec![item("p", 0.0, bad)]))), Err(DecodeError::Invalid(_))), "y = {bad}");
        }
        let mut spin = item("p", 0.0, 0.0);
        spin.rotation = f32::NAN;
        assert!(decode_payload(&encode(&snapshot(vec![spin]))).is_err());
    }

    #[test]
    fn too_many_items_and_long_strings_are_rejected() {
        let many = (0..=MAX_ITEMS).map(|i| item(&format!("k{i}"), 0.0, 0.0)).collect();
        assert!(decode_payload(&encode(&snapshot(many))).is_err());
        assert!(decode_payload(&encode(&snapshot(vec![item(&"k".repeat(MAX_KEY + 1), 0.0, 0.0)]))).is_err());
        assert!(decode_payload(&encode(&snapshot(vec![item("", 0.0, 0.0)]))).is_err(), "空 key");
        let long = Payload::Message { kind: "k".into(), json: "x".repeat(MAX_JSON + 1) };
        assert!(decode_payload(&encode(&long)).is_err());
        let kind = Payload::Message { kind: "\u{7}".into(), json: "1".into() };
        assert!(decode_payload(&encode(&kind)).is_err(), "控制字符");
        let peers = (0..=MAX_PEERS as u32).map(|id| Peer { id, name: "p".into() }).collect();
        assert!(decode_relay(&encode(&RelayMsg::Welcome { you: 0, peers, max: 8 }), 1 << 20).is_err());
        let joined = RelayMsg::Joined(Peer { id: 1, name: "n".repeat(MAX_NAME + 1) });
        assert!(decode_relay(&encode(&joined), 1024).is_err());
    }

    #[test]
    fn bad_auth_inputs_and_scores_are_rejected() {
        let hello = |token: String, proof: Vec<u8>| encode(&ClientMsg::Hello { version: PROTOCOL_VERSION, room: "r".into(), name: "n".into(), mode: Mode::Relay, auth: Some(HelloAuth { token, proof }) });
        assert!(decode_client(&hello("t".repeat(MAX_TOKEN + 1), vec![]), 1 << 16).is_err(), "token 太长");
        assert!(decode_client(&hello("t".into(), vec![0; PROOF + 1]), 1 << 16).is_err(), "证明太长");
        assert!(decode_client(&hello("t".into(), vec![0; PROOF]), 1 << 16).is_ok());
        let input = ClientMsg::Input(InputMsg { first: 1, inputs: vec![0xff], ack: None, view: 0 });
        assert!(decode_client(&encode(&input), 1 << 16).is_err(), "不认得的按键位");
        for bad in [f64::NAN, f64::INFINITY] {
            assert!(decode_client(&encode(&ClientMsg::Score { board: "b".into(), score: bad }), 1 << 16).is_err());
        }
    }

    #[test]
    fn clean_text_trims_and_rejects() {
        assert_eq!(clean_text("  小明 ", 4), Some("小明".into()));
        assert_eq!(clean_text("   ", 4), None);
        assert_eq!(clean_text("a\nb", 4), None);
        assert_eq!(clean_text("五个字的名", 4), None, "按字符数，不按字节");
        assert_eq!(clean_text("四个字名", 4), Some("四个字名".into()));
        // 看不见的格式字符（零宽、方向控制、BOM）：能拿来冒充别人，不收
        for sneaky in ["小\u{200b}红", "\u{202e}abc", "a\u{feff}", "a\u{2066}b", "a\u{00ad}b"] {
            assert_eq!(clean_text(sneaky, 8), None, "{sneaky:?}");
        }
    }
}
