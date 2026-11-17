use nge_anim::AnimGraph;
use nge_ecs::App;
use nge_render2d::{Sprite, SpriteImage};
use nge_render3d::Animator;
use serde_json::{Value as Json, json};

use super::*;
use crate::anim::FrameAnimation;
use crate::testing::{VIEWPORT, app};

/// idle（0、1 两帧）和 walk（4、5、6），speed > 1 时走；walk 的第 5 帧（0.1 秒）有 step 事件。
fn hero() -> Json {
    json!({
        "params": { "speed": 0, "jump": "trigger" },
        "clips": { "idle": { "frames": [0, 1], "fps": 10 }, "walk": { "frames": [4, 5, 6], "fps": 10, "events": [{ "t": 0.1, "name": "step" }] } },
        "start": "idle",
        "states": { "idle": { "clip": "idle", "to": [{ "to": "walk", "when": "speed > 1" }] }, "walk": { "clip": "walk" } },
    })
}

fn sprite(extra: Json) -> Json {
    let mut node = json!({ "id": "hero", "texture": "grid", "frame": 7 });
    node.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    json!({ "sprites": [node] })
}

fn declare(app: &mut App, scene: Json) {
    app.prop("scene", Some(&scene));
}

fn tick(app: &mut App, dt: f32) -> Vec<(String, Json)> {
    app.tick(VIEWPORT, dt)
}

fn entity(app: &mut App, id: &str) -> Entity {
    let world = app.world_mut();
    world.query::<(Entity, &SceneId)>().iter(world).find(|(_, s)| s.0 == id).unwrap().0
}

fn frame(app: &mut App) -> u32 {
    let e = entity(app, "hero");
    match app.world().get::<Sprite>(e).unwrap().image {
        SpriteImage::Atlas(_, f) => f,
        SpriteImage::Texture(_) => panic!("不是图集"),
    }
}

fn graph(app: &mut App) -> Option<AnimGraph> {
    let e = entity(app, "hero");
    app.world().get::<AnimGraph>(e).cloned()
}

fn state(app: &mut App) -> String {
    graph(app).unwrap().state(0).unwrap().to_owned()
}

#[test]
fn sprite_frames_follow_the_graph_and_survive_resyncs() {
    let mut app = app();
    declare(&mut app, sprite(json!({ "animGraph": hero() })));
    tick(&mut app, 0.0);
    assert_eq!(frame(&mut app), 0, "状态机管帧，节点的 frame 不起作用");
    tick(&mut app, 0.1);
    assert_eq!(frame(&mut app), 1);
    graph_mut(&mut app).set_float("speed", 2.0);
    tick(&mut app, 0.0);
    assert_eq!((state(&mut app), frame(&mut app)), ("walk".to_owned(), 4));

    declare(&mut app, sprite(json!({ "x": 5, "animGraph": hero() })));
    tick(&mut app, 0.1);
    assert_eq!((state(&mut app), frame(&mut app)), ("walk".to_owned(), 5), "定义一样（新对象）：不重建，接着播");

    let mut changed = hero();
    changed["clips"]["idle"]["fps"] = json!(5);
    declare(&mut app, sprite(json!({ "x": 5, "animGraph": changed })));
    tick(&mut app, 0.0);
    assert_eq!((state(&mut app), frame(&mut app)), ("idle".to_owned(), 0), "定义变了：从默认状态重来");
}

fn graph_mut(app: &mut App) -> Mut<'_, AnimGraph> {
    let e = entity(app, "hero");
    app.world_mut().get_mut::<AnimGraph>(e).unwrap()
}

#[test]
fn declared_params_are_written_only_when_they_change() {
    let mut app = app();
    declare(&mut app, sprite(json!({ "animGraph": hero(), "animParams": { "speed": 2 } })));
    tick(&mut app, 0.0);
    assert_eq!(graph(&mut app).unwrap().float("speed"), Some(2.0), "建好时写进去");
    graph_mut(&mut app).set_float("speed", 0.0);
    declare(&mut app, sprite(json!({ "x": 1, "animGraph": hero(), "animParams": { "speed": 2 } })));
    tick(&mut app, 0.0);
    assert_eq!(graph(&mut app).unwrap().float("speed"), Some(0.0), "没变：Rust 设的值不被盖回去");
    declare(&mut app, sprite(json!({ "x": 1, "animGraph": hero(), "animParams": { "speed": 3 } })));
    tick(&mut app, 0.0);
    assert_eq!(graph(&mut app).unwrap().float("speed"), Some(3.0), "变了才写");
}

#[test]
fn anim_set_command_sets_params_and_triggers() {
    let mut app = app();
    declare(&mut app, sprite(json!({ "animGraph": hero() })));
    tick(&mut app, 0.0);
    app.command("animSet", &json!({ "id": "hero", "params": { "speed": 1.5 }, "trigger": "jump" }));
    app.command("animSet", &json!({ "id": "nobody", "params": { "speed": 1 } }));
    tick(&mut app, 0.0);
    let g = graph(&mut app).unwrap();
    assert_eq!((g.float("speed"), g.triggered("jump")), (Some(1.5), Some(true)));
    app.command("animSet", &json!({ "id": "hero", "trigger": ["jump"], "params": { "speed": true } }));
    tick(&mut app, 0.0);
    assert_eq!(graph(&mut app).unwrap().float("speed"), Some(1.5), "类型不对的不写");
}

#[test]
fn anim_events_reach_tsx() {
    let mut app = app();
    declare(&mut app, sprite(json!({ "animGraph": hero(), "animParams": { "speed": 2 } })));
    tick(&mut app, 0.0);
    tick(&mut app, 0.0);
    let events = tick(&mut app, 0.15);
    let got: Vec<&Json> = events.iter().filter(|(n, _)| n == "animEvent").map(|(_, d)| d).collect();
    assert_eq!(got, [&json!({ "id": "hero", "layer": "base", "state": "walk", "clip": "walk", "name": "step" })]);
    let again = tick(&mut app, 0.1);
    assert!(again.iter().all(|(n, _)| n != "animEvent"), "不重复");
}

#[test]
fn bad_graphs_fall_back_and_graphs_replace_anim() {
    let mut app = app();
    declare(&mut app, sprite(json!({ "anim": { "frames": [2, 3] }, "animGraph": { "states": {} } })));
    tick(&mut app, 0.0);
    let e = entity(&mut app, "hero");
    assert!(app.world().get::<AnimGraph>(e).is_none() && app.world().get::<FrameAnimation>(e).is_some(), "不合法的定义不建，退回 anim");
    declare(&mut app, sprite(json!({ "anim": { "frames": [2, 3] }, "animGraph": hero() })));
    tick(&mut app, 0.0);
    assert!(app.world().get::<AnimGraph>(e).is_some() && app.world().get::<FrameAnimation>(e).is_none(), "两个都写时以状态机为准");
    declare(&mut app, sprite(json!({ "anim": { "frames": [2, 3] } })));
    tick(&mut app, 0.0);
    assert!(app.world().get::<AnimGraph>(e).is_none() && app.world().get::<FrameAnimation>(e).is_some(), "去掉状态机");
}

#[test]
fn model_graph_replaces_animation() {
    let mut app = app();
    let declare_model = |app: &mut App, extra: Json| {
        let mut node = json!({ "id": "jelly", "model": "asset:/@t/a.glb", "animation": { "clip": "idle" } });
        node.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        declare(app, json!({ "models": [node] }));
        tick(app, 0.1);
    };
    declare_model(&mut app, json!({ "animGraph": { "params": { "near": false }, "states": { "idle": { "clip": "idle" } } }, "animParams": { "near": true } }));
    let e = entity(&mut app, "jelly");
    let g = app.world().get::<AnimGraph>(e).cloned().unwrap();
    assert_eq!(g.bool("near"), Some(true));
    assert!(app.world().get::<Animator>(e).is_none(), "有状态机时不用 animation");
    declare_model(&mut app, json!({}));
    assert!(app.world().get::<AnimGraph>(e).is_none() && app.world().get::<Animator>(e).is_some(), "去掉状态机，回到 animation");
}

#[test]
fn sprite_frame_uses_the_top_heavy_override_layer() {
    let def: nge_anim::GraphDef = serde_json::from_value(json!({
        "params": { "upper": 0.4, "hit": 1 },
        "clips": { "idle": { "frames": [0, 1] }, "wave": { "frames": [8, 9] }, "shake": { "frames": [12] } },
        "states": { "idle": { "clip": "idle" } },
        "layers": [
            { "name": "upper", "weight": "upper", "states": { "wave": { "clip": "wave" } } },
            { "name": "hit", "mode": "additive", "weight": "hit", "states": { "shake": { "clip": "shake" } } },
        ],
    }))
    .unwrap();
    let mut g = AnimGraph::from_def(&def).unwrap();
    let step = |g: &mut AnimGraph, dt| {
        let graph = g.graph().clone();
        g.step(dt, |c| graph.clip(c).and_then(nge_anim::Clip::frames_duration));
        sprite_frame(g.graph(), g.output())
    };
    assert_eq!(step(&mut g, 0.15), Some(1), "上层不到一半、叠加层不算：第 0 层");
    g.set_float("upper", 0.5);
    assert_eq!(step(&mut g, 0.0), Some(9), "上层够一半");
    let empty = AnimGraph::from_def(&def).unwrap();
    assert_eq!(sprite_frame(empty.graph(), empty.output()), None, "还没走过一步");

    let blend: nge_anim::GraphDef = serde_json::from_value(json!({
        "params": { "speed": 0.3 },
        "clips": { "idle": { "frames": [0] }, "walk": { "frames": [4] } },
        "states": { "move": { "blend": { "by": "speed", "clips": [{ "clip": "idle", "at": 0 }, { "clip": "walk", "at": 1 }] } } },
    }))
    .unwrap();
    let mut g = AnimGraph::from_def(&blend).unwrap();
    assert_eq!(step(&mut g, 0.0), Some(0), "混合树里 idle 占 0.7：取 idle 的帧");
    g.set_float("speed", 0.7);
    assert_eq!(step(&mut g, 0.0), Some(4), "walk 占 0.7");
}
