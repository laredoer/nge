//! `scene.level`：换了值就是换关，场景整体重建（见 docs/levels.md）。

use bevy_ecs::prelude::*;
use nge_particles::ParticleEmitter;
use nge_text::FloatingText;
use nge_core::EventKey;
use nge_ecs::Outbox;
use serde::{Deserialize, Deserializer, Serialize};

use crate::camera::CameraRig;
use crate::protocol::Scene;
use crate::sync::SceneId;

/// 事件 `levelStart`：换关后新关卡的实体建好的那一帧发给 TSX，在它里面发的命令一定落在新关卡上。
pub const LEVEL_START: EventKey<LevelStart> = EventKey::new("levelStart");

/// 换关（包括第一次声明 `level`）时发出，Rust 系统读到它重置自己记的状态；同时以 `levelStart` 事件发给 TSX。
#[derive(Message, Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LevelStart {
    pub level: String,
}

/// `level` 写字符串或数字都行，按字符串比较。
pub(crate) fn level_id<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Text(String),
        Number(serde_json::Number),
    }
    Ok(Option::<Id>::deserialize(d)?.map(|id| match id {
        Id::Text(s) => s,
        // JS 的数字可能以 1.0 传过来：整数一律写成 "1"
        Id::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
    }))
}

/// 场景变了时、同步之前跑：`level` 和上次见到的不一样就清掉上一关的东西，发 `LevelStart`。
pub(crate) fn switch_level(world: &mut World, mut last: Local<Option<Option<String>>>) {
    let level = world.resource::<Scene>().level.clone();
    let first = last.is_none();
    if last.as_ref() == Some(&level) {
        return;
    }
    *last = Some(level.clone());
    // 第一次见到场景、又没声明关卡：还没有上一关
    if first && level.is_none() {
        return;
    }
    let mut old: Vec<Entity> = world.query_filtered::<Entity, Or<(With<SceneId>, With<FloatingText>)>>().iter(world).collect();
    old.extend(world.query::<(Entity, &ParticleEmitter)>().iter(world).filter(|(_, e)| e.despawn_when_done).map(|(e, _)| e));
    for entity in old {
        // 瓦片之类的子实体跟着删；同一个实体可能出现两次，第二次已经不在了
        if let Ok(e) = world.get_entity_mut(entity) {
            e.despawn();
        }
    }
    world.resource_mut::<CameraRig>().restart();
    if let Some(level) = level {
        world.resource_mut::<Outbox>().send(LEVEL_START, &LevelStart { level: level.clone() });
        world.write_message(LevelStart { level });
    }
}

#[cfg(test)]
mod tests {
    use bevy_ecs::prelude::*;
    use glam::Vec2;
    use nge_particles::ParticleEmitter;
    use nge_render2d::{Camera2d, Transform2d};
    use nge_text::FloatingText;
    use serde_json::json;

    use super::LevelStart;
    use crate::sync::SceneId;
    use crate::testing::{app, frame};
    use crate::Tweens;

    #[derive(Component)]
    struct Marked;

    #[derive(Resource, Default)]
    struct Started(Vec<String>);

    fn player(app: &mut nge_ecs::App) -> (Entity, Transform2d, bool) {
        let world = app.world_mut();
        let (e, _, t, m) = world.query::<(Entity, &SceneId, &Transform2d, Has<Marked>)>().iter(world).find(|(_, id, ..)| id.0 == "p").unwrap();
        (e, *t, m)
    }

    fn mark_and_move(app: &mut nge_ecs::App) {
        let (e, ..) = player(app);
        let mut entity = app.world_mut().entity_mut(e);
        entity.insert(Marked);
        entity.get_mut::<Transform2d>().unwrap().translation.x = 50.0;
    }

    #[test]
    fn same_level_keeps_entities_and_a_new_level_rebuilds_them() {
        let mut app = app();
        let scene = |level: serde_json::Value| json!({ "level": level, "sprites": [{ "id": "p", "w": 10 }, { "id": "q", "w": 10, "spin": 90 }] });
        frame(&mut app, Some(scene(json!(1))));
        mark_and_move(&mut app);
        let (before, ..) = player(&mut app);
        frame(&mut app, Some(scene(json!("1"))));
        let (same, t, marked) = player(&mut app);
        assert_eq!((same, t.translation.x, marked), (before, 50.0, true), "关卡号没变（数字 1 和字符串 \"1\" 一样）：照常增量同步");
        frame(&mut app, Some(scene(json!(2))));
        let (rebuilt, t, marked) = player(&mut app);
        assert!(rebuilt != before && t.translation.x == 0.0 && !marked, "换关：同 id、同声明也重建");
        let world = app.world_mut();
        assert_eq!(world.query::<&SceneId>().iter(world).count(), 2, "旧实体都删掉了");
    }

    #[test]
    fn a_new_level_clears_one_shot_effects_and_old_tweens() {
        let mut app = app();
        let scene = |level: u32| json!({ "level": level, "sprites": [{ "id": "p", "w": 10 }], "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 2, "data": [0, 1] }] });
        frame(&mut app, Some(scene(1)));
        app.command("burst", &json!({ "x": 0, "y": 0, "count": 5, "lifetime": [10, 10] }));
        app.command("floatText", &json!({ "x": 0, "y": 0, "text": "+1", "duration": 10 }));
        app.command("tween", &json!({ "id": "p", "to": { "x": 100 }, "duration": 10 }));
        frame(&mut app, None);
        // 换关和新补间同一帧到达：新补间留下
        app.prop("scene", Some(&scene(2)));
        app.command("tween", &json!({ "id": "p", "to": { "y": 100 }, "duration": 10 }));
        frame(&mut app, None);
        let world = app.world_mut();
        assert_eq!(world.query::<&FloatingText>().iter(world).count(), 0);
        assert_eq!(world.query::<&ParticleEmitter>().iter(world).count(), 0);
        let tweens: Vec<Tweens> = world.query::<&Tweens>().iter(world).cloned().collect();
        assert_eq!(tweens.len(), 1);
        assert_eq!(tweens[0].0.len(), 1, "只剩新关卡的补间");
        let p = world.query::<(&SceneId, &Transform2d)>().iter(world).find(|(id, _)| id.0 == "p").map(|(_, t)| t.translation).unwrap();
        assert_eq!(p.x, 0.0, "旧补间（往 x = 100 走）没有带过来");
        assert_eq!(world.query_filtered::<(), With<bevy_ecs::hierarchy::ChildOf>>().iter(world).count(), 2, "瓦片只有新的两块");
    }

    #[test]
    fn level_start_is_sent_once_per_level() {
        let mut app = app();
        app.world_mut().init_resource::<Started>();
        app.add_systems(nge_ecs::Update, |mut reader: MessageReader<LevelStart>, mut started: ResMut<Started>| {
            started.0.extend(reader.read().map(|m| m.level.clone()));
        });
        frame(&mut app, Some(json!({})));
        frame(&mut app, Some(json!({ "level": "a" })));
        frame(&mut app, Some(json!({ "level": "a", "background": "#000000" })));
        frame(&mut app, None);
        frame(&mut app, Some(json!({ "level": 3 })));
        assert_eq!(app.world().resource::<Started>().0, ["a", "3"]);
    }

    /// TSX 也收到 `levelStart`，在新实体建好的那一帧之后：这时发的补间一定落在新关卡上。
    #[test]
    fn tsx_hears_level_start_after_the_rebuild() {
        let mut app = app();
        let events = |e: Vec<(String, serde_json::Value)>| e.into_iter().filter(|(n, _)| n == "levelStart").map(|(_, v)| v).collect::<Vec<_>>();
        assert_eq!(events(frame(&mut app, Some(json!({ "sprites": [{ "id": "p" }] })))), Vec::<serde_json::Value>::new());
        assert_eq!(events(frame(&mut app, Some(json!({ "level": "2-1", "sprites": [{ "id": "p" }] })))), [json!({ "level": "2-1" })]);
        // 事件发出时新关卡的实体已经在了
        player(&mut app);
        assert_eq!(events(frame(&mut app, Some(json!({ "level": "2-1", "sprites": [{ "id": "p" }] })))), Vec::<serde_json::Value>::new());
    }

    #[test]
    fn whole_numbers_name_the_same_level_however_they_are_written() {
        let mut app = app();
        app.world_mut().init_resource::<Started>();
        app.add_systems(nge_ecs::Update, |mut reader: MessageReader<LevelStart>, mut started: ResMut<Started>| {
            started.0.extend(reader.read().map(|m| m.level.clone()));
        });
        frame(&mut app, Some(json!({ "level": 1.0 })));
        frame(&mut app, Some(json!({ "level": 1 })));
        frame(&mut app, Some(json!({ "level": "1" })));
        frame(&mut app, Some(json!({ "level": 1.5 })));
        assert_eq!(app.world().resource::<Started>().0, ["1", "1.5"]);
    }

    #[test]
    fn a_new_level_stops_the_shake() {
        let mut app = app();
        let scene = |level: u32| json!({ "level": level, "camera": { "x": 0, "y": 0 } });
        frame(&mut app, Some(scene(1)));
        app.command("shake", &json!({ "strength": 50, "duration": 10 }));
        frame(&mut app, None);
        frame(&mut app, Some(scene(2)));
        frame(&mut app, None);
        let world = app.world_mut();
        let cam = world.query::<&Camera2d>().single(world).unwrap();
        assert_eq!(cam.position, Vec2::ZERO, "新关卡不接着抖");
    }

    #[test]
    fn the_camera_jumps_to_the_target_in_a_new_level() {
        let mut app = app();
        let scene = |level: u32, x: f32| json!({ "level": level, "camera": { "follow": "p", "smooth": 5 }, "sprites": [{ "id": "p", "x": x }] });
        frame(&mut app, Some(scene(1, 0.0)));
        frame(&mut app, Some(scene(2, 500.0)));
        let world = app.world_mut();
        let cam = world.query::<&Camera2d>().single(world).unwrap();
        assert_eq!(cam.position, Vec2::new(500.0, 0.0), "不从上一关的位置平滑过去");
    }
}
