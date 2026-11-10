//! ECS 插件：`Net` 资源管连接，状态、成员进出、消息变成 `NetEvent` 消息，同时发给 TSX 的 `net` 事件；
//! TSX 的 `netSend` command 发消息。实体同步（`NetSync` / `Remote`）见 `sync.rs`。

use std::sync::Mutex;

use bevy_ecs::prelude::*;
use nge_core::{CommandKey, EventKey};
use nge_ecs::{App, Outbox, Plugin, PostUpdate, PreUpdate};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{ClientEvent, NetClient, SendError, Status, Target};
use crate::link::Connector;
use crate::proto::{Payload, Peer, PeerId};
use crate::sync::{self, Remotes};

/// 发给 TSX 的事件。
pub const NET: EventKey<NetWire> = EventKey::new("net");
/// TSX 发消息。
pub const NET_SEND: CommandKey<NetSend> = CommandKey::new("netSend");
/// 不指定时连的中继。
pub const DEFAULT_URL: &str = "ws://127.0.0.1:9100";

/// 系统的先后：`Receive` 在 `PreUpdate`，`Apply`（写远端位置）和 `Broadcast`（发快照）在 `PostUpdate`，
/// 游戏把它们排在物理之后、相机之前。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum NetSet {
    Receive,
    Apply,
    Broadcast,
}

#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
pub struct NetSend {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub to: Option<PeerId>,
}

/// 联网的事件（Rust 用 `MessageReader<NetEvent>` 读）。
#[derive(Message, Debug, Clone, PartialEq)]
pub enum NetEvent {
    Status(Status),
    Joined(Peer),
    Left(Peer),
    Message { from: PeerId, kind: String, data: Value },
    Error(String),
}

/// `net` 事件的 JSON：`kind` 区分。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum NetWire {
    Status { status: &'static str, me: Option<PeerId>, peers: Vec<Peer> },
    Join { peer: Peer },
    Leave { peer: Peer },
    Message { from: PeerId, #[serde(rename = "type")] kind: String, data: Value },
    Error { message: String },
}

type Clock = Box<dyn Fn() -> f64 + Send + Sync>;

/// 联网的状态和设置。
#[derive(Resource)]
pub struct Net {
    pub(crate) client: NetClient,
    clock: Clock,
    /// 每秒发几次快照（默认 20）。
    pub rate: f32,
    /// 远端显示的延迟，毫秒（默认 100）。
    pub delay: f64,
}

impl Net {
    pub fn connect(&mut self, target: Target) {
        self.client.set_target(Some(target));
    }

    pub fn disconnect(&mut self) {
        self.client.set_target(None);
    }

    pub fn target(&self) -> Option<&Target> {
        self.client.target()
    }

    /// 发一条消息（`data` 编成 JSON）给房间里的人，`to` 指定时只给他。
    pub fn send(&mut self, kind: &str, data: &Value, to: Option<PeerId>) -> Result<(), SendError> {
        let now = self.now();
        self.client.send_message(kind, &data.to_string(), to, now)
    }

    pub fn status(&self) -> Status {
        self.client.status()
    }

    pub fn me(&self) -> Option<PeerId> {
        self.client.me()
    }

    pub fn peers(&self) -> Vec<Peer> {
        self.client.peers()
    }

    /// 本地时钟（秒）。
    pub fn now(&self) -> f64 {
        (self.clock)()
    }
}

/// 加上 `Net` 资源、`NetEvent` 消息、TSX 的 `net` 事件和 `netSend` command。
pub struct NetPlugin {
    connector: Mutex<Option<Connector>>,
    clock: Mutex<Option<Clock>>,
}

impl NetPlugin {
    pub fn new() -> Self {
        Self { connector: Mutex::new(None), clock: Mutex::new(None) }
    }

    /// 换掉建连接的函数（测试用假的）。
    pub fn connector(self, connector: Connector) -> Self {
        *self.connector.lock().unwrap_or_else(|e| e.into_inner()) = Some(connector);
        self
    }

    /// 换掉时钟（秒，测试用）。
    pub fn clock(self, clock: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        *self.clock.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(clock));
        self
    }
}

impl Default for NetPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        let connector = self.connector.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_else(|| Box::new(crate::link::connect));
        let clock = self.clock.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_else(|| {
            let start = web_time::Instant::now();
            Box::new(move || start.elapsed().as_secs_f64())
        });
        app.insert_resource(Net { client: NetClient::new(connector), clock, rate: 20.0, delay: crate::interp::DEFAULT_DELAY })
            .init_resource::<Remotes>()
            .add_message::<NetEvent>()
            .add_event(NET)
            .add_command(NET_SEND)
            .add_systems(PreUpdate, (receive, send_commands).chain().in_set(NetSet::Receive))
            .add_systems(PostUpdate, sync::apply.in_set(NetSet::Apply))
            .add_systems(PostUpdate, sync::broadcast.in_set(NetSet::Broadcast).after(NetSet::Apply));
    }
}

fn status_name(status: Status) -> &'static str {
    match status {
        Status::Offline => "offline",
        Status::Connecting => "connecting",
        Status::Online => "online",
    }
}

/// 收连接上的事件：发给 ECS 和 TSX，快照交给插值缓冲。
fn receive(mut net: ResMut<Net>, mut remotes: ResMut<Remotes>, mut events: MessageWriter<NetEvent>, mut outbox: ResMut<Outbox>) {
    let now = net.now();
    for event in net.client.update(now) {
        let (event, wire) = match event {
            ClientEvent::Status(status) => {
                if status != Status::Online {
                    remotes.clear();
                }
                (NetEvent::Status(status), NetWire::Status { status: status_name(status), me: net.me(), peers: net.peers() })
            }
            ClientEvent::Joined(peer) => (NetEvent::Joined(peer.clone()), NetWire::Join { peer }),
            ClientEvent::Left(peer) => {
                remotes.forget(peer.id);
                (NetEvent::Left(peer.clone()), NetWire::Leave { peer })
            }
            ClientEvent::Payload { from, payload: Payload::Snapshot(snapshot) } => {
                remotes.receive(from, &snapshot, now * 1000.0);
                continue;
            }
            ClientEvent::Payload { from, payload: Payload::Message { kind, json } } => match serde_json::from_str::<Value>(&json) {
                Ok(data) => (NetEvent::Message { from, kind: kind.clone(), data: data.clone() }, NetWire::Message { from, kind, data }),
                Err(e) => {
                    log::warn!("成员 {from} 发来的 {kind} 不是合法的 JSON: {e}");
                    continue;
                }
            },
            ClientEvent::Error(message) => {
                log::info!("联网: {message}");
                (NetEvent::Error(message.clone()), NetWire::Error { message })
            }
        };
        events.write(event);
        outbox.send(NET, &wire);
    }
}

fn send_commands(mut net: ResMut<Net>, mut commands: MessageReader<NetSend>) {
    for cmd in commands.read() {
        if let Err(e) = net.send(&cmd.kind, &cmd.data, cmd.to) {
            log::warn!("联网消息 {} 没发出去: {e:?}", cmd.kind);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use nge_core::Viewport;
    use serde_json::json;

    use super::*;
    use crate::link::LinkEvent;
    use crate::proto::{ClientMsg, RelayMsg};
    use crate::testing::FakeRelay;

    pub const VIEWPORT: Viewport = Viewport { size: [100.0, 100.0], pixels: [100, 100] };

    /// 手动拨的时钟（毫秒）。
    #[derive(Clone, Default)]
    pub struct Manual(Arc<AtomicU64>);

    impl Manual {
        pub fn set(&self, ms: u64) {
            self.0.store(ms, Ordering::SeqCst);
        }
    }

    pub fn app(fake: &FakeRelay, clock: &Manual) -> App {
        let mut app = App::new();
        let c = clock.0.clone();
        app.add_plugins(NetPlugin::new().connector(fake.connector()).clock(move || c.load(Ordering::SeqCst) as f64 / 1000.0));
        app
    }

    pub fn target() -> Target {
        Target { url: "ws://relay".into(), room: "r".into(), name: "me".into() }
    }

    pub fn bob() -> Peer {
        Peer { id: 2, name: "bob".into() }
    }

    pub fn nets(events: &[(String, Value)]) -> Vec<Value> {
        events.iter().filter(|(n, _)| n == "net").map(|(_, v)| v.clone()).collect()
    }

    /// 连上、进房间（有 bob）。
    pub fn online(app: &mut App, fake: &FakeRelay) -> Vec<(String, Value)> {
        app.world_mut().resource_mut::<Net>().connect(target());
        let mut events = app.tick(VIEWPORT, 0.016);
        fake.push(LinkEvent::Open);
        fake.relay(RelayMsg::Welcome { you: 1, peers: vec![bob()], max: 8 });
        events.extend(app.tick(VIEWPORT, 0.016));
        fake.take_sent();
        events
    }

    #[test]
    fn connection_state_and_members_reach_tsx_and_the_ecs() {
        let (fake, clock) = (FakeRelay::default(), Manual::default());
        let mut app = app(&fake, &clock);
        assert_eq!(app.world().resource::<Net>().status(), Status::Offline);
        let events = online(&mut app, &fake);
        assert_eq!(
            nets(&events),
            vec![
                json!({ "kind": "status", "status": "connecting", "me": null, "peers": [] }),
                json!({ "kind": "status", "status": "online", "me": 1, "peers": [{ "id": 2, "name": "bob" }] }),
                json!({ "kind": "join", "peer": { "id": 2, "name": "bob" } }),
            ]
        );
        fake.relay(RelayMsg::Left(2));
        let events = app.tick(VIEWPORT, 0.016);
        assert_eq!(nets(&events), vec![json!({ "kind": "leave", "peer": { "id": 2, "name": "bob" } })]);
        let seen: Vec<NetEvent> = app.world_mut().resource_mut::<Messages<NetEvent>>().drain().collect();
        assert!(seen.contains(&NetEvent::Left(bob())), "{seen:?}");
    }

    #[test]
    fn tsx_messages_go_out_as_json_and_come_back_parsed() {
        let (fake, clock) = (FakeRelay::default(), Manual::default());
        let mut app = app(&fake, &clock);
        online(&mut app, &fake);
        app.command("netSend", &json!({ "type": "wave", "data": { "n": 1 } }));
        app.command("netSend", &json!({ "type": "poke", "to": 2 }));
        app.tick(VIEWPORT, 0.016);
        assert_eq!(
            fake.take_payloads(),
            vec![
                (None, Payload::Message { kind: "wave".into(), json: r#"{"n":1}"#.into() }),
                (Some(2), Payload::Message { kind: "poke".into(), json: "null".into() }),
            ]
        );
        fake.data(2, &Payload::Message { kind: "wave".into(), json: r#"{"n":2}"#.into() });
        fake.data(2, &Payload::Message { kind: "wave".into(), json: "{坏的".into() });
        let events = app.tick(VIEWPORT, 0.016);
        assert_eq!(nets(&events), vec![json!({ "kind": "message", "from": 2, "type": "wave", "data": { "n": 2 } })]);
    }

    #[test]
    fn disconnecting_reports_offline_and_everyone_leaving() {
        let (fake, clock) = (FakeRelay::default(), Manual::default());
        let mut app = app(&fake, &clock);
        online(&mut app, &fake);
        app.world_mut().resource_mut::<Net>().disconnect();
        let events = app.tick(VIEWPORT, 0.016);
        assert_eq!(
            nets(&events),
            vec![
                json!({ "kind": "leave", "peer": { "id": 2, "name": "bob" } }),
                json!({ "kind": "status", "status": "offline", "me": null, "peers": [] }),
            ]
        );
        assert!(fake.dropped(0));
        assert!(!fake.take_sent().contains(&ClientMsg::Ping));
    }
}
