//! 线上的消息格式：客户端 ↔ 中继（`ClientMsg` / `RelayMsg`），客户端之间（`Payload`，放在 `Data.body` 里由中继原样转发）。
//! 都是 serde + postcard：枚举的序号就是消息类型，整数是变长的。解码先看长度，再解，最后校验，坏数据不往外交。

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// 协议版本：`Hello` 里带着，中继对不上就回 `Error(Version)`。
pub const PROTOCOL_VERSION: u16 = 1;
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

/// 客户端发给中继。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// 第一条：进哪个房间、叫什么。
    Hello { version: u16, room: String, name: String },
    /// 转发：`to` 是 `None` 时给房间里的其他人。
    Data { to: Option<PeerId>, body: Vec<u8> },
    Ping,
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

/// 名字、房间名：去掉两头空白，不能是空的、不能有控制字符、不超过 `max` 个字符。
pub fn clean_text(s: &str, max: usize) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control)).then(|| s.to_owned())
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
            ClientMsg::Hello { version: PROTOCOL_VERSION, room: "r".into(), name: "小明".into() },
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
    fn clean_text_trims_and_rejects() {
        assert_eq!(clean_text("  小明 ", 4), Some("小明".into()));
        assert_eq!(clean_text("   ", 4), None);
        assert_eq!(clean_text("a\nb", 4), None);
        assert_eq!(clean_text("五个字的名", 4), None, "按字符数，不按字节");
        assert_eq!(clean_text("四个字名", 4), Some("四个字名".into()));
    }
}
