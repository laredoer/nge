//! 权威模式接进 ECS：`Predicted` 的实体按自己的输入预测、收到快照对账回滚后平滑修正；
//! 别人的角色按快照插值（和中继模式的 `Remote` 走同一套）；每秒两次把延迟、回滚次数、预测误差发给 TSX。
//! 输入从动作来：`move`（轴，左右）、`jump`、`tag`（见 docs/input-actions.md）。

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_ecs::{Actions, Outbox, Time};
use nge_render2d::Transform2d;
use serde::Serialize;

use crate::arena::client::ArenaClient;
use crate::arena::{TICK_MS, World, button};
use crate::plugin::{NET, Net, NetWire};
use crate::proto::{Item, PeerId, Snapshot};
use crate::sync::Remotes;

/// 权威房间里远端角色的 key：TSX 用 `remoteId(成员, "hero")` 声明。
pub const ARENA_KEY: &str = "hero";
/// 还没量到往返时间时按多少算（秒）。
pub const DEFAULT_RTT: f64 = 0.1;
/// 统计多久发一次（秒）。
pub const STATS_INTERVAL: f64 = 0.5;
/// 轴推过多少算按着左右。
const DEAD_ZONE: f32 = 0.3;

/// 这个实体是自己的角色：位置由预测写。
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct Predicted;

/// 权威模式的客户端状态（在权威房间里在线时才有）。
#[derive(Resource, Default)]
pub struct Arena {
    pub(crate) client: Option<ArenaClient>,
    next_stats: Option<f64>,
}

impl Arena {
    pub fn client(&self) -> Option<&ArenaClient> {
        self.client.as_ref()
    }
}

/// 发给 TSX 的统计（`net` 事件，`kind: "stats"`）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArenaStats {
    /// 心跳量到的往返，毫秒；还没量到是 null。
    pub rtt: Option<f64>,
    pub rollbacks: u32,
    /// 整个重来的次数。
    pub resets: u32,
    /// 最近一次回滚的预测误差（point）。
    pub error: f32,
    pub max_error: f32,
    /// 超前服务端几个 tick。
    pub lead: i64,
    pub tick: u32,
    /// 自己被拍过几次（服务端的数）。
    pub tagged: u16,
    /// 解不出来丢掉的快照。
    pub dropped: u32,
}

/// 动作 → 按键位。
pub fn input_of(actions: &Actions) -> u8 {
    let x = actions.value("move");
    let mut bits = 0;
    if x < -DEAD_ZONE {
        bits |= button::LEFT;
    }
    if x > DEAD_ZONE {
        bits |= button::RIGHT;
    }
    if actions.down("jump") {
        bits |= button::JUMP;
    }
    if actions.down("tag") {
        bits |= button::TAG;
    }
    bits
}

/// 收到的快照解出来以后：别人的角色喂给插值缓冲（时间戳是服务端的 tick）。
pub(crate) fn feed(remotes: &mut Remotes, me: PeerId, tick: u32, world: &World, local_ms: f64) {
    let t = (tick as f64 * TICK_MS) as u32;
    for (&id, hero) in world.iter().filter(|(id, _)| **id != me) {
        let item = Item { key: ARENA_KEY.to_owned(), x: hero.x, y: hero.y, rotation: 0.0 };
        remotes.receive(id, &Snapshot { seq: tick, t, items: vec![item] }, local_ms);
    }
}

/// 每帧：走该走的 tick、发输入、写自己角色的位置、到点发统计。
pub(crate) fn predict(mut net: ResMut<Net>, mut arena: ResMut<Arena>, time: Res<Time>, actions: Option<Res<Actions>>, mut heroes: Query<&mut Transform2d, With<Predicted>>, mut outbox: ResMut<Outbox>) {
    let now = net.now();
    let Some(client) = arena.client.as_mut() else { return };
    let input = actions.map_or(0, |a| input_of(&a));
    let rtt = net.client.rtt();
    if let Some(msg) = client.frame(time.real_delta as f64, input, rtt.unwrap_or(DEFAULT_RTT)) {
        net.client.send_input(&msg);
    }
    if let Some(pos) = client.display() {
        let pos = Vec2::from(pos);
        for mut t in &mut heroes {
            if t.translation != pos {
                t.translation = pos;
            }
        }
    }
    if arena.next_stats.is_some_and(|at| now < at) {
        return;
    }
    arena.next_stats = Some(now + STATS_INTERVAL);
    let Some(client) = arena.client.as_ref() else { return };
    let stats = client.stats();
    let tagged = client.world().and_then(|(_, w)| w.get(&client.me())).map_or(0, |h| h.tagged);
    let wire = ArenaStats {
        rtt: rtt.map(|r| (r * 1000.0).round()),
        rollbacks: stats.rollbacks,
        resets: stats.resets,
        error: stats.last_error,
        max_error: stats.max_error,
        lead: client.lead().unwrap_or(0),
        tick: client.tick().unwrap_or(0),
        tagged,
        dropped: client.dropped,
    };
    outbox.send(NET, &NetWire::Stats(wire));
}

#[cfg(test)]
mod tests {
    use nge_render2d::Transform2d;
    use serde_json::json;

    use super::*;
    use crate::arena::room::ArenaRoom;
    use crate::arena::{HALF_H, spawn};
    use crate::client::Target;
    use crate::link::LinkEvent;
    use crate::plugin::tests::{Manual, VIEWPORT, app, nets};
    use crate::proto::{ClientMsg, Mode, RelayMsg};
    use crate::sync::{KeepOnLeave, NetSync, Remote};
    use crate::testing::FakeRelay;

    #[test]
    fn the_own_hero_is_predicted_others_are_interpolated_and_stats_reach_tsx() {
        let (fake, clock) = (FakeRelay::default(), Manual::default());
        let mut app = app(&fake, &clock);
        let me = app.world_mut().spawn((Transform2d::default(), Predicted, NetSync { key: "hero".into() })).id();
        let other = app.world_mut().spawn((Transform2d::default(), Remote { peer: 2, key: ARENA_KEY.into() }, KeepOnLeave)).id();
        app.world_mut().resource_mut::<Actions>().set_map(controls());
        app.world_mut().resource_mut::<Net>().connect(Target { mode: Mode::Authority, ..Target::relay("ws://relay", "arena", "me") });
        app.tick(VIEWPORT, 0.016);
        fake.push(LinkEvent::Open);
        fake.relay(RelayMsg::Welcome { you: 1, peers: vec![crate::proto::Peer { id: 2, name: "bob".into() }], max: 8 });
        let events = app.tick(VIEWPORT, 0.016);
        let status = nets(&events).into_iter().find(|e| e["kind"] == "status" && e["status"] == "online").unwrap();
        assert_eq!(status["arena"]["width"], json!(crate::arena::WIDTH), "在线时带着竞技场的几何 {status}");
        fake.take_sent();

        let mut room = ArenaRoom::new();
        room.join(1);
        room.join(2);
        // 往右按着：服务端一直收到输入，客户端预测往右跑
        app.command("action", &json!({ "action": "move", "player": 0, "down": true, "x": 1.0, "y": 0.0, "source": "test" }));
        let mut stats = Vec::new();
        for frame in 1..=120u64 {
            clock.set(frame * 1000 / 60);
            if frame % 2 == 0 {
                for m in fake.take_sent() {
                    match m {
                        ClientMsg::Input(input) => room.input(1, &input),
                        ClientMsg::Data { .. } => panic!("权威房间里不广播自己的位置"),
                        _ => {}
                    }
                }
                for (id, s) in room.step() {
                    if id == 1 {
                        fake.relay(RelayMsg::World(s));
                    }
                }
            }
            let events = app.tick(VIEWPORT, 1.0 / 60.0);
            stats.extend(nets(&events).into_iter().filter(|e| e["kind"] == "stats"));
        }
        let shown = app.world().get::<Transform2d>(me).unwrap().translation;
        let server = room.world()[&1];
        assert!(shown.x > spawn(1).x + 100.0, "自己的角色跟着输入往右跑 {shown:?}");
        assert!(shown.x >= server.x, "预测在服务端前面 {shown:?} {server:?}");
        assert_eq!(shown.y, HALF_H);
        let theirs = app.world().get::<Transform2d>(other).unwrap().translation;
        assert_eq!(theirs, Vec2::new(spawn(2).x, HALF_H), "别人的角色按快照插值");
        assert!((4..=9).contains(&stats.len()), "每秒两次统计 {}", stats.len());
        let last = stats.last().unwrap();
        assert_eq!((last["rollbacks"].clone(), last["tagged"].clone()), (json!(0), json!(0)), "{last}");
        assert!(last["tick"].as_u64().unwrap() > 30 && last["lead"].as_i64().unwrap() > 0, "{last}");
    }

    /// 声明了 move / jump / tag 的绑定表。
    fn controls() -> nge_ecs::ActionMap {
        serde_json::from_value(json!({ "players": [{
            "move": { "kind": "axis", "negative": ["ArrowLeft"], "positive": ["ArrowRight"] },
            "jump": { "kind": "button", "keys": ["Space"] },
            "tag": { "kind": "button", "keys": ["KeyJ"] },
        }] }))
        .unwrap()
    }

    #[test]
    fn actions_become_input_bits() {
        let (fake, clock) = (FakeRelay::default(), Manual::default());
        let mut app = app(&fake, &clock);
        app.world_mut().resource_mut::<Actions>().set_map(controls());
        app.tick(VIEWPORT, 0.016);
        assert_eq!(input_of(app.world().resource::<Actions>()), 0);
        let drive = |app: &mut nge_ecs::App, action: &str, x: f32| app.command("action", &json!({ "action": action, "player": 0, "down": true, "x": x, "y": 0.0, "source": "t" }));
        drive(&mut app, "move", -1.0);
        drive(&mut app, "jump", 1.0);
        drive(&mut app, "tag", 1.0);
        app.tick(VIEWPORT, 0.016);
        assert_eq!(input_of(app.world().resource::<Actions>()), button::LEFT | button::JUMP | button::TAG);
        drive(&mut app, "move", 0.2);
        app.tick(VIEWPORT, 0.016);
        assert_eq!(input_of(app.world().resource::<Actions>()) & (button::LEFT | button::RIGHT), 0, "推得太轻不算");
    }
}
