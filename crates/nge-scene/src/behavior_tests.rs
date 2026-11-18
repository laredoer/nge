//! 精灵的行为树：按地面寻路跳上平台、落下来追到目标，巡逻、看见就追、追丢了回去，黑板和事件（见 docs/gameplay2d.md）。

use glam::Vec2;
use nge_render2d::Transform2d;
use serde_json::{Value, json};

use super::*;
use crate::testing::{VIEWPORT, app};

const T: f32 = 32.0;
const STEP: f32 = 1.0 / 60.0;

/// 16 列 8 行、每格 32，左上角在 (0, 256)：底行实心，第 5 行第 8 … 11 格是单向平台（帧 1），第 0 … 1 列左边有一堵两格高的墙。
fn level(sprites: Vec<Value>) -> Value {
    let mut data = vec![-1; 16 * 8];
    for c in 0..16 {
        data[7 * 16 + c] = 0;
    }
    for c in 8..12 {
        data[5 * 16 + c] = 1;
    }
    json!({
        "physics": { "gravity": [0, -1800] },
        "tilemaps": [{ "id": "m", "texture": "grid", "tile": T, "cols": 16, "x": 0, "y": 256, "data": data, "solid": [0], "oneWay": [1] }],
        "sprites": sprites,
    })
}

/// 站在第 `r` 行格子里的地面上、第 `c` 列中间（身高 24）。
fn on(c: f32, r: f32) -> [f32; 2] {
    [(c + 0.5) * T, 256.0 - (r + 1.0) * T + 12.0]
}

fn hero(at: [f32; 2]) -> Value {
    json!({ "id": "hero", "x": at[0], "y": at[1], "w": 16, "h": 24 })
}

fn walker(at: [f32; 2], tree: Value) -> Value {
    json!({ "id": "enemy", "x": at[0], "y": at[1], "w": 24, "h": 24, "body": { "kind": "character" },
        "behavior": { "tree": tree, "speed": 150, "jump": 640 } })
}

fn pos(app: &mut nge_ecs::App, id: &str) -> Vec2 {
    let world = app.world_mut();
    world.query::<(&SceneId, &Transform2d)>().iter(world).find(|(s, _)| s.0 == id).unwrap().1.translation
}

fn set_pos(app: &mut nge_ecs::App, id: &str, at: [f32; 2]) {
    let world = app.world_mut();
    let mut q = world.query::<(&SceneId, &mut Transform2d)>();
    let (_, mut t) = q.iter_mut(world).find(|(s, _)| s.0 == id).unwrap();
    t.translation = Vec2::from(at);
}

/// 按物理的步长跑 `frames` 帧，返回发出来的事件。
fn run(app: &mut nge_ecs::App, frames: usize) -> Vec<(String, Value)> {
    (0..frames).flat_map(|_| app.tick(VIEWPORT, STEP)).collect()
}

fn reached(app: &mut nge_ecs::App, target: Vec2) -> bool {
    let p = pos(app, "enemy");
    (p.x - target.x).abs() <= 0.5 * T && (p.y - target.y).abs() < 0.5 * T
}

#[test]
fn walkers_jump_onto_platforms_and_drop_back_down_to_reach_the_target() {
    let mut app = app();
    let up = on(10.0, 4.0);
    app.prop("scene", Some(&level(vec![hero(up), walker(on(2.0, 6.0), json!({ "type": "chase", "target": "hero" }))])));
    let mut highest = f32::MIN;
    for _ in 0..300 {
        run(&mut app, 1);
        highest = highest.max(pos(&mut app, "enemy").y);
        if reached(&mut app, Vec2::from(up)) {
            break;
        }
    }
    assert!(reached(&mut app, Vec2::from(up)), "跳上单向平台追到了 {} 最高 {highest}", pos(&mut app, "enemy"));
    // 目标回到平台右下的地上：从平台上穿下去或走出边落下
    let down = on(14.0, 6.0);
    set_pos(&mut app, "hero", down);
    run(&mut app, 240);
    assert!(reached(&mut app, Vec2::from(down)), "下来追到了 {}", pos(&mut app, "enemy"));
    // 目标在平台正下方：从单向平台穿下去
    run(&mut app, 1);
    set_pos(&mut app, "hero", up);
    run(&mut app, 300);
    assert!(reached(&mut app, Vec2::from(up)), "又上去了 {}", pos(&mut app, "enemy"));
    let below = on(9.0, 6.0);
    set_pos(&mut app, "hero", below);
    run(&mut app, 240);
    assert!(reached(&mut app, Vec2::from(below)), "从平台上穿下去 {}", pos(&mut app, "enemy"));
}

#[test]
fn unreachable_targets_fail_and_the_tree_moves_on() {
    // 目标在很高的地方（跳不上去）：chase 失败，选择节点改去 emit
    let mut app = app();
    let tree = json!({ "type": "selector", "children": [{ "type": "chase", "target": "hero" }, { "type": "emit", "name": "giveUp" }] });
    let mut scene = level(vec![hero(on(10.0, 0.0)), walker(on(2.0, 6.0), tree)]);
    // 第 1 行第 10 格放一块实心的，目标站在它上面（比地面高 6 格）
    let data = scene["tilemaps"][0]["data"].as_array_mut().unwrap();
    data[16 + 10] = json!(0);
    app.prop("scene", Some(&scene));
    let events = run(&mut app, 30);
    let gave_up: Vec<&Value> = events.iter().filter(|(k, _)| k == "behavior").map(|(_, v)| v).collect();
    assert!(!gave_up.is_empty(), "走不到就放弃");
    assert_eq!((gave_up[0]["id"].as_str(), gave_up[0]["name"].as_str()), (Some("enemy"), Some("giveUp")));
}

/// 巡逻：在出生点和右边 4 格之间来回；看见目标（400 以内、没墙挡着）就追，追丢了回去接着巡逻。
fn patrol_tree() -> Value {
    json!({ "type": "selector", "reactive": true, "children": [
        { "type": "sequence", "reactive": true, "children": [
            { "type": "see", "target": "hero", "range": 400 },
            { "type": "set", "key": "alert", "value": true },
            { "type": "chase", "target": "hero" },
        ]},
        { "type": "repeat", "child": { "type": "sequence", "children": [
            { "type": "set", "key": "alert", "value": false },
            { "type": "goto", "x": 2.5 * T, "y": 0 },
            { "type": "wait", "seconds": 0.2 },
            { "type": "goto", "x": 6.5 * T, "y": 0 },
            { "type": "wait", "seconds": 0.2 },
        ]}},
    ]})
}

fn alert(app: &mut nge_ecs::App) -> Option<nge_ai::Value> {
    let world = app.world_mut();
    world.query::<&Agent>().iter(world).next().unwrap().tree.blackboard.get("alert").cloned()
}

#[test]
fn walkers_patrol_chase_what_they_see_and_go_back() {
    let mut app = app();
    let mut tree = patrol_tree();
    // goto 的 y 用地面的高
    let y = on(0.0, 6.0)[1];
    for i in [1, 3] {
        tree["children"][1]["child"]["children"][i]["y"] = json!(y);
    }
    let far = on(15.0, 6.0);
    app.prop("scene", Some(&level(vec![hero([far[0], far[1] + 2000.0]), walker(on(2.0, 6.0), tree)])));
    // 目标远在天上看不见：来回巡逻
    let (mut left, mut right) = (f32::MAX, f32::MIN);
    for _ in 0..240 {
        run(&mut app, 1);
        let x = pos(&mut app, "enemy").x;
        (left, right) = (left.min(x), right.max(x));
    }
    assert!(left < 3.0 * T && right > 6.0 * T, "来回巡逻 {left} … {right}");
    assert_eq!(alert(&mut app), Some(nge_ai::Value::Bool(false)));
    // 目标落到右边地上、看得见：追过去
    set_pos(&mut app, "hero", far);
    run(&mut app, 240);
    assert!(reached(&mut app, Vec2::from(far)), "看见就追 {}", pos(&mut app, "enemy"));
    assert_eq!(alert(&mut app), Some(nge_ai::Value::Bool(true)));
    // 目标消失（飞走）：回去巡逻
    set_pos(&mut app, "hero", [far[0], far[1] + 2000.0]);
    run(&mut app, 300);
    let x = pos(&mut app, "enemy").x;
    assert!(x < 7.0 * T, "回到巡逻的地方 {x}");
    assert_eq!(alert(&mut app), Some(nge_ai::Value::Bool(false)));
}

#[test]
fn the_blackboard_command_and_bad_trees() {
    let mut app = app();
    let tree = json!({ "type": "sequence", "reactive": true, "children": [{ "type": "check", "key": "go" }, { "type": "goto", "x": 8.5 * T, "y": on(0.0, 6.0)[1] }] });
    let mut bad = walker(on(12.0, 6.0), json!({ "type": "wait", "seconds": -1 }));
    bad["id"] = json!("bad");
    let mut typo = walker(on(13.0, 6.0), json!({ "type": "dance" }));
    typo["id"] = json!("typo");
    app.prop("scene", Some(&level(vec![walker(on(2.0, 6.0), tree), bad, typo])));
    run(&mut app, 60);
    assert!((pos(&mut app, "enemy").x - 2.5 * T).abs() < 2.0, "黑板上没有 go：不走");
    app.command("blackboard", &json!({ "id": "enemy", "key": "go", "value": true }));
    run(&mut app, 180);
    assert!((pos(&mut app, "enemy").x - 8.5 * T).abs() < 0.5 * T, "写了 go 就走 {}", pos(&mut app, "enemy"));
    // 树写错的节点照样生成，只是没有行为
    let world = app.world_mut();
    let agents = world.query::<(&SceneId, Option<&Agent>)>().iter(world).filter(|(s, _)| s.0 == "bad" || s.0 == "typo").map(|(_, a)| a.is_some()).collect::<Vec<_>>();
    assert_eq!(agents, [false, false]);
}

#[test]
fn behaviors_survive_redeclaring_the_same_scene() {
    // 每帧重新声明同一个场景（TSX 每次渲染）：树的状态不重置
    let mut app = app();
    let tree = json!({ "type": "sequence", "children": [{ "type": "wait", "seconds": 0.5 }, { "type": "emit", "name": "done" }] });
    let mut events = Vec::new();
    for i in 0..45 {
        // 精灵别的属性（颜色）每帧在变，行为树不变
        let mut scene = level(vec![walker(on(2.0, 6.0), tree.clone())]);
        scene["sprites"][0]["color"] = json!(if i % 2 == 0 { "#ff0000" } else { "#00ff00" });
        app.prop("scene", Some(&scene));
        events.extend(run(&mut app, 1));
    }
    assert_eq!(events.iter().filter(|(k, _)| k == "behavior").count(), 1, "等够 0.5 秒发一次");
}
