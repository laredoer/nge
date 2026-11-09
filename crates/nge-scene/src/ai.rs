//! 精灵的 `chase`：在瓦片地图里绕开墙追目标，状态变了发 `chase` 事件（见 docs/ai.md）。

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_ai::{ChaseChanged, ChaseState, Chaser};
use nge_core::EventKey;
use nge_ecs::Outbox;
use serde::{Deserialize, Serialize};

use crate::protocol::SpriteNode;
use crate::sync::SceneId;

pub const CHASE: EventKey<ChaseEvent> = EventKey::new("chase");

/// `chase`：追哪个节点（id），每秒多少像素，多远能看见，离出发点多远就不追了。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ChaseNode {
    pub target: String,
    #[serde(default = "default_speed")]
    pub speed: f32,
    #[serde(default = "default_sight")]
    pub sight: f32,
    #[serde(default = "default_leash")]
    pub leash: f32,
}

fn default_speed() -> f32 {
    120.0
}

fn default_sight() -> f32 {
    240.0
}

fn default_leash() -> f32 {
    480.0
}

/// `chase` 事件：状态变了。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChaseEvent {
    pub id: String,
    pub state: &'static str,
    pub x: f32,
    pub y: f32,
}

/// 追的目标的场景 id，每帧对一遍。
#[derive(Component, Debug, Clone, PartialEq)]
pub(crate) struct ChaseTarget(String);

/// 位置变了（出发点变了）或者新加了 `chase` 才重新建 `Chaser`；只改了 `chase` 的参数时原地换掉，接着追；别的属性变了不碰；去掉就删。
pub(crate) fn apply_chase(entity: &mut EntityCommands, before: Option<&SpriteNode>, node: &SpriteNode) {
    let placed = before.is_some_and(|b| (b.x, b.y) == (node.x, node.y));
    if placed && before.is_some_and(|b| b.chase == node.chase) {
        return;
    }
    let had = placed && before.is_some_and(|b| b.chase.is_some());
    match &node.chase {
        Some(c) => {
            let valid = |v: f32| v.is_finite() && v >= 0.0;
            if !(valid(c.speed) && valid(c.sight) && valid(c.leash)) {
                log::warn!("节点 {:?} 的 chase 里 speed / sight / leash 要是不小于 0 的数，不追", node.id);
                entity.remove::<(Chaser, ChaseTarget)>();
                return;
            }
            let fresh = Chaser::new(Vec2::new(node.x, node.y), c.speed, c.sight, c.leash);
            entity.insert(ChaseTarget(c.target.clone()));
            if had {
                entity.queue(move |mut e: EntityWorldMut| match e.get_mut::<Chaser>() {
                    Some(mut old) => (old.speed, old.sight, old.leash) = (fresh.speed, fresh.sight, fresh.leash),
                    None => {
                        e.insert(fresh);
                    }
                });
            } else {
                entity.insert(fresh);
            }
        }
        None => {
            entity.remove::<(Chaser, ChaseTarget)>();
        }
    }
}

/// 按场景 id 找目标实体（目标重建了也跟得上）；找不到就是没有目标。
pub(crate) fn resolve_targets(ids: Query<(Entity, &SceneId)>, mut chasers: Query<(&ChaseTarget, &mut Chaser)>) {
    if chasers.is_empty() {
        return;
    }
    let index: HashMap<&str, Entity> = ids.iter().map(|(e, id)| (id.0.as_str(), e)).collect();
    for (target, mut chaser) in &mut chasers {
        let found = index.get(target.0.as_str()).copied();
        if chaser.target != found {
            chaser.target = found;
        }
    }
}

pub(crate) fn send_changes(mut reader: MessageReader<ChaseChanged>, ids: Query<&SceneId>, mut outbox: ResMut<Outbox>) {
    for m in reader.read() {
        let Ok(id) = ids.get(m.entity) else { continue };
        let state = match m.state {
            ChaseState::Idle => "idle",
            ChaseState::Chase => "chase",
            ChaseState::Return => "return",
        };
        outbox.send(CHASE, &ChaseEvent { id: id.0.clone(), state, x: m.at.x, y: m.at.y });
    }
}

#[cfg(test)]
mod tests {
    use nge_ai::NavGrid;
    use nge_render2d::Transform2d;
    use serde_json::{Value, json};

    use super::*;
    use crate::testing::{app, frame};

    /// 一行 6 格的地图（一格 10），中间第 3 格是墙；左上角在 (0, 10)。
    fn scene(ghost: Value) -> Value {
        json!({
            "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 6, "x": 0, "y": 10, "data": [-1, -1, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1], "solid": [0] }],
            "sprites": [{ "id": "hero", "x": 15, "y": 5, "w": 4 }, ghost],
        })
    }

    fn ghost(extra: Value) -> Value {
        let mut g = json!({ "id": "ghost", "x": 5, "y": 5, "w": 8, "chase": { "target": "hero", "speed": 60, "sight": 50 } });
        g.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        g
    }

    fn chaser(app: &mut nge_ecs::App) -> Option<(Chaser, Vec2)> {
        let world = app.world_mut();
        world.query::<(&Chaser, &Transform2d)>().iter(world).next().map(|(c, t)| (c.clone(), t.translation))
    }

    fn hero(app: &mut nge_ecs::App) -> Entity {
        let world = app.world_mut();
        world.query::<(Entity, &SceneId)>().iter(world).find(|(_, id)| id.0 == "hero").unwrap().0
    }

    fn chase_events(events: &[(String, Value)]) -> Vec<Value> {
        events.iter().filter(|(k, _)| k == "chase").map(|(_, v)| v.clone()).collect()
    }

    #[test]
    fn a_chase_node_builds_a_chaser_aimed_at_the_target() {
        let mut app = app();
        frame(&mut app, Some(scene(ghost(json!({})))));
        let (c, _) = chaser(&mut app).expect("有 Chaser");
        assert_eq!((c.speed, c.sight, c.leash, c.home), (60.0, 50.0, 480.0, Vec2::new(5.0, 5.0)), "没写的用默认值");
        frame(&mut app, None);
        let hero = hero(&mut app);
        assert_eq!(chaser(&mut app).unwrap().0.target, Some(hero), "目标按 id 找到");
        let world = app.world_mut();
        let nav = world.query::<&NavGrid>().iter(world).next().expect("瓦片地图有 NavGrid").clone();
        assert_eq!((nav.grid.cols(), nav.grid.rows(), nav.origin, nav.tile), (6, 2, Vec2::new(0.0, 10.0), 10.0));
        assert!(!nav.grid.open((2, 0)) && nav.grid.open((2, 1)), "实心格子挡住");
    }

    #[test]
    fn chases_around_the_wall_and_reports_state_changes() {
        let mut app = app();
        // 测试的一帧是 0.5 秒。先在墙这边看见、开始追
        let mut events = frame(&mut app, Some(scene(ghost(json!({})))));
        for _ in 0..3 {
            events.extend(frame(&mut app, None));
        }
        let e = chase_events(&events);
        assert_eq!(e.len(), 1, "{e:?}");
        assert_eq!((e[0]["id"].as_str(), e[0]["state"].as_str()), (Some("ghost"), Some("chase")));
        // 目标挪到墙后面
        let hero = hero(&mut app);
        app.world_mut().get_mut::<Transform2d>(hero).unwrap().translation = Vec2::new(35.0, 5.0);
        let mut below = false;
        for _ in 0..10 {
            frame(&mut app, None);
            let (_, at) = chaser(&mut app).unwrap();
            below |= at.y < 0.0;
            assert!(!(20.0..30.0).contains(&at.x) || at.y < 0.0, "进了墙 {at}");
        }
        assert!(below, "从墙下面绕过去");
        assert!(chaser(&mut app).unwrap().1.distance(Vec2::new(35.0, 5.0)) < 1.0, "追上了");
    }

    #[test]
    fn only_solid_unshifted_maps_get_a_nav_grid() {
        let mut app = app();
        let map = |extra: Value| {
            let mut m = json!({ "id": "m", "texture": "grid", "tile": 10, "cols": 2, "data": [0, -1], "solid": [0] });
            m.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            json!({ "tilemaps": [m] })
        };
        let grids = |app: &mut nge_ecs::App| {
            let world = app.world_mut();
            world.query::<&NavGrid>().iter(world).count()
        };
        frame(&mut app, Some(map(json!({}))));
        assert_eq!(grids(&mut app), 1);
        frame(&mut app, Some(map(json!({ "solid": [] }))));
        assert_eq!(grids(&mut app), 0, "纯装饰的层");
        frame(&mut app, Some(map(json!({ "parallax": 0.5 }))));
        assert_eq!(grids(&mut app), 0, "视差层");
        frame(&mut app, Some(map(json!({}))));
        frame(&mut app, Some(map(json!({ "cols": 0 }))));
        assert_eq!(grids(&mut app), 0, "改成不合法的，旧的删掉");
    }

    #[test]
    fn other_changes_keep_the_chase_and_removing_it_stops() {
        let mut app = app();
        frame(&mut app, Some(scene(ghost(json!({})))));
        for _ in 0..10 {
            frame(&mut app, None);
        }
        let (moving, _) = chaser(&mut app).unwrap();
        assert_eq!(moving.state(), ChaseState::Chase);
        // 一帧 0.5 秒能走 30：先让它追到离家 50 多的地方停下，改颜色时被拉回出发点的话这一帧走不回来
        let hero = hero(&mut app);
        app.world_mut().get_mut::<Transform2d>(hero).unwrap().translation = Vec2::new(55.0, -5.0);
        for _ in 0..10 {
            frame(&mut app, None);
        }
        let at = chaser(&mut app).unwrap().1;
        assert!(at.distance(Vec2::new(55.0, -5.0)) < 1.0, "{at}");
        let events = frame(&mut app, Some(scene(ghost(json!({ "color": "#ff0000" })))));
        assert_eq!(chase_events(&events), Vec::<Value>::new(), "没有重建、从 Idle 重新开始追");
        let (kept, now) = chaser(&mut app).unwrap();
        assert_eq!(kept.state(), ChaseState::Chase, "改颜色不重置");
        assert_eq!(now, at, "也没拉回出发点（目标就在原地）");
        // 只改参数：原地换掉，接着追，不拉回出发点
        let before = chaser(&mut app).unwrap().1;
        let events = frame(&mut app, Some(scene(ghost(json!({ "color": "#ff0000", "chase": { "target": "hero", "speed": 30, "sight": 50 } })))));
        let (tuned, _) = chaser(&mut app).unwrap();
        assert_eq!((tuned.state(), tuned.speed), (ChaseState::Chase, 30.0));
        assert_eq!(chase_events(&events), Vec::<Value>::new());
        assert_ne!(before, Vec2::new(5.0, 5.0));
        frame(&mut app, Some(scene(ghost(json!({ "chase": null })))));
        assert!(chaser(&mut app).is_none(), "去掉就删");
        frame(&mut app, Some(scene(ghost(json!({ "chase": { "target": "hero", "speed": -1 } })))));
        assert!(chaser(&mut app).is_none(), "参数不对不追");
    }
}
