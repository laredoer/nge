//! 瓦片地图按块改：改一格只重建一条碰撞体，导航、地面、点地图原地改，结果和整张重建一样（见 docs/big-scene-perf.md）。

use bevy_ecs::prelude::*;
use nge_ai::{Ground, GroundMap, NavGrid};
use nge_physics2d::Collider;
use nge_render2d::{TileLayer, Transform2d};
use serde_json::{Value, json};

use crate::testing::{app, frame};
use crate::tile_chunks::{TileBodies, band_colliders};
use crate::tilemap::{TileSolid, TilemapNode};

/// 20 列 12 行、每格 10、一条 4 行：底下两行实心（帧 0），中间一行单向平台（帧 1），一块斜坡（帧 2）。
fn data() -> Vec<i32> {
    let mut d = vec![-1; 20 * 12];
    d[20 * 10..].fill(0);
    d[20 * 5 + 3..20 * 5 + 9].fill(1);
    d[20 * 9 + 15] = 2;
    d
}

fn node(data: &[i32]) -> Value {
    json!({ "id": "m", "texture": "grid", "tile": 10, "cols": 20, "x": -100, "y": 60, "chunk": 4, "data": data,
        "solid": [0], "oneWay": [1], "slopes": [{ "frame": 2, "left": 0, "right": 1 }], "tap": true })
}

fn colliders(app: &mut nge_ecs::App) -> Vec<String> {
    let world = app.world_mut();
    let mut v: Vec<String> = world.query_filtered::<(&Transform2d, &Collider), With<TileSolid>>().iter(world).map(|(t, c)| format!("{:?} {:?}", t.translation, c)).collect();
    v.sort();
    v
}

fn rebuilt(app: &mut nge_ecs::App) -> (u64, u64) {
    let world = app.world_mut();
    let (layer, bodies) = world.query::<(&TileLayer, &TileBodies)>().single(world).unwrap();
    (layer.rebuilt(), bodies.rebuilt())
}

fn solid_entities(app: &mut nge_ecs::App) -> Vec<Entity> {
    let world = app.world_mut();
    let mut v: Vec<Entity> = world.query_filtered::<Entity, With<TileSolid>>().iter(world).collect();
    v.sort();
    v
}

#[test]
fn set_tiles_rebuilds_only_the_band_it_touches() {
    let mut app = app();
    frame(&mut app, Some(json!({ "tilemaps": [node(&data())] })));
    let before = solid_entities(&mut app);
    // 挖掉底行一格（第 11 行在第 2 条），填上第 1 行一格（第 0 条）
    app.command("setTiles", &json!({ "id": "m", "tiles": [[4, 11, -1], [7, 1, 0]] }));
    frame(&mut app, None);
    assert_eq!(rebuilt(&mut app).1, 2, "两条各重建一次");
    let after = solid_entities(&mut app);
    let gone: Vec<&Entity> = before.iter().filter(|e| !after.contains(e)).collect();
    // 底下两行原来是一整块，挖了一格变成两块；第 1 条、斜坡没动
    assert_eq!(gone.len(), 1, "只有变了的那块换了实体 {gone:?}");
    assert_eq!(after.len(), before.len() + 3, "底下两行从一块拆成三块，第 0 条多一块");
    let world = app.world_mut();
    let (layer, nav, ground) = world.query::<(&TileLayer, &NavGrid, &GroundMap)>().single(world).unwrap();
    assert_eq!((layer.frame(4, 11), layer.frame(7, 1)), (Some(-1), Some(0)));
    assert!(nav.grid.open((4, 11)) && !nav.grid.open((7, 1)), "导航网格跟着改");
    assert_eq!((ground.terrain.at((4, 11)), ground.terrain.at((7, 1))), (Ground::Empty, Ground::Solid));
}

#[test]
fn local_edits_build_the_same_colliders_as_a_fresh_map() {
    let mut edited = data();
    let edits = [[4, 11, -1], [5, 11, -1], [7, 1, 0], [15, 9, -1], [16, 9, 2], [3, 5, -1], [19, 0, 1]];
    for [c, r, f] in edits {
        edited[r as usize * 20 + c as usize] = f;
    }
    let mut local = app();
    frame(&mut local, Some(json!({ "tilemaps": [node(&data())] })));
    local.command("setTiles", &json!({ "id": "m", "tiles": edits }));
    frame(&mut local, None);
    let mut fresh = app();
    frame(&mut fresh, Some(json!({ "tilemaps": [node(&edited)] })));
    assert_eq!(colliders(&mut local), colliders(&mut fresh));
    // 声明里改格子也走局部：和整张建的一样
    let mut declared = app();
    frame(&mut declared, Some(json!({ "tilemaps": [node(&data())] })));
    frame(&mut declared, Some(json!({ "tilemaps": [node(&edited)] })));
    assert_eq!(rebuilt(&mut declared).1, 3, "改到的三条各重建一次");
    assert_eq!(colliders(&mut declared), colliders(&mut fresh));
}

#[test]
fn a_ball_lands_the_same_on_an_edited_map_and_a_fresh_one() {
    let ball = json!({ "id": "ball", "w": 8, "x": 25, "y": 80, "body": { "shape": "circle" } });
    let mut edited = data();
    edited[11 * 20 + 12] = -1;
    edited[10 * 20 + 12] = -1;
    let land = |local: bool| {
        let mut app = app();
        let start = if local { data() } else { edited.clone() };
        frame(&mut app, Some(json!({ "tilemaps": [node(&start)], "sprites": [ball.clone()] })));
        if local {
            app.command("setTiles", &json!({ "id": "m", "tiles": [[12, 11, -1], [12, 10, -1]] }));
        }
        for _ in 0..8 {
            frame(&mut app, None);
        }
        let world = app.world_mut();
        world.query::<(&crate::SceneId, &Transform2d)>().iter(world).find(|(id, _)| id.0 == "ball").unwrap().1.translation
    };
    let (a, b) = (land(true), land(false));
    assert!((a - b).length() < 1e-3, "挖开的洞里掉下去的位置一样 {a} {b}");
    assert!(a.y < -50.0, "掉进挖开的洞里 {a}");
}

#[test]
fn bands_span_the_whole_width_so_floors_have_no_seams() {
    let mut d = vec![-1; 100 * 8];
    d[100 * 7..].fill(0);
    let node: TilemapNode = serde_json::from_value(json!({ "id": "m", "texture": "grid", "tile": 10, "cols": 100, "chunk": 4, "data": d, "solid": [0] })).unwrap();
    let floor = band_colliders(&node, &node.data, 4..8);
    assert_eq!(floor.len(), 1, "100 格宽的地面还是一整块");
    assert!(band_colliders(&node, &node.data, 0..4).is_empty());
}

#[test]
fn set_tiles_skips_bad_cells_and_unknown_maps() {
    let mut app = app();
    frame(&mut app, Some(json!({ "tilemaps": [node(&data())] })));
    app.command("setTiles", &json!({ "id": "nope", "tiles": [[0, 0, 0]] }));
    app.command("setTiles", &json!({ "id": "m", "tiles": [[20, 0, 0], [-1, 0, 0], [0, 12, 0], "x", [0, 0, 9999999999i64], [1, 0, 0]] }));
    frame(&mut app, None);
    let world = app.world_mut();
    let layer = world.query::<&TileLayer>().single(world).unwrap();
    assert_eq!(layer.frames().iter().filter(|&&f| f >= 0).count(), data().iter().filter(|&&f| f >= 0).count() + 1, "只有 [1, 0, 0] 生效");
    assert_eq!(rebuilt(&mut app).1, 1);
}

#[test]
fn a_map_without_solids_gets_a_nav_grid_once_one_is_dug_in() {
    let mut app = app();
    frame(&mut app, Some(json!({ "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 4, "data": vec![-1; 8], "solid": [0] }] })));
    assert_eq!(app.world_mut().query::<&NavGrid>().iter(app.world()).count(), 0);
    app.command("setTiles", &json!({ "id": "m", "tiles": [[1, 1, 0]] }));
    frame(&mut app, None);
    let world = app.world_mut();
    let nav = world.query::<&NavGrid>().single(world).unwrap();
    assert!(!nav.grid.open((1, 1)) && nav.grid.open((0, 0)));
}

#[test]
fn chunk_is_lenient_and_clamped() {
    let chunk = |v: Value| {
        let mut app = app();
        let mut m = node(&data());
        m["chunk"] = v;
        frame(&mut app, Some(json!({ "tilemaps": [m] })));
        let world = app.world_mut();
        world.query::<&TileLayer>().single(world).unwrap().chunk()
    };
    assert_eq!(chunk(json!("big")), 32, "写错当没写");
    assert_eq!(chunk(json!(1)), 4);
    assert_eq!(chunk(json!(1000)), 256);
    assert_eq!(chunk(json!(16)), 16);
}

#[test]
fn rule_changes_still_rebuild_the_whole_map_and_drop_set_tiles_edits() {
    let mut app = app();
    frame(&mut app, Some(json!({ "tilemaps": [node(&data())] })));
    app.command("setTiles", &json!({ "id": "m", "tiles": [[0, 0, 0]] }));
    frame(&mut app, None);
    let mut m = node(&data());
    m["friction"] = json!(0.9);
    frame(&mut app, Some(json!({ "tilemaps": [m] })));
    let world = app.world_mut();
    let layer = world.query::<&TileLayer>().single(world).unwrap();
    assert_eq!(layer.frame(0, 0), Some(-1), "整张重建回到声明的样子");
}

/// 45° 上坡（帧 2）从 (0, 7) 斜着爬到 (5, 2)，坡下实心（帧 0），右边是一块平台。
fn slope_map() -> (usize, usize, Vec<i32>) {
    let (cols, rows) = (12, 8);
    let mut d = vec![-1; cols * rows];
    for i in 0..6 {
        let r = 7 - i;
        d[r * cols + i] = 2;
        for rr in r + 1..rows {
            d[rr * cols + i] = 0;
        }
    }
    for c in 6..cols {
        for r in 2..rows {
            d[r * cols + c] = 0;
        }
    }
    (cols, rows, d)
}

fn slope_node(cols: usize, d: &[i32], chunk: u32) -> Value {
    json!({ "id": "m", "texture": "grid", "tile": 10, "cols": cols, "chunk": chunk, "data": d, "solid": [0], "slopes": [{ "frame": 2, "left": 0, "right": 1 }] })
}

#[test]
fn slopes_across_seams_build_the_same_colliders_as_one_band() {
    let (cols, _, d) = slope_map();
    let built = |chunk: u32| {
        let mut app = app();
        frame(&mut app, Some(json!({ "tilemaps": [slope_node(cols, &d, chunk)] })));
        colliders(&mut app)
    };
    let one = built(64);
    assert_eq!(built(4), one, "坡跨过第 4 行的接缝：两条合成一组，坡不被切开");
    assert_eq!(one.iter().filter(|c| c.contains("Slope")).count(), 1);
}

#[test]
fn edits_that_join_or_split_a_slope_across_a_seam_match_a_fresh_map() {
    let (cols, _, d) = slope_map();
    // 先把跨接缝的那一格（第 3 列第 4 行）挖掉：坡断成两段，两条分开
    let mut cut = d.clone();
    cut[4 * cols + 3] = -1;
    let fresh = |data: &[i32]| {
        let mut app = app();
        frame(&mut app, Some(json!({ "tilemaps": [slope_node(cols, data, 4)] })));
        colliders(&mut app)
    };
    let mut app = app();
    frame(&mut app, Some(json!({ "tilemaps": [slope_node(cols, &d, 4)] })));
    let groups = |app: &mut nge_ecs::App| {
        let world = app.world_mut();
        world.query::<&TileBodies>().single(world).unwrap().groups()
    };
    assert_eq!(groups(&mut app), 1);
    app.command("setTiles", &json!({ "id": "m", "tiles": [[3, 4, -1]] }));
    frame(&mut app, None);
    assert_eq!(groups(&mut app), 2, "坡断开，两条分开");
    assert_eq!(colliders(&mut app), fresh(&cut));
    app.command("setTiles", &json!({ "id": "m", "tiles": [[3, 4, 2]] }));
    frame(&mut app, None);
    assert_eq!(groups(&mut app), 1, "接上以后又合成一组");
    assert_eq!(colliders(&mut app), fresh(&d));
    // 接缝上面那一格（第 4 列第 3 行，上一条的最后一行）挖掉、再填回
    let mut above = d.clone();
    above[3 * cols + 4] = -1;
    app.command("setTiles", &json!({ "id": "m", "tiles": [[4, 3, -1]] }));
    frame(&mut app, None);
    assert_eq!(groups(&mut app), 2, "从上面断开也分开");
    assert_eq!(colliders(&mut app), fresh(&above));
    app.command("setTiles", &json!({ "id": "m", "tiles": [[4, 3, 2]] }));
    frame(&mut app, None);
    assert_eq!(groups(&mut app), 1);
    assert_eq!(colliders(&mut app), fresh(&d));
}

#[test]
fn a_box_slides_down_a_slope_the_same_whatever_the_chunk() {
    let (cols, rows) = (40usize, 26usize);
    let mut d = vec![-1; cols * rows];
    // 下坡（左 1 右 0）从 (0, 2) 到 (21, 23)，坡下实心；底下两行是地面
    for i in 0..22 {
        let r = 2 + i;
        d[r * cols + i] = 2;
        for rr in r + 1..rows {
            d[rr * cols + i] = 0;
        }
    }
    for c in 22..cols {
        for r in 24..rows {
            d[r * cols + c] = 0;
        }
    }
    let run = |chunk: u32| {
        let mut app = app();
        let map = json!({ "id": "m", "texture": "grid", "tile": 10, "cols": cols, "chunk": chunk, "data": d, "solid": [0], "friction": 0,
            "slopes": [{ "frame": 2, "left": 1, "right": 0 }] });
        let ball = json!({ "id": "b", "w": 6, "h": 6, "x": 5, "y": -16, "body": { "friction": 0 } });
        frame(&mut app, Some(json!({ "tilemaps": [map], "sprites": [ball] })));
        (0..60)
            .map(|_| {
                frame(&mut app, None);
                let world = app.world_mut();
                world.query::<(&crate::SceneId, &Transform2d)>().iter(world).find(|(id, _)| id.0 == "b").unwrap().1.translation
            })
            .collect::<Vec<_>>()
    };
    let (a, b) = (run(4), run(64));
    for (i, (p, q)) in a.iter().zip(&b).enumerate() {
        assert!((*p - *q).length() < 1e-3, "第 {i} 帧：chunk 4 {p}，chunk 64 {q}");
    }
    // 一直贴着坡面滑：离坡面（y = -x - 20）的距离不超过半个对角线加一点
    let off = |t: &glam::Vec2| (t.y + t.x + 20.0) / std::f32::consts::SQRT_2;
    assert!(a.iter().take_while(|t| t.x < 210.0).all(|t| off(t) < 4.5), "没在接缝上弹起来");
}

/// 第 4 … 8 行这一条（不从第 0 行开始）的碰撞体位置：斜坡、实心、单向平台都按条的起点往下挪。
#[test]
fn a_band_below_the_first_puts_its_colliders_at_their_rows() {
    let mut d = vec![-1; 6 * 8];
    d[5 * 6 + 1] = 2;
    d[6 * 6 + 1] = 0;
    d[7 * 6 + 1] = 0;
    d[6 * 6 + 4] = 0;
    d[5 * 6 + 3] = 1;
    d[5 * 6 + 4] = 1;
    let node: TilemapNode = serde_json::from_value(json!({ "id": "m", "texture": "grid", "tile": 10, "cols": 6, "x": 100, "y": 50, "data": d, "solid": [0], "oneWay": [1],
        "slopes": [{ "frame": 2, "left": 0, "right": 1 }] }))
    .unwrap();
    let mut got: Vec<String> = band_colliders(&node, &node.data, 4..8).iter().map(|(at, c)| format!("{at} {:?} {}", c.shape, c.one_way)).collect();
    got.sort();
    let mut want = vec![
        format!("{} {:?} false", glam::Vec2::new(115.0, -5.0), nge_physics2d::Shape::Slope { size: glam::Vec2::new(10.0, 10.0), left: 0.0, right: 1.0 }),
        format!("{} {:?} false", glam::Vec2::new(115.0, -20.0), nge_physics2d::Shape::Rect(glam::Vec2::new(10.0, 20.0))),
        format!("{} {:?} false", glam::Vec2::new(145.0, -15.0), nge_physics2d::Shape::Rect(glam::Vec2::new(10.0, 10.0))),
        format!("{} {:?} true", glam::Vec2::new(140.0, -5.0), nge_physics2d::Shape::Rect(glam::Vec2::new(20.0, 10.0))),
    ];
    want.sort();
    assert_eq!(got, want);
}

/// 静止压在地面上的球：同一条远处改一格，地面那块碰撞体不换实体，不多发 collide。
#[test]
fn far_edits_in_the_same_band_send_no_collide() {
    let (cols, rows) = (40usize, 8usize);
    let mut d = vec![-1; cols * rows];
    d[6 * cols..].fill(0);
    let mut app = app();
    let map = json!({ "id": "floor", "texture": "grid", "tile": 10, "cols": cols, "chunk": 4, "data": d, "solid": [0], "collide": true });
    let ball = json!({ "id": "ball", "w": 10, "x": 25, "y": -40, "body": { "shape": "circle", "collide": true } });
    frame(&mut app, Some(json!({ "tilemaps": [map], "sprites": [ball] })));
    for _ in 0..60 {
        frame(&mut app, None);
    }
    app.command("setTiles", &json!({ "id": "floor", "tiles": [[35, 4, 0]] }));
    let mut events: Vec<(String, Value)> = Vec::new();
    for _ in 0..5 {
        events.extend(frame(&mut app, None));
    }
    let collides: Vec<&Value> = events.iter().filter(|(n, _)| n == "collide").map(|(_, d)| d).collect();
    assert!(collides.is_empty(), "{collides:?}");
}

/// 站在两层叠着的单向板上往下穿，同一帧在同一条远处改一格：两层还是一起穿过去，落到地上。
#[test]
fn dropping_through_stacked_planks_survives_a_far_edit() {
    let (cols, rows) = (20usize, 8usize);
    let mut d = vec![-1; cols * rows];
    d[2 * cols..3 * cols].fill(1);
    d[3 * cols..4 * cols].fill(1);
    d[7 * cols..].fill(0);
    let mut app = app();
    let map = json!({ "id": "m", "texture": "grid", "tile": 10, "cols": cols, "x": -100, "y": 60, "chunk": 4, "data": d, "solid": [0], "oneWay": [1] });
    let hero = json!({ "id": "hero", "x": 0, "y": 50, "w": 10, "h": 10, "body": { "kind": "character" } });
    frame(&mut app, Some(json!({ "physics": { "gravity": [0, -1000] }, "tilemaps": [map], "sprites": [hero] })));
    for _ in 0..10 {
        frame(&mut app, None);
    }
    let y = |app: &mut nge_ecs::App| {
        let world = app.world_mut();
        world.query::<(&crate::SceneId, &Transform2d)>().iter(world).find(|(id, _)| id.0 == "hero").unwrap().1.translation.y
    };
    assert!((y(&mut app) - 45.0).abs() < 1.0, "站在上面那层板上 {}", y(&mut app));
    app.command("dropThrough", &json!({ "id": "hero" }));
    app.command("setTiles", &json!({ "id": "m", "tiles": [[19, 0, 0]] }));
    for _ in 0..30 {
        frame(&mut app, None);
    }
    assert!((y(&mut app) + 5.0).abs() < 1.0, "两层一起穿下去，落到地上 {}", y(&mut app));
}

fn change_ticks<C: Component>(app: &mut nge_ecs::App) -> u32 {
    let world = app.world_mut();
    let e = world.query_filtered::<Entity, With<C>>().single(world).unwrap();
    world.entity(e).get_change_ticks::<C>().unwrap().changed.get()
}

/// 只改了装饰帧、全部越界的 setTiles：地形、导航网格没变，不标改过（寻路图不重建）。
#[test]
fn edits_that_leave_the_terrain_alone_do_not_touch_nav_or_ground() {
    let mut d = vec![-1; 10 * 4];
    d[30..].fill(0);
    let node = |d: &[i32]| json!({ "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 10, "data": d, "solid": [0] }] });
    let mut app = app();
    frame(&mut app, Some(node(&d)));
    frame(&mut app, None);
    let before = (change_ticks::<GroundMap>(&mut app), change_ticks::<NavGrid>(&mut app));
    d[5] = 5;
    frame(&mut app, Some(node(&d)));
    app.command("setTiles", &json!({ "id": "m", "tiles": [[99, 99, 0], [6, 0, 7]] }));
    frame(&mut app, None);
    assert_eq!((change_ticks::<GroundMap>(&mut app), change_ticks::<NavGrid>(&mut app)), before);
    // 真改了地形才标
    app.command("setTiles", &json!({ "id": "m", "tiles": [[0, 0, 0]] }));
    frame(&mut app, None);
    assert!(change_ticks::<GroundMap>(&mut app) > before.0 && change_ticks::<NavGrid>(&mut app) > before.1);
}

/// 除了 `data` 以外任何一个字段变了都整张重建（`setTiles` 改的格子回到声明的样子）。
#[test]
fn every_field_but_data_rebuilds_the_whole_map() {
    let changes = [
        ("texture", json!("other")),
        ("grid", json!([2, 2])),
        ("tile", json!(12)),
        ("x", json!(-99)),
        ("y", json!(61)),
        ("z", json!(1)),
        ("parallax", json!(1)),
        ("tap", json!(false)),
        ("chunk", json!(8)),
        ("solid", json!([0, 5])),
        ("friction", json!(0.9)),
        ("restitution", json!(0.5)),
        ("collide", json!(true)),
        ("oneWay", json!([1, 5])),
        ("slopes", json!([{ "frame": 2, "left": 0, "right": 0.5 }])),
        ("animated", json!([{ "frame": 6, "frames": [6, 7], "fps": 2 }])),
        ("materials", json!([{ "frame": 0, "friction": 0.1 }])),
    ];
    for (field, value) in changes {
        let mut app = app();
        frame(&mut app, Some(json!({ "tilemaps": [node(&data())] })));
        app.command("setTiles", &json!({ "id": "m", "tiles": [[0, 0, 0]] }));
        frame(&mut app, None);
        let mut m = node(&data());
        m[field] = value;
        frame(&mut app, Some(json!({ "tilemaps": [m] })));
        let world = app.world_mut();
        let layer = world.query::<&TileLayer>().single(world).unwrap();
        assert_eq!(layer.frame(0, 0), Some(-1), "改了 {field} 整张重建");
    }
}

#[test]
fn set_tiles_on_an_invalid_map_changes_nothing() {
    let mut app = app();
    frame(&mut app, Some(json!({ "tilemaps": [{ "id": "bad", "texture": "grid", "tile": 0, "cols": 4, "data": vec![-1; 8] }] })));
    app.command("setTiles", &json!({ "id": "bad", "tiles": [[0, 0, 0]] }));
    frame(&mut app, None);
    assert_eq!(app.world_mut().query::<&TileLayer>().iter(app.world()).count(), 0);
    let message = "setTiles：瓦片地图 \"bad\" 本身无效（cols 是 0 或 tile 不是正数），没建出来，改不了".to_string();
    assert!(!crate::lenient::warn_once(message), "提示说清是地图本身无效，不是没有这个 id");
}
