//! 瓦片地图的单向平台、斜坡、动画瓦片、每种瓦片的材质、点某一格（见 docs/gameplay2d.md）。

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_physics2d::{Collider, Shape, Velocity};
use nge_render2d::{Sprite, SpriteImage, Transform2d};
use serde_json::{Value, json};

use crate::tilemap::TileSolid;
use crate::testing::{app, frame};

fn colliders(app: &mut nge_ecs::App) -> Vec<(Vec2, Collider)> {
    let world = app.world_mut();
    let mut v: Vec<(Vec2, Collider)> = world.query_filtered::<(&Transform2d, &Collider), With<TileSolid>>().iter(world).map(|(t, c)| (t.translation, *c)).collect();
    v.sort_by(|a, b| (a.0.x, a.0.y).partial_cmp(&(b.0.x, b.0.y)).unwrap());
    v
}

fn sprite_pos(app: &mut nge_ecs::App, id: &str) -> Vec2 {
    let world = app.world_mut();
    world.query::<(&crate::SceneId, &Transform2d)>().iter(world).find(|(s, _)| s.0 == id).unwrap().1.translation
}

/// 4 列 3 行、每格 10：第 0 行第 1、2 格是单向平台（帧 1），第 1 行第 3 格是 45° 斜坡（帧 2），底行是实心（帧 0）。
fn map(extra: Value) -> Value {
    let mut m = json!({ "id": "m", "texture": "grid", "tile": 10, "cols": 4, "x": 0, "y": 30,
        "data": [-1, 1, 1, -1, -1, -1, -1, 2, 0, 0, 0, 0], "solid": [0], "oneWay": [1], "slopes": [{ "frame": 2, "left": 0, "right": 1 }] });
    for (k, v) in extra.as_object().unwrap() {
        m[k] = v.clone();
    }
    m
}

#[test]
fn one_way_and_slope_tiles_get_their_own_colliders() {
    let mut app = app();
    frame(&mut app, Some(json!({ "tilemaps": [map(json!({}))] })));
    let c = colliders(&mut app);
    assert_eq!(c.len(), 3, "单向平台一块、斜坡一块、地面一块 {c:?}");
    let plank = c.iter().find(|(_, c)| c.one_way).expect("单向平台");
    assert_eq!((plank.0, plank.1.shape), (Vec2::new(20.0, 25.0), Shape::Rect(Vec2::new(20.0, 10.0))), "同一行连着的并成一块");
    let slope = c.iter().find(|(_, c)| matches!(c.shape, Shape::Slope { .. })).expect("斜坡");
    assert_eq!((slope.0, slope.1.shape), (Vec2::new(35.0, 15.0), Shape::Slope { size: Vec2::splat(10.0), left: 0.0, right: 1.0 }));
    assert!(!slope.1.one_way);
    let ground = c.iter().find(|(_, c)| !c.one_way && matches!(c.shape, Shape::Rect(_))).unwrap();
    assert_eq!(ground.0, Vec2::new(20.0, 5.0));
    // 一个帧号同时写在几处时：斜坡优先，然后单向平台，最后实心
    frame(&mut app, Some(json!({ "tilemaps": [map(json!({ "solid": [0, 1, 2] }))] })));
    let c = colliders(&mut app);
    assert_eq!(c.iter().filter(|(_, c)| c.one_way).count(), 1);
    assert_eq!(c.iter().filter(|(_, c)| matches!(c.shape, Shape::Slope { .. })).count(), 1);
}

#[test]
fn sprite_bodies_can_be_one_way_platforms() {
    // 精灵刚体的 oneWay（比如移动平台）：从下面跳上去，站住
    let mut app = app();
    let scene = |vy: f32| json!({
        "physics": { "gravity": [0, -1000] },
        "sprites": [
            { "id": "ground", "x": 0, "y": -10, "w": 400, "h": 20, "body": { "kind": "fixed" } },
            { "id": "lift", "x": 0, "y": 59, "w": 100, "h": 10, "body": { "kind": "kinematic", "oneWay": true } },
            { "id": "hero", "x": 0, "y": 10, "w": 20, "h": 20, "body": { "kind": "character", "velocity": [0, vy] } },
        ],
    });
    frame(&mut app, Some(scene(0.0)));
    frame(&mut app, None);
    assert!((sprite_pos(&mut app, "hero").y - 10.0).abs() < 1.0, "站在地面上，头顶离平台还有一段");
    frame(&mut app, Some(scene(600.0)));
    for _ in 0..25 {
        frame(&mut app, None);
    }
    let y = sprite_pos(&mut app, "hero").y;
    assert!((y - 74.0).abs() < 1.0, "穿上去站在平台上 {y}");
}

#[test]
fn rows_of_one_way_tiles_stay_separate_platforms() {
    // 上下两行的单向平台不并成一块厚的：各自一块，站在哪一层都对
    let mut app = app();
    frame(&mut app, Some(json!({ "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 2, "data": [1, 1, 1, 1], "oneWay": [1] }] })));
    let c = colliders(&mut app);
    assert_eq!(c.iter().map(|(p, c)| (*p, c.shape)).collect::<Vec<_>>(), [(Vec2::new(10.0, -15.0), Shape::Rect(Vec2::new(20.0, 10.0))), (Vec2::new(10.0, -5.0), Shape::Rect(Vec2::new(20.0, 10.0)))]);
}

#[test]
fn characters_jump_up_through_one_way_tiles_and_stand_on_them() {
    let mut app = app();
    // 地面顶边 y = 0，单向平台在第 2 行（顶边 y = 64），每格 32
    let mut data = vec![-1; 8 * 4];
    data[8 + 2..8 + 6].fill(1);
    data[24..32].fill(0);
    let scene = |vy: f32| json!({
        "physics": { "gravity": [0, -1000] },
        "tilemaps": [{ "id": "m", "texture": "grid", "tile": 32, "cols": 8, "x": 0, "y": 96, "data": data, "solid": [0], "oneWay": [1] }],
        "sprites": [{ "id": "hero", "x": 128, "y": 12, "w": 20, "h": 20, "body": { "kind": "character", "velocity": [0, vy] } }],
    });
    frame(&mut app, Some(scene(0.0)));
    for _ in 0..4 {
        frame(&mut app, None);
    }
    assert!((sprite_pos(&mut app, "hero").y - 10.0).abs() < 1.0, "站在地面上 {}", sprite_pos(&mut app, "hero"));
    frame(&mut app, Some(scene(600.0)));
    for _ in 0..25 {
        frame(&mut app, None);
    }
    let y = sprite_pos(&mut app, "hero").y;
    assert!((y - 74.0).abs() < 1.0, "跳上去站在单向平台上 {y}");
}

#[test]
fn slope_tiles_are_walked_up_and_down() {
    // 每格 32：左边平地，中间两格 45° 的坡，右边高两格的平地；坡下面是实心的
    let mut app = app();
    let cols = 10;
    let mut data = vec![-1; cols * 4];
    data[cols + 5] = 2;
    data[cols + 6..cols * 2].fill(0);
    data[2 * cols + 4] = 2;
    data[2 * cols + 5..cols * 3].fill(0);
    data[3 * cols..].fill(0);
    let tilemap = json!({ "id": "m", "texture": "grid", "tile": 32, "cols": cols, "x": 0, "y": 128, "data": data, "solid": [0],
        "slopes": [{ "frame": 2, "left": 0, "right": 1 }], "friction": 0 });
    let hero = |x: f32, y: f32, vx: f32| json!({ "id": "hero", "x": x, "y": y, "w": 20, "h": 20, "body": { "kind": "character", "velocity": [vx, 0] } });
    frame(&mut app, Some(json!({ "physics": { "gravity": [0, -1000] }, "tilemaps": [tilemap.clone()], "sprites": [hero(40.0, 42.0, 0.0)] })));
    frame(&mut app, None);
    let world = app.world_mut();
    let e = world.query::<(Entity, &crate::SceneId)>().iter(world).find(|(_, s)| s.0 == "hero").unwrap().0;
    let mut least = f32::INFINITY;
    for _ in 0..120 {
        let vy = app.world().get::<Velocity>(e).unwrap().linear.y;
        app.world_mut().get_mut::<Velocity>(e).unwrap().linear = Vec2::new(150.0, vy);
        let before = sprite_pos(&mut app, "hero").x;
        app.tick(crate::testing::VIEWPORT, 1.0 / 60.0);
        least = least.min(sprite_pos(&mut app, "hero").x - before);
        if sprite_pos(&mut app, "hero").x > 240.0 {
            break;
        }
    }
    let p = sprite_pos(&mut app, "hero");
    assert!(p.x > 230.0 && (p.y - 106.0).abs() < 1.5, "走上坡顶 {p}");
    assert!(least > 1.0, "坡和坡、坡和平地相接处没卡住 {least}");
}

#[test]
fn animated_tiles_change_frames_without_rebuilding_the_map() {
    let mut app = app();
    let scene = json!({ "tilemaps": [{ "id": "w", "texture": "grid", "tile": 10, "cols": 3, "data": [3, 0, 3],
        "animated": [{ "frame": 3, "frames": [3, 4, 5], "fps": 4 }] }] });
    frame(&mut app, Some(scene));
    let world = app.world_mut();
    let before: Vec<Entity> = world.query_filtered::<Entity, With<Sprite>>().iter(world).collect();
    let frames = |app: &mut nge_ecs::App| {
        let world = app.world_mut();
        let mut v: Vec<(i32, u32)> = world
            .query::<(&Transform2d, &Sprite)>()
            .iter(world)
            .map(|(t, s)| (t.translation.x as i32, if let SpriteImage::Atlas(_, f) = s.image { f } else { 99 }))
            .collect();
        v.sort();
        v
    };
    // 每帧 0.5 秒、4 帧 / 秒：每帧往前两格
    frame(&mut app, None);
    // 生成的那一帧也走了 0.5 秒：到这里是 1 秒，第 4 格，循环回第 1 格
    assert_eq!(frames(&mut app), [(5, 4), (15, 0), (25, 4)], "动画瓦片一起换帧，别的格子不动");
    frame(&mut app, None);
    assert_eq!(frames(&mut app), [(5, 3), (15, 0), (25, 3)], "循环");
    let world = app.world_mut();
    let after: Vec<Entity> = world.query_filtered::<Entity, With<Sprite>>().iter(world).collect();
    assert_eq!(before, after, "没重建");
    frame(&mut app, None);
    // 改了地图的规则（整张重建）：动画接着原来的相位，不从头播（到这里 2.5 秒，第 10 格，循环下来是第 2 格）
    let scene = json!({ "tilemaps": [{ "id": "w", "texture": "grid", "tile": 10, "cols": 3, "data": [3, 0, 3],
        "animated": [{ "frame": 3, "frames": [3, 4, 5], "fps": 4 }], "materials": [{ "frame": 0, "friction": 0.1 }] }] });
    frame(&mut app, Some(scene));
    assert_eq!(frames(&mut app), [(5, 4), (15, 0), (25, 4)], "接着原来的相位");
}

#[test]
fn tile_materials_split_colliders_by_friction() {
    let mut app = app();
    frame(&mut app, Some(json!({ "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 4, "data": [0, 0, 6, 6], "solid": [0, 6],
        "friction": 0.5, "materials": [{ "frame": 6, "friction": 0.02 }, { "frame": 0, "restitution": 0.3 }] }] })));
    let c = colliders(&mut app);
    assert_eq!(c.iter().map(|(p, c)| (*p, c.friction, c.restitution)).collect::<Vec<_>>(), [(Vec2::new(10.0, -5.0), 0.5, 0.3), (Vec2::new(30.0, -5.0), 0.02, 0.0)]);
}

#[test]
fn ice_lets_boxes_slide_further() {
    let slide = |friction: f32| {
        let mut app = app();
        let mut data = vec![0; 40];
        data[..20].fill(-1);
        frame(&mut app, Some(json!({ "tilemaps": [{ "id": "m", "texture": "grid", "tile": 20, "cols": 20, "x": -200, "y": 20, "data": data, "solid": [0],
            "materials": [{ "frame": 0, "friction": friction }] }],
            "sprites": [{ "id": "box", "w": 10, "h": 10, "y": 6, "body": { "velocity": [200, 0], "friction": 0 } }] })));
        for _ in 0..10 {
            frame(&mut app, None);
        }
        sprite_pos(&mut app, "box").x
    };
    let (rough, ice) = (slide(1.0), slide(0.0));
    assert!(ice > rough + 50.0, "冰面上滑得远 {rough} {ice}");
}

#[test]
fn tapping_a_tilemap_reports_the_cell() {
    let mut app = app();
    // 视口 100×100：屏幕 (80, 40) 是世界 (30, 10)
    frame(&mut app, Some(json!({ "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 5, "x": 0, "y": 30, "data": vec![0; 15], "tap": true }] })));
    app.command("touch", &json!({ "phase": "start", "id": 1, "x": 80, "y": 40 }));
    let events = frame(&mut app, None);
    let tap = events.iter().find(|(n, _)| n == "tap").expect("点中了").1.clone();
    assert_eq!((tap["id"].as_str(), tap["cell"].clone(), tap["frame"].as_i64()), (Some("m"), json!([3, 2]), Some(0)));
    // 没写 tap 的、点在地图外的不发
    frame(&mut app, Some(json!({ "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 5, "x": 0, "y": 30, "data": vec![0; 15] }] })));
    app.command("touch", &json!({ "phase": "start", "id": 2, "x": 80, "y": 40 }));
    assert!(frame(&mut app, None).iter().all(|(n, _)| n != "tap"));
    frame(&mut app, Some(json!({ "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 5, "x": 0, "y": 30, "data": vec![0; 15], "tap": true }] })));
    app.command("touch", &json!({ "phase": "start", "id": 3, "x": 10, "y": 90 }));
    assert!(frame(&mut app, None).iter().all(|(n, _)| n != "tap"), "地图外");
}

#[test]
fn bad_tile_rules_only_drop_that_rule() {
    let mut app = app();
    frame(&mut app, Some(json!({ "tilemaps": [map(json!({ "slopes": [{ "frame": 2, "left": "low", "right": 1 }, { "frame": 2, "left": 0, "right": 1 }],
        "animated": [{ "frame": 3, "frames": [-1], "fps": 4 }], "materials": [{ "frame": 0, "friction": null }] }))] })));
    let c = colliders(&mut app);
    assert_eq!(c.len(), 3, "写错的那条斜坡丢掉，别的照常 {c:?}");
}

#[test]
fn slopes_block_flyers_but_one_way_tiles_do_not() {
    let mut app = app();
    frame(&mut app, Some(json!({ "tilemaps": [map(json!({}))] })));
    let world = app.world_mut();
    let nav = world.query::<&nge_ai::NavGrid>().iter(world).next().unwrap().clone();
    assert!(nav.grid.open((1, 0)) && !nav.grid.open((3, 1)) && !nav.grid.open((0, 2)), "单向平台能飞过去，斜坡和实心挡住");
}

#[test]
fn the_drop_through_command_drops_characters_off_one_way_platforms() {
    let mut app = app();
    let scene = json!({
        "physics": { "gravity": [0, -1000] },
        "sprites": [
            { "id": "ground", "x": 0, "y": -10, "w": 400, "h": 20, "body": { "kind": "fixed" } },
            { "id": "plank", "x": 0, "y": 59, "w": 100, "h": 10, "body": { "kind": "fixed", "oneWay": true } },
            { "id": "hero", "x": 0, "y": 80, "w": 20, "h": 20, "body": { "kind": "character" } },
        ],
    });
    frame(&mut app, Some(scene));
    for _ in 0..4 {
        frame(&mut app, None);
    }
    assert!((sprite_pos(&mut app, "hero").y - 74.0).abs() < 1.0, "站在板上 {}", sprite_pos(&mut app, "hero"));
    app.command("dropThrough", &json!({ "id": "hero" }));
    app.command("dropThrough", &json!({ "id": "nobody" }));
    let events = frame(&mut app, None);
    let replies: Vec<&Value> = events.iter().filter(|(k, _)| k == "dropThrough").map(|(_, v)| v).collect();
    assert_eq!(replies, [&json!({ "id": "hero", "ok": true }), &json!({ "id": "nobody", "ok": false })]);
    for _ in 0..10 {
        frame(&mut app, None);
    }
    assert!((sprite_pos(&mut app, "hero").y - 10.0).abs() < 1.0, "穿下去落到地上 {}", sprite_pos(&mut app, "hero"));
}

#[test]
fn tapping_sorts_tilemaps_and_background_sprites_by_z_and_skips_empty_cells() {
    let mut app = app();
    // 视口 100×100：屏幕 (80, 40) 是世界 (30, 10)，落在第 3 列第 2 行
    let mut data = vec![0; 15];
    data[2 * 5 + 4] = -1;
    let scene = |map_z: f32| json!({
        "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 5, "x": 0, "y": 30, "z": map_z, "data": data, "tap": true }],
        "sprites": [{ "id": "back", "x": 30, "y": 10, "w": 40, "h": 40, "z": -3, "tap": true }],
    });
    let tap = |app: &mut nge_ecs::App, id: u32, x: f32| {
        app.command("touch", &json!({ "phase": "start", "id": id, "x": x, "y": 40 }));
        let events = frame(app, None);
        events.iter().find(|(n, _)| n == "tap").map(|(_, v)| v["id"].as_str().unwrap_or_default().to_owned())
    };
    frame(&mut app, Some(scene(-5.0)));
    assert_eq!(tap(&mut app, 1, 80.0).as_deref(), Some("back"), "背景精灵比地图高，先点到它");
    frame(&mut app, Some(scene(-1.0)));
    assert_eq!(tap(&mut app, 2, 80.0).as_deref(), Some("m"), "地图比背景精灵高");
    // 第 4 列第 2 行是空格：点到后面的精灵
    assert_eq!(tap(&mut app, 3, 95.0).as_deref(), Some("back"), "空格不算点中");
}

#[test]
fn slope_frames_also_listed_as_one_way_are_flagged() {
    let node: crate::tilemap::TilemapNode = serde_json::from_value(map(json!({ "oneWay": [1, 2], "slopes": [{ "frame": 2, "left": 0, "right": 1 }] }))).unwrap();
    assert_eq!(node.slopes_also_one_way(), [2]);
    let node: crate::tilemap::TilemapNode = serde_json::from_value(map(json!({ "oneWay": [1], "slopes": [{ "frame": 2, "left": 0, "right": 1 }] }))).unwrap();
    assert!(node.slopes_also_one_way().is_empty());
}
