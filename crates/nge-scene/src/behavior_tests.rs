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

/// 底行第 0 … 7 列实心，8 … 15 列是坑（底下什么都没有）。
fn pit_level(sprites: Vec<Value>) -> Value {
    let mut data = vec![-1; 16 * 8];
    for c in 0..8 {
        data[7 * 16 + c] = 0;
    }
    json!({
        "physics": { "gravity": [0, -1800] },
        "tilemaps": [{ "id": "m", "texture": "grid", "tile": T, "cols": 16, "x": 0, "y": 256, "data": data, "solid": [0] }],
        "sprites": sprites,
    })
}

fn behavior_events(events: &[(String, Value)]) -> Vec<String> {
    events.iter().filter(|(k, _)| k == "behavior").filter_map(|(_, v)| v["name"].as_str().map(str::to_owned)).collect()
}

#[test]
fn targets_over_a_pit_are_chased_to_the_edge_without_falling_in() {
    // 目标悬在坑上方（站在移动平台上、或者正跳过坑）：走到坑边停下，不掉下去
    let mut app = app();
    let over = [11.5 * T, 256.0 - 5.0 * T];
    app.prop("scene", Some(&pit_level(vec![hero(over), walker(on(3.0, 6.0), json!({ "type": "chase", "target": "hero" }))])));
    let mut lowest = f32::MAX;
    for _ in 0..240 {
        run(&mut app, 1);
        lowest = lowest.min(pos(&mut app, "enemy").y);
    }
    let p = pos(&mut app, "enemy");
    assert!(lowest > on(0.0, 6.0)[1] - 2.0, "没掉下去 {p} 最低 {lowest}");
    assert!(p.x > 6.5 * T && p.x < 8.0 * T, "停在坑边 {p}");
}

#[test]
fn a_failed_chase_does_not_keep_walking_toward_the_target() {
    // 目标站在坑对面够不着的地方：chase 失败，选择节点改去 wait，角色站着不动
    let mut app = app();
    let mut lv = pit_level(vec![]);
    lv["tilemaps"][0]["data"][7 * 16 + 15] = json!(0);
    let tree = json!({ "type": "selector", "reactive": true, "children": [
        { "type": "chase", "target": "hero" }, { "type": "wait", "seconds": 100 } ] });
    lv["sprites"] = json!([hero(on(15.0, 6.0)), walker(on(3.0, 6.0), tree)]);
    app.prop("scene", Some(&lv));
    run(&mut app, 240);
    let p = pos(&mut app, "enemy");
    assert!((p.x - 3.5 * T).abs() < 2.0, "原地等 {p}");
}

#[test]
fn walkers_without_a_plan_do_not_walk_off_ledges() {
    // 站在坑上方一块精灵刚体（不是瓦片）上：找不到脚下的地面，朝目标走也不走出这块的边
    let mut app = app();
    let mut lv = pit_level(vec![]);
    lv["tilemaps"][0]["data"][7 * 16 + 15] = json!(0);
    let ledge = json!({ "id": "ledge", "x": 9.0 * T, "y": 256.0 - 7.5 * T, "w": 2.0 * T, "h": T, "body": { "kind": "fixed" } });
    lv["sprites"] = json!([ledge, hero(on(15.0, 6.0)), walker(on(8.5, 6.0), json!({ "type": "chase", "target": "hero" }))]);
    app.prop("scene", Some(&lv));
    let mut lowest = f32::MAX;
    for _ in 0..240 {
        run(&mut app, 1);
        lowest = lowest.min(pos(&mut app, "enemy").y);
    }
    assert!(lowest > on(0.0, 6.0)[1] - 2.0, "没从那块上走下去 {} 最低 {lowest}", pos(&mut app, "enemy"));
}

#[test]
fn walkers_that_fall_out_of_the_map_go_back_home_and_say_so() {
    let mut app = app();
    let home = on(3.0, 6.0);
    app.prop("scene", Some(&pit_level(vec![walker(home, json!({ "type": "wait", "seconds": 100 }))])));
    run(&mut app, 10);
    set_pos(&mut app, "enemy", [11.5 * T, 100.0]);
    let mut events = Vec::new();
    for _ in 0..240 {
        events.extend(run(&mut app, 1));
        if !behavior_events(&events).is_empty() {
            break;
        }
    }
    assert_eq!(behavior_events(&events), ["fell"]);
    run(&mut app, 30);
    let p = pos(&mut app, "enemy");
    assert!((p - Vec2::from(home)).length() < 4.0, "回到出生点 {p}");
}

#[test]
fn walkers_give_up_on_jumps_they_keep_failing() {
    // 右边两格高的台子，按地形跳得上去；可是头顶有一块精灵刚体（地形里没有）挡着，跳几次都碰头
    let mut app = app();
    let mut lv = level(vec![]);
    let data = lv["tilemaps"][0]["data"].as_array_mut().unwrap();
    for c in 8..12 {
        data[5 * 16 + c] = json!(-1);
    }
    for c in 12..16 {
        data[5 * 16 + c] = json!(0);
        data[6 * 16 + c] = json!(0);
    }
    let lid = json!({ "id": "lid", "x": 6.0 * T, "y": 256.0 - 5.5 * T, "w": 12.0 * T, "h": 8, "body": { "kind": "fixed" } });
    let tree = json!({ "type": "selector", "children": [{ "type": "chase", "target": "hero" }, { "type": "emit", "name": "gaveUp" }] });
    lv["sprites"] = json!([lid, hero(on(14.0, 4.0)), walker(on(9.0, 6.0), tree)]);
    app.prop("scene", Some(&lv));
    let events = run(&mut app, 900);
    assert!(behavior_events(&events).iter().any(|n| n == "gaveUp"), "跳了几次都没上去就放弃 {}", pos(&mut app, "enemy"));
    assert!(pos(&mut app, "enemy").y < on(0.0, 5.0)[1], "确实没上去");
}

#[test]
fn walkers_use_the_map_under_their_feet() {
    // 两张地图：大的只有最底下一行地面；小的叠在上面，有一行带一格缺口的地面。角色站在小地图上，追同一张上缺口对面的目标
    let mut app = app();
    let mut lv = level(vec![]);
    let data = lv["tilemaps"][0]["data"].as_array_mut().unwrap();
    for c in 8..12 {
        data[5 * 16 + c] = json!(-1);
    }
    let mut upper = vec![-1; 16 * 3];
    for c in 0..16 {
        if c != 7 {
            upper[2 * 16 + c] = 0;
        }
    }
    let small = json!({ "id": "upper", "texture": "grid", "tile": T, "cols": 16, "x": 0, "y": 192, "data": upper, "solid": [0] });
    lv["tilemaps"].as_array_mut().unwrap().push(small);
    // 小地图第 2 行是地面：站在它上面的格子（世界 y 128 … 160）
    let stand = |c: f32| [(c + 0.5) * T, 192.0 - 2.0 * T + 12.0];
    lv["sprites"] = json!([hero(stand(10.0)), walker(stand(4.0), json!({ "type": "chase", "target": "hero" }))]);
    app.prop("scene", Some(&lv));
    run(&mut app, 240);
    assert!(reached(&mut app, Vec2::from(stand(10.0))), "按脚下那张图跳过缺口 {}", pos(&mut app, "enemy"));
}

#[test]
fn walkers_standing_on_slopes_still_see() {
    // 底下一行地面，第 6 列一格 45° 上坡，右边高一格的台子；角色站在坡上看左边地上的目标
    let mut app = app();
    let mut lv = level(vec![]);
    let data = lv["tilemaps"][0]["data"].as_array_mut().unwrap();
    for c in 8..12 {
        data[5 * 16 + c] = json!(-1);
    }
    data[6 * 16 + 6] = json!(2);
    for c in 7..16 {
        data[6 * 16 + c] = json!(0);
    }
    lv["tilemaps"][0]["slopes"] = json!([{ "frame": 2, "left": 0, "right": 1 }]);
    let tree = json!({ "type": "sequence", "children": [{ "type": "see", "target": "hero", "range": 400 }, { "type": "emit", "name": "seen" }] });
    lv["sprites"] = json!([hero(on(2.0, 6.0)), walker([6.5 * T, 256.0 - 6.5 * T + 12.0], tree)]);
    app.prop("scene", Some(&lv));
    let events = run(&mut app, 3);
    assert!(behavior_events(&events).iter().any(|n| n == "seen"), "{}", pos(&mut app, "enemy"));
}

#[test]
fn trees_that_can_never_succeed_and_commands_without_a_value_are_flagged() {
    let tree: BtNode = serde_json::from_value(json!({ "type": "sequence", "children": [
        { "type": "parallel", "succeed": 3, "children": [{ "type": "wait", "seconds": 1 }, { "type": "emit", "name": "x" }] },
        { "type": "parallel", "succeed": 1, "children": [{ "type": "wait", "seconds": 1 }] },
    ]}))
    .unwrap();
    let lint = tree.lint();
    assert_eq!(lint.len(), 1, "{lint:?}");
    assert!(lint[0].contains("succeed"));
    // 没写 value 不删
    let mut app = app();
    app.prop("scene", Some(&level(vec![walker(on(2.0, 6.0), json!({ "type": "wait", "seconds": 100 }))])));
    app.command("blackboard", &json!({ "id": "enemy", "key": "k", "value": 1 }));
    run(&mut app, 1);
    app.command("blackboard", &json!({ "id": "enemy", "key": "k" }));
    run(&mut app, 1);
    let world = app.world_mut();
    let k = world.query::<&Agent>().iter(world).next().unwrap().tree.blackboard.get("k").cloned();
    assert_eq!(k, Some(nge_ai::Value::Num(1.0)), "没写 value 的不改");
    app.command("blackboard", &json!({ "id": "enemy", "key": "k", "value": null }));
    run(&mut app, 1);
    let world = app.world_mut();
    assert_eq!(world.query::<&Agent>().iter(world).next().unwrap().tree.blackboard.get("k"), None, "null 删掉");
}

#[test]
fn walker_size_and_slope_come_from_the_node() {
    let agent = Agent::new(Tree::new(Node::wait(1.0)), 100.0, 600.0, Vec2::ZERO, Vec2::new(12.0, 20.0));
    let w = super::walker(&agent, 32.0, 1800.0, 0.5);
    assert_eq!((w.half, w.height, w.max_slope), (12.0 / 32.0, 40.0 / 32.0, 0.5));
    assert!((w.reach.height - 600.0 * 600.0 / 3600.0 / 32.0 * MARGIN).abs() < 1e-4);
}
