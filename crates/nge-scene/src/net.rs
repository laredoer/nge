//! 场景的 `net`（feature `net` 同时加上 HTTP 的 `http` 通道）：写了就连中继，去掉就断；`sync` 里的精灵广播自己的位置，id 是 `net:<成员>:<key>` 的精灵按远端插值（见 docs/networking.md）。

use serde::Deserialize;

/// 远端精灵 id 的前缀。
pub const REMOTE_PREFIX: &str = "net:";

#[derive(Clone, PartialEq, Deserialize)]
pub struct NetNode {
    /// 中继地址；不写时用环境变量 `NGE_RELAY`，再没有就是 `ws://127.0.0.1:9100`。
    pub url: Option<String>,
    pub room: String,
    /// 自己的名字，默认 "player"。
    pub name: Option<String>,
    /// 广播位置的精灵 id。
    #[serde(default)]
    pub sync: Vec<String>,
    /// 每秒发几次快照，默认 20。
    pub rate: Option<f32>,
    /// 远端显示的延迟（毫秒），默认 100。
    pub delay: Option<f32>,
    /// 房间的模式：`"relay"`（默认，各自广播自己）或 `"authority"`（服务端跑模拟，见 docs/net-more.md）。
    pub mode: Option<String>,
    /// 鉴权的凭据（`nge-token` 签的整串）；不写时原生用环境变量 `NGE_TOKEN`。
    pub token: Option<String>,
    /// 权威模式里自己的角色的精灵 id：位置由预测写。
    pub predict: Option<String>,
}

/// `token` 含会话密钥，`Debug` 不打印。
impl std::fmt::Debug for NetNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetNode")
            .field("url", &self.url)
            .field("room", &self.room)
            .field("name", &self.name)
            .field("sync", &self.sync)
            .field("rate", &self.rate)
            .field("delay", &self.delay)
            .field("mode", &self.mode)
            .field("token", &self.token.as_ref().map(|_| "…"))
            .field("predict", &self.predict)
            .finish()
    }
}

/// `net:<成员>:<key>` → (成员, key)。
pub fn parse_remote(id: &str) -> Option<(u32, &str)> {
    let (peer, key) = id.strip_prefix(REMOTE_PREFIX)?.split_once(':')?;
    let peer = peer.parse().ok()?;
    (!key.is_empty()).then_some((peer, key))
}

#[cfg(feature = "net")]
pub(crate) use enabled::register;

#[cfg(feature = "net")]
mod enabled {
    use bevy_ecs::prelude::*;
    use nge_ecs::{App, PostUpdate, PreUpdate};
    use nge_net::{DEFAULT_URL, HttpPlugin, KeepOnLeave, Mode, Net, NetPlugin, NetSet, NetSync, Predicted, Remote, Target};
    use nge_physics2d::PhysicsStepped;

    use super::{NetNode, parse_remote};
    use crate::protocol::Scene;
    use crate::sync::SceneId;

    pub(crate) fn register(app: &mut App) {
        if !app.is_plugin_added::<NetPlugin>() {
            app.add_plugins(NetPlugin::new());
        }
        if !app.is_plugin_added::<HttpPlugin>() {
            app.add_plugins(HttpPlugin::new());
        }
        // 先设好目标再收：同一帧里就开始连
        app.add_systems(PreUpdate, (connect, tag).chain().run_if(crate::transition::scene_ready).after(crate::sync::sync_sprites).before(NetSet::Receive))
            .configure_sets(
                PostUpdate,
                NetSet::Apply
                    .after(PhysicsStepped)
                    .before(crate::camera::camera_rig)
                    .before(crate::world_ui::place_bars)
                    .before(crate::world_ui::place_bubbles)
                    .before(crate::behave::follow),
            )
            .configure_sets(PostUpdate, NetSet::Broadcast.after(PhysicsStepped));
    }

    fn target(node: &NetNode) -> Target {
        let url = node.url.clone().or_else(|| std::env::var("NGE_RELAY").ok()).unwrap_or_else(|| DEFAULT_URL.to_owned());
        let mode = match node.mode.as_deref() {
            Some("authority") => Mode::Authority,
            None | Some("relay") => Mode::Relay,
            Some(other) => {
                log::warn!("net.mode 只认 relay / authority，{other} 按 relay");
                Mode::Relay
            }
        };
        let token = node.token.clone().or_else(|| std::env::var("NGE_TOKEN").ok()).filter(|t| !t.trim().is_empty());
        Target { url, room: node.room.clone(), name: node.name.clone().unwrap_or_else(|| "player".into()), mode, token }
    }

    /// 按场景的 `net` 连或断，设好频率和延迟。
    fn connect(scene: Res<Scene>, mut net: ResMut<Net>) {
        match &scene.net {
            Some(node) => {
                net.connect(target(node));
                net.rate = node.rate.filter(|r| r.is_finite() && *r > 0.0).unwrap_or(20.0);
                net.delay = node.delay.filter(|d| d.is_finite() && *d >= 0.0).map_or(nge_net::interp::DEFAULT_DELAY, f64::from);
            }
            None => net.disconnect(),
        }
    }

    /// 给 `sync` 里的精灵加 `NetSync`、`predict` 的加 `Predicted`、远端精灵加 `Remote`；不再是的去掉。
    fn tag(scene: Res<Scene>, entities: Query<(Entity, &SceneId, Has<NetSync>, Has<Remote>, Has<Predicted>)>, mut commands: Commands) {
        let sync: &[String] = scene.net.as_ref().map_or(&[], |n| &n.sync);
        let predict = scene.net.as_ref().and_then(|n| n.predict.as_deref());
        for (entity, id, synced, remote, predicted) in &entities {
            let want = predict == Some(id.0.as_str());
            if want && !predicted {
                commands.entity(entity).insert(Predicted);
            } else if !want && predicted {
                commands.entity(entity).remove::<Predicted>();
            }
            let want = sync.contains(&id.0);
            if want && !synced {
                commands.entity(entity).insert(NetSync { key: id.0.clone() });
            } else if !want && synced {
                commands.entity(entity).remove::<NetSync>();
            }
            if !remote && let Some((peer, key)) = parse_remote(&id.0) {
                commands.entity(entity).insert((Remote { peer, key: key.to_owned() }, KeepOnLeave));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_ids_are_parsed() {
        assert_eq!(parse_remote("net:12:player"), Some((12, "player")));
        assert_eq!(parse_remote("net:3:a:b"), Some((3, "a:b")));
        for bad in ["player", "net:", "net:x:player", "net:1:", "net:-1:p", "net:1"] {
            assert_eq!(parse_remote(bad), None, "{bad}");
        }
    }

    #[test]
    fn debug_hides_the_token() {
        let node: NetNode = serde_json::from_value(serde_json::json!({ "room": "r", "token": "v1.body.sig.session-key" })).unwrap();
        let shown = format!("{node:?}");
        assert!(!shown.contains("session-key") && !shown.contains("body"), "{shown}");
        assert!(shown.contains("room"), "别的字段照样打 {shown}");
    }

    #[cfg(feature = "net")]
    mod with_net {
        use bevy_ecs::prelude::*;
        use nge_net::testing::FakeRelay;
        use nge_net::{KeepOnLeave, Net, NetPlugin, NetSync, Remote, Status};
        use serde_json::json;

        use crate::sync::SceneId;
        use crate::testing::{app_with, frame};

        fn find(app: &mut nge_ecs::App, id: &str) -> Entity {
            let world = app.world_mut();
            world.query::<(Entity, &SceneId)>().iter(world).find(|(_, s)| s.0 == id).unwrap().0
        }

        #[test]
        fn the_net_node_connects_tags_sprites_and_disconnects() {
            let fake = FakeRelay::default();
            let mut app = app_with(NetPlugin::new().connector(fake.connector()));
            let scene = json!({
                "net": { "url": "ws://relay:1", "room": "level-1", "name": "甲", "sync": ["me"], "rate": 10 },
                "sprites": [{ "id": "me" }, { "id": "net:7:me", "color": "#ff000080" }, { "id": "box" }],
            });
            frame(&mut app, Some(scene.clone()));
            assert_eq!((fake.count(), fake.url(0)), (1, "ws://relay:1".to_owned()));
            let net = app.world().resource::<Net>();
            assert_eq!((net.target().unwrap().room.as_str(), net.target().unwrap().name.as_str(), net.rate), ("level-1", "甲", 10.0));
            let (me, remote, other) = (find(&mut app, "me"), find(&mut app, "net:7:me"), find(&mut app, "box"));
            assert_eq!(app.world().get::<NetSync>(me), Some(&NetSync { key: "me".into() }));
            assert_eq!(app.world().get::<Remote>(remote), Some(&Remote { peer: 7, key: "me".into() }));
            assert!(app.world().get::<KeepOnLeave>(remote).is_some(), "场景的远端精灵交给场景去掉");
            assert!(app.world().get::<NetSync>(other).is_none() && app.world().get::<Remote>(other).is_none());

            // 换了房间：重连；不再同步 me
            let mut moved = scene.clone();
            moved["net"]["room"] = json!("level-2");
            moved["net"]["sync"] = json!([]);
            frame(&mut app, Some(moved));
            assert!(fake.dropped(0) && fake.count() == 2);
            assert!(app.world().get::<NetSync>(me).is_none());

            frame(&mut app, Some(json!({ "sprites": [{ "id": "me" }] })));
            assert_eq!(app.world().resource::<Net>().status(), Status::Offline, "去掉 net 就断开");
        }

        #[test]
        fn authority_mode_token_and_the_predicted_sprite() {
            use nge_net::{Mode, Predicted};
            let fake = FakeRelay::default();
            let mut app = app_with(NetPlugin::new().connector(fake.connector()));
            let scene = json!({
                "net": { "url": "ws://relay:1", "room": "arena", "mode": "authority", "token": "v1.a.b.c", "predict": "hero" },
                "sprites": [{ "id": "hero" }, { "id": "box" }],
            });
            frame(&mut app, Some(scene.clone()));
            let target = app.world().resource::<Net>().target().cloned().unwrap();
            assert_eq!((target.mode, target.token.as_deref()), (Mode::Authority, Some("v1.a.b.c")));
            let (hero, other) = (find(&mut app, "hero"), find(&mut app, "box"));
            assert!(app.world().get::<Predicted>(hero).is_some() && app.world().get::<Predicted>(other).is_none());
            let mut plain = scene.clone();
            plain["net"]["mode"] = json!("什么");
            plain["net"]["predict"] = json!(null);
            frame(&mut app, Some(plain));
            assert_eq!(app.world().resource::<Net>().target().unwrap().mode, Mode::Relay, "不认得的按 relay");
            assert!(app.world().get::<Predicted>(hero).is_none());
        }

        #[test]
        fn changing_level_and_room_drops_the_old_rooms_remotes() {
            use nge_net::Peer;
            use nge_net::link::LinkEvent;
            use nge_net::proto::RelayMsg;
            let fake = FakeRelay::default();
            let mut app = app_with(NetPlugin::new().connector(fake.connector()));
            let scene = |level: u32, remotes: &[&str]| {
                let mut sprites = vec![json!({ "id": "me" })];
                sprites.extend(remotes.iter().map(|id| json!({ "id": id })));
                json!({ "level": level, "net": { "room": format!("r{level}"), "sync": ["me"] }, "sprites": sprites })
            };
            frame(&mut app, Some(scene(1, &["net:7:me"])));
            fake.push(LinkEvent::Open);
            fake.relay(RelayMsg::Welcome { you: 1, peers: vec![Peer { id: 7, name: "bob".into() }], max: 8 });
            let events = frame(&mut app, Some(scene(1, &["net:7:me"])));
            assert!(app.world().resource::<Net>().peers().iter().any(|p| p.id == 7), "{events:?}");
            // 换关：房间跟着换，旧房间的人都算离开，TSX 那边按成员列表不再声明他的精灵
            let events = frame(&mut app, Some(scene(2, &[])));
            assert!(events.iter().any(|(name, e)| name == "net" && e["kind"] == "leave" && e["peer"]["id"] == 7), "{events:?}");
            assert!(app.world().resource::<Net>().peers().is_empty());
            let world = app.world_mut();
            let left: Vec<String> = world.query::<&SceneId>().iter(world).map(|s| s.0.clone()).collect();
            assert_eq!(left, vec!["me".to_owned()], "上一关的远端精灵去掉了");
            assert!(world.query::<&Remote>().iter(world).next().is_none());
        }

        #[test]
        fn rebuilt_levels_get_tagged_again() {
            let fake = FakeRelay::default();
            let mut app = app_with(NetPlugin::new().connector(fake.connector()));
            let scene = |level: u32| json!({ "level": level, "net": { "room": "r", "sync": ["me"] }, "sprites": [{ "id": "me" }] });
            frame(&mut app, Some(scene(1)));
            frame(&mut app, Some(scene(2)));
            let me = find(&mut app, "me");
            assert!(app.world().get::<NetSync>(me).is_some());
        }
    }
}
