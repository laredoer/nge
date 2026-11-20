//! 瓦片地图按块改：改一格只重建一条碰撞体，导航、地面、点地图原地改，结果和整张重建一样（见 docs/big-scene-perf.md）。

use bevy_ecs::prelude::*;
use nge_ai::{Ground, GroundMap, NavGrid};
use nge_physics2d::Collider;
use nge_render2d::{TileLayer, Transform2d};
use serde_json::{Value, json};

use crate::testing::{app, frame};
use crate::tile_chunks::{TileBodies, band_colliders};
use crate::tilemap::{TileSolid, TileTap, TilemapNode};

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
    let kept = before.iter().filter(|e| after.contains(e)).count();
    assert_eq!(kept, 1, "第 1 条（只有单向平台那一块）的碰撞体没动");
    let world = app.world_mut();
    let (layer, nav, ground, tap) = world.query::<(&TileLayer, &NavGrid, &GroundMap, &TileTap)>().single(world).unwrap();
    assert_eq!((layer.frame(4, 11), layer.frame(7, 1)), (Some(-1), Some(0)));
    assert!(nav.grid.open((4, 11)) && !nav.grid.open((7, 1)), "导航网格跟着改");
    assert_eq!((ground.terrain.at((4, 11)), ground.terrain.at((7, 1))), (Ground::Empty, Ground::Solid));
    assert_eq!((tap.data[11 * 20 + 4], tap.data[20 + 7]), (-1, 0), "点地图的数据跟着改");
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
