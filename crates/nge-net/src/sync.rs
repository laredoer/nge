//! 实体同步：`NetSync` 的实体按固定频率把位置广播出去，`Remote` 的实体按收到的快照插值显示。
//! 对方离开、自己断线时，没带 `KeepOnLeave` 的远端实体删掉。

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_render2d::Transform2d;

use crate::client::Status;
use crate::interp::{Clock, InterpBuffer, Sample};
use crate::plugin::Net;
use crate::proto::{Item, MAX_COORD, MAX_ITEMS, Payload, PeerId, Snapshot};

/// 一个远端最多同步几个 key。
pub const MAX_KEYS: usize = 16;

/// 把这个实体的 `Transform2d`（位置、角度）广播给房间里的人，对方按 `key` 认。
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct NetSync {
    pub key: String,
}

/// 房间里成员 `peer` 的 `key` 实体：位置按收到的快照插值。
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub peer: PeerId,
    pub key: String,
}

/// 对方离开、自己断线时不在这里删（场景声明的远端实体由场景去掉）。
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct KeepOnLeave;

#[derive(Default)]
struct PeerState {
    clock: Clock,
    buffers: HashMap<String, InterpBuffer>,
}

/// 每个远端的时钟和插值缓冲，以及自己发快照的序号。
#[derive(Resource, Default)]
pub struct Remotes {
    peers: HashMap<PeerId, PeerState>,
    seq: u32,
    next_send: Option<f64>,
}

impl Remotes {
    /// 本地时间 `local`（毫秒）收到 `from` 的快照。
    pub(crate) fn receive(&mut self, from: PeerId, snapshot: &Snapshot, local: f64) {
        let peer = self.peers.entry(from).or_default();
        peer.clock.observe(local, snapshot.t as f64);
        for item in snapshot.items.iter().take(MAX_ITEMS) {
            if !peer.buffers.contains_key(&item.key) && peer.buffers.len() >= MAX_KEYS {
                continue;
            }
            let sample = Sample { seq: snapshot.seq, t: snapshot.t as f64, pos: [item.x, item.y], rotation: item.rotation };
            peer.buffers.entry(item.key.clone()).or_default().push(sample);
        }
    }

    pub(crate) fn forget(&mut self, peer: PeerId) {
        self.peers.remove(&peer);
    }

    pub(crate) fn clear(&mut self) {
        self.peers.clear();
    }

    /// 本地时间 `local`（毫秒）该把 `peer` 的 `key` 画在哪；还没收到过时是 `None`。
    pub fn state(&mut self, peer: PeerId, key: &str, local: f64, delay: f64) -> Option<crate::interp::State> {
        let state = self.peers.get_mut(&peer)?;
        let at = state.clock.remote_time(local, delay)?;
        state.buffers.get_mut(key)?.sample(at)
    }
}

/// 给 `Remote` 写插值后的位置；对方不在房间里时删掉（带 `KeepOnLeave` 的留着不动）。
pub(crate) fn apply(net: Res<Net>, mut remotes: ResMut<Remotes>, mut entities: Query<(Entity, &Remote, &mut Transform2d, Has<KeepOnLeave>)>, mut commands: Commands) {
    let local = net.now() * 1000.0;
    for (entity, remote, mut t, keep) in &mut entities {
        if !net.client.has_peer(remote.peer) {
            if !keep {
                commands.entity(entity).despawn();
            }
            continue;
        }
        let Some(s) = remotes.state(remote.peer, &remote.key, local, net.delay) else { continue };
        let (pos, rotation) = (Vec2::from(s.pos), s.rotation);
        if t.translation != pos || t.rotation != rotation {
            t.translation = pos;
            t.rotation = rotation;
        }
    }
}

/// 到点了把 `NetSync` 的实体发出去（每秒 `Net::rate` 次）。
pub(crate) fn broadcast(mut net: ResMut<Net>, mut remotes: ResMut<Remotes>, entities: Query<(&NetSync, &Transform2d)>) {
    if net.status() != Status::Online {
        remotes.next_send = None;
        return;
    }
    let now = net.now();
    // 留 1 微秒的余量：累加 1/20 有舍入误差，不该因此晚一帧
    if remotes.next_send.is_some_and(|at| now + 1e-6 < at) {
        return;
    }
    let interval = 1.0 / net.rate.clamp(1.0, 60.0) as f64;
    // 掉帧时不补发，从现在起重新算
    remotes.next_send = Some(match remotes.next_send {
        Some(at) if now - at < interval => at + interval,
        _ => now + interval,
    });
    let mut items: Vec<Item> = entities
        .iter()
        .filter(|(_, t)| t.translation.is_finite() && t.rotation.is_finite() && t.translation.abs().max_element() <= MAX_COORD)
        .map(|(s, t)| Item { key: s.key.clone(), x: t.translation.x, y: t.translation.y, rotation: t.rotation })
        .collect();
    if items.is_empty() {
        return;
    }
    items.sort_by(|a, b| a.key.cmp(&b.key));
    items.truncate(MAX_ITEMS);
    remotes.seq = remotes.seq.wrapping_add(1);
    let snapshot = Snapshot { seq: remotes.seq, t: (now * 1000.0) as u32, items };
    net.client.send_payload(&Payload::Snapshot(snapshot), None);
}

#[cfg(test)]
mod tests {
    use nge_ecs::App;

    use super::*;
    use crate::link::LinkEvent;
    use crate::plugin::tests::{Manual, VIEWPORT, app, online};
    use crate::proto::RelayMsg;
    use crate::testing::FakeRelay;

    fn snapshot(seq: u32, t: u32, x: f32) -> Payload {
        Payload::Snapshot(Snapshot { seq, t, items: vec![Item { key: "player".into(), x, y: 1.0, rotation: 0.0 }] })
    }

    /// 时钟拨到 `ms` 再跑一帧。
    fn at(app: &mut App, clock: &Manual, ms: u64) {
        clock.set(ms);
        app.tick(VIEWPORT, 0.016);
    }

    fn x(app: &App, e: Entity) -> f32 {
        app.world().get::<Transform2d>(e).unwrap().translation.x
    }

    fn snapshots(fake: &FakeRelay) -> Vec<Snapshot> {
        fake.take_payloads()
            .into_iter()
            .filter_map(|(_, p)| match p {
                Payload::Snapshot(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn synced_entities_are_broadcast_twenty_times_a_second_only_while_online() {
        let (fake, clock) = (FakeRelay::default(), Manual::default());
        let mut app = app(&fake, &clock);
        app.world_mut().spawn((NetSync { key: "player".into() }, Transform2d { translation: Vec2::new(3.0, 4.0), rotation: 0.5, ..Default::default() }));
        app.world_mut().spawn(Transform2d::default());
        online(&mut app, &fake);
        // 0 毫秒那次在 online 里发过了
        for ms in (10..=1000).step_by(10) {
            at(&mut app, &clock, ms);
        }
        let sent = snapshots(&fake);
        assert_eq!(sent.len(), 20, "一秒 20 次");
        assert_eq!(sent[0].items, vec![Item { key: "player".into(), x: 3.0, y: 4.0, rotation: 0.5 }]);
        assert!(sent.windows(2).all(|w| w[1].seq == w[0].seq + 1 && w[1].t >= w[0].t + 49), "序号连着，时间隔 50 毫秒");
        app.world_mut().resource_mut::<Net>().disconnect();
        for ms in (1010..2000).step_by(10) {
            at(&mut app, &clock, ms);
        }
        assert!(snapshots(&fake).is_empty(), "离线不发");
    }

    #[test]
    fn non_finite_positions_are_not_sent() {
        let (fake, clock) = (FakeRelay::default(), Manual::default());
        let mut app = app(&fake, &clock);
        app.world_mut().spawn((NetSync { key: "bad".into() }, Transform2d { translation: Vec2::new(f32::NAN, 0.0), ..Default::default() }));
        app.world_mut().spawn((NetSync { key: "far".into() }, Transform2d { translation: Vec2::new(1.0e7, 0.0), ..Default::default() }));
        online(&mut app, &fake);
        at(&mut app, &clock, 100);
        assert!(snapshots(&fake).is_empty());
    }

    /// 在线、有一个 bob 的远端角色；返回实体。
    fn with_remote(app: &mut App, fake: &FakeRelay, keep: bool) -> Entity {
        online(app, fake);
        let mut e = app.world_mut().spawn((Remote { peer: 2, key: "player".into() }, Transform2d { translation: Vec2::new(-5.0, 0.0), ..Default::default() }));
        if keep {
            e.insert(KeepOnLeave);
        }
        e.id()
    }

    #[test]
    fn remotes_are_interpolated_a_hundred_ms_behind() {
        let (fake, clock) = (FakeRelay::default(), Manual::default());
        let mut app = app(&fake, &clock);
        let bob = with_remote(&mut app, &fake, false);
        at(&mut app, &clock, 900);
        assert_eq!(x(&app, bob), -5.0, "还没收到时不动");
        // 对方的时钟比我们慢 1000 毫秒，包路上不花时间
        for (seq, t, x) in [(1, 0, 0.0), (2, 50, 10.0), (3, 100, 20.0)] {
            fake.data(2, &snapshot(seq, t, x));
            at(&mut app, &clock, 1000 + t as u64);
        }
        at(&mut app, &clock, 1175);
        assert_eq!(x(&app, bob), 15.0, "画的是 75 毫秒（对方时钟）那一刻");
        // 晚到的乱序包：插在中间，不往回跳
        fake.data(2, &snapshot(5, 200, 40.0));
        fake.data(2, &snapshot(4, 150, 30.0));
        at(&mut app, &clock, 1250);
        // 晚到的包让时钟偏移往上跟了一点点（几毫秒），所以不是正好 30
        assert!((x(&app, bob) - 30.0).abs() < 1.0, "{}", x(&app, bob));
        // 再没有包：外推 100 毫秒后停住
        at(&mut app, &clock, 1500);
        assert!((x(&app, bob) - 60.0).abs() < 1e-3, "{}", x(&app, bob));
        at(&mut app, &clock, 3000);
        assert!((x(&app, bob) - 60.0).abs() < 1e-3);
    }

    #[test]
    fn local_entities_are_not_moved_by_remote_data() {
        let (fake, clock) = (FakeRelay::default(), Manual::default());
        let mut app = app(&fake, &clock);
        let me = app.world_mut().spawn((NetSync { key: "player".into() }, Transform2d::default())).id();
        online(&mut app, &fake);
        for (seq, t) in [(1, 0), (2, 50), (3, 100)] {
            fake.data(2, &snapshot(seq, t, 99.0));
            at(&mut app, &clock, 1000 + t as u64);
        }
        assert_eq!(x(&app, me), 0.0);
    }

    #[test]
    fn remotes_of_a_peer_who_left_are_removed_unless_the_scene_keeps_them() {
        let (fake, clock) = (FakeRelay::default(), Manual::default());
        let mut app = app(&fake, &clock);
        let gone = with_remote(&mut app, &fake, false);
        let kept = app.world_mut().spawn((Remote { peer: 2, key: "player".into() }, KeepOnLeave, Transform2d::default())).id();
        fake.data(2, &snapshot(1, 0, 7.0));
        at(&mut app, &clock, 1000);
        fake.relay(RelayMsg::Left(2));
        at(&mut app, &clock, 1100);
        assert!(app.world().get_entity(gone).is_err(), "删掉了");
        assert!(app.world().get_entity(kept).is_ok(), "场景的留着");
        assert!(app.world_mut().resource_mut::<Remotes>().state(2, "player", 1100.0, 0.0).is_none(), "缓冲清掉了");
    }

    #[test]
    fn going_offline_removes_remotes() {
        let (fake, clock) = (FakeRelay::default(), Manual::default());
        let mut app = app(&fake, &clock);
        let bob = with_remote(&mut app, &fake, false);
        at(&mut app, &clock, 100);
        assert!(app.world().get_entity(bob).is_ok());
        fake.push(LinkEvent::Closed("断了".into()));
        at(&mut app, &clock, 200);
        assert!(app.world().get_entity(bob).is_err());
    }

    #[test]
    fn a_flood_of_keys_from_one_peer_is_capped() {
        let mut remotes = Remotes::default();
        for i in 0..40 {
            let items = vec![Item { key: format!("k{i}"), x: 0.0, y: 0.0, rotation: 0.0 }];
            remotes.receive(2, &Snapshot { seq: i, t: i * 10, items }, 0.0);
        }
        assert_eq!(remotes.peers[&2].buffers.len(), MAX_KEYS);
    }
}
