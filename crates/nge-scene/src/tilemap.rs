//! 瓦片地图（`scene.tilemaps`）：按图集的格子画出来，实心格子合并成静态碰撞体。

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use glam::{Vec2, Vec4};
use nge_physics2d::Collider;
use nge_render2d::{Sprite, SpriteImage, Transform2d};
use serde::Deserialize;

use crate::protocol::{ParallaxNode, Scene};
use crate::resolve::SceneAssets;
use crate::sync::{Declared, Node, sync_list};

/// 一张瓦片地图。`data` 一行一行从上往下，每格是图集帧号，-1 是空格；`(x, y)` 是左上角。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TilemapNode {
    pub id: String,
    pub texture: String,
    pub grid: Option<[u32; 2]>,
    pub tile: f32,
    pub cols: u32,
    pub data: Vec<i32>,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub z: f32,
    #[serde(default)]
    pub solid: Vec<i32>,
    #[serde(default = "default_friction")]
    pub friction: f32,
    #[serde(default)]
    pub restitution: f32,
    #[serde(default)]
    pub collide: bool,
    /// 视差，加在每个瓦片上；碰撞体不跟着挪（见 docs/parallax.md）。
    pub parallax: Option<ParallaxNode>,
}

fn default_friction() -> f32 {
    0.5
}

impl Node for TilemapNode {
    fn id(&self) -> &str {
        &self.id
    }
}

/// 地图的实心碰撞体（子实体），转 `collide` 事件时按 `ChildOf` 找地图 id。
#[derive(Component)]
pub(crate) struct TileSolid;

/// 一块合并后的实心矩形：左上角格子和宽高（格数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Run {
    pub col: usize,
    pub row: usize,
    pub w: usize,
    pub h: usize,
}

/// 把实心格子合并成矩形：每行先切成尽量长的横条，再把上下列和宽度都一样的横条并成一块。
/// 横条优先，平地永远是一整块，刚体沿地面滑不会碰到接缝。
pub(crate) fn merge(solid: &[bool], cols: usize) -> Vec<Run> {
    let rows = if cols == 0 { 0 } else { solid.len() / cols };
    let mut runs: Vec<Run> = Vec::new();
    // 上一行结束的横条：(起始列, 宽) → runs 里的下标
    let mut open: HashMap<(usize, usize), usize> = HashMap::new();
    for row in 0..rows {
        let line = &solid[row * cols..(row + 1) * cols];
        let mut next = HashMap::new();
        let mut col = 0;
        while col < cols {
            if !line[col] {
                col += 1;
                continue;
            }
            let w = line[col..].iter().take_while(|&&s| s).count();
            let index = match open.get(&(col, w)) {
                Some(&i) => {
                    runs[i].h += 1;
                    i
                }
                None => {
                    runs.push(Run { col, row, w, h: 1 });
                    runs.len() - 1
                }
            };
            next.insert((col, w), index);
            col += w;
        }
        open = next;
    }
    runs
}

pub(crate) fn sync_tilemaps(mut commands: Commands, scene: Res<Scene>, mut assets: SceneAssets, nodes: Query<(Entity, &Declared<TilemapNode>)>) {
    sync_list(&mut commands, &scene.tilemaps, &nodes, |commands, map, _, node| {
        commands.entity(map).despawn_related::<Children>();
        if node.cols == 0 || !(node.tile > 0.0 && node.tile.is_finite()) {
            log::warn!("瓦片地图 {:?} 的 cols 是 0 或 tile 不是正数，不画", node.id);
            return;
        }
        let cols = node.cols as usize;
        let rows = node.data.len() / cols;
        if node.data.len() % cols != 0 {
            log::warn!("瓦片地图 {:?} 的 data 长度 {} 不是 cols {} 的整数倍，最后不满一行的不画", node.id, node.data.len(), cols);
        }
        let image = assets.texture(&node.id, Some(&node.texture), node.grid, 0);
        if matches!(image, SpriteImage::Texture(_)) {
            log::warn!("瓦片地图 {:?} 的 texture 不是图集，每格都画整张图", node.id);
        }
        let parallax = ParallaxNode::resolve(node.parallax, &node.id);
        let origin = Vec2::new(node.x, node.y);
        let tile = node.tile;
        let center = |c: f32, r: f32| origin + Vec2::new(c * tile, -r * tile);
        let data = &node.data[..rows * cols];
        for (i, &value) in data.iter().enumerate() {
            let Ok(frame) = u32::try_from(value) else { continue };
            let image = match image {
                SpriteImage::Atlas(atlas, _) => SpriteImage::Atlas(atlas, frame),
                texture => texture,
            };
            let at = center((i % cols) as f32 + 0.5, (i / cols) as f32 + 0.5);
            let sprite = Sprite { image, size: Some(Vec2::splat(tile)), color: Vec4::ONE };
            let mut tile = commands.spawn((sprite, Transform2d::from_xy(at.x, at.y).with_z(node.z), ChildOf(map)));
            if let Some(p) = parallax {
                tile.insert(p);
            }
        }
        // 负数都是空格，写进 solid 也不算实心
        let solid: Vec<bool> = data.iter().map(|v| *v >= 0 && node.solid.contains(v)).collect();
        if parallax.is_some_and(|p| p.0 != Vec2::ONE) && solid.contains(&true) {
            log::warn!("瓦片地图 {:?} 设了视差又有实心格子：碰撞体在声明的位置，和画面对不上", node.id);
        }
        for run in merge(&solid, cols) {
            let size = Vec2::new(run.w as f32, run.h as f32) * tile;
            let at = center(run.col as f32 + run.w as f32 / 2.0, run.row as f32 + run.h as f32 / 2.0);
            let collider = Collider { restitution: node.restitution, friction: node.friction, events: node.collide, ..Collider::rect(size) };
            commands.spawn((collider, Transform2d::from_xy(at.x, at.y), TileSolid, ChildOf(map)));
        }
    });
}

#[cfg(test)]
mod tests {
    use bevy_ecs::system::RunSystemOnce;
    use nge_physics2d::{Physics, Shape};
    use serde_json::json;

    use super::*;
    use crate::testing::{app, frame};

    fn tiles(app: &mut nge_ecs::App) -> Vec<(Vec2, u32)> {
        let world = app.world_mut();
        let mut v: Vec<(Vec2, u32)> = world
            .query_filtered::<(&Sprite, &Transform2d), With<ChildOf>>()
            .iter(world)
            .map(|(s, t)| (t.translation, if let SpriteImage::Atlas(_, f) = s.image { f } else { u32::MAX }))
            .collect();
        v.sort_by(|a, b| (a.0.x, -a.0.y).partial_cmp(&(b.0.x, -b.0.y)).unwrap());
        v
    }

    fn solids(app: &mut nge_ecs::App) -> Vec<(Vec2, Shape)> {
        let world = app.world_mut();
        world.query_filtered::<(&Transform2d, &Collider), With<TileSolid>>().iter(world).map(|(t, c)| (t.translation, c.shape)).collect()
    }

    #[test]
    fn merge_makes_large_rectangles() {
        let grid = [1, 1, 0, 1, 1, 0, 0, 1, 1].map(|v| v == 1);
        assert_eq!(merge(&grid, 3), [Run { col: 0, row: 0, w: 2, h: 2 }, Run { col: 1, row: 2, w: 2, h: 1 }]);
        let bump = [0, 1, 0, 1, 1, 1].map(|v| v == 1);
        assert_eq!(merge(&bump, 3), [Run { col: 1, row: 0, w: 1, h: 1 }, Run { col: 0, row: 1, w: 3, h: 1 }], "地面上立着一块时地面还是一整块");
        let walls = [1, 0, 1, 1, 0, 1, 1, 1, 1].map(|v| v == 1);
        assert_eq!(merge(&walls, 3).len(), 3, "两面墙各一块，地面一块");
        assert_eq!(merge(&[true; 6], 3), [Run { col: 0, row: 0, w: 3, h: 2 }]);
        assert!(merge(&[false; 4], 2).is_empty() && merge(&[], 0).is_empty());
    }

    #[test]
    fn tiles_are_drawn_from_the_top_left() {
        let mut app = app();
        frame(&mut app, Some(json!({ "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 3, "x": 100, "y": 50, "z": 2, "data": [0, -1, 1, 2, 2, -1, 7] }] })));
        let expected = [(Vec2::new(105.0, 45.0), 0), (Vec2::new(105.0, 35.0), 2), (Vec2::new(115.0, 35.0), 2), (Vec2::new(125.0, 45.0), 1)];
        assert_eq!(tiles(&mut app), expected, "不满一行的 7 不画");
        let world = app.world_mut();
        let (s, t) = world.query_filtered::<(&Sprite, &Transform2d), With<ChildOf>>().iter(world).next().unwrap();
        assert_eq!((s.size, t.z), (Some(Vec2::splat(10.0)), 2.0));
        assert!(solids(&mut app).is_empty(), "不写 solid 就不碰撞");
    }

    #[test]
    fn parallax_applies_to_every_tile_but_not_the_colliders() {
        let mut app = app();
        frame(&mut app, Some(json!({ "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 2, "data": [0, 1, 1, -1], "solid": [1], "parallax": [0.4, 1] }] })));
        let world = app.world_mut();
        let drawn: Vec<Option<nge_render2d::Parallax>> = world.query_filtered::<Option<&nge_render2d::Parallax>, (With<Sprite>, With<ChildOf>)>().iter(world).map(|p| p.copied()).collect();
        assert_eq!(drawn, [Some(nge_render2d::Parallax(Vec2::new(0.4, 1.0))); 3]);
        assert_eq!(world.query_filtered::<(), (With<TileSolid>, With<nge_render2d::Parallax>)>().iter(world).count(), 0);
    }

    #[test]
    fn solid_tiles_become_merged_colliders_and_rebuild() {
        let mut app = app();
        let map = |data: Vec<i32>| json!({ "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 3, "data": data, "solid": [1, 2] }] });
        frame(&mut app, Some(map(vec![1, 2, 0, -1, 1, 1])));
        let mut s = solids(&mut app);
        s.sort_by(|a, b| a.0.x.partial_cmp(&b.0.x).unwrap());
        assert_eq!(s, [(Vec2::new(10.0, -5.0), Shape::Rect(Vec2::new(20.0, 10.0))), (Vec2::new(20.0, -15.0), Shape::Rect(Vec2::new(20.0, 10.0)))]);
        frame(&mut app, Some(map(vec![1, 1, 1])));
        assert_eq!(solids(&mut app), [(Vec2::new(15.0, -5.0), Shape::Rect(Vec2::new(30.0, 10.0)))], "data 变了整张重建");
        assert_eq!(app.world().resource::<Physics>().len().1, 1, "物理里也只剩新的一块");
        assert_eq!(tiles(&mut app).len(), 3);
        frame(&mut app, Some(json!({})));
        assert!(tiles(&mut app).is_empty() && solids(&mut app).is_empty());
        assert_eq!(app.world().resource::<Physics>().len().1, 0, "碰撞体也删了");
    }

    #[test]
    fn negative_values_are_never_solid() {
        let mut app = app();
        frame(&mut app, Some(json!({ "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 2, "data": [-1, -2], "solid": [-1, -2] }] })));
        assert!(solids(&mut app).is_empty() && tiles(&mut app).is_empty());
    }

    #[test]
    fn touching_several_pieces_is_one_contact() {
        let mut app = app();
        // 墙角：左边一列墙、底下一行地，合并成两块；球落在墙角同时碰到两块
        let scene = json!({
            "physics": { "gravity": [-300, -981] },
            "tilemaps": [{ "id": "level", "texture": "grid", "tile": 10, "cols": 4, "x": -20, "y": 40,
                "data": [0, -1, -1, -1, 0, -1, -1, -1, 0, -1, -1, -1, 0, 0, 0, 0], "solid": [0] }],
            "sprites": [{ "id": "ball", "w": 10, "x": 0, "y": 20, "body": { "shape": "circle", "collide": true } }],
        });
        let mut events = frame(&mut app, Some(scene.clone()));
        for _ in 0..20 {
            events.extend(frame(&mut app, None));
        }
        let collides: Vec<bool> = events.iter().filter(|(n, _)| n == "collide").map(|(_, d)| d["started"].as_bool().unwrap()).collect();
        assert_eq!(collides, [true], "碰到墙和地两块也只发一次 started");
        let mut rebuilt = scene;
        rebuilt["tilemaps"][0]["friction"] = json!(0.9);
        let mut events = frame(&mut app, Some(rebuilt));
        for _ in 0..5 {
            events.extend(frame(&mut app, None));
        }
        let collides: Vec<bool> = events.iter().filter(|(n, _)| n == "collide").map(|(_, d)| d["started"].as_bool().unwrap()).collect();
        assert!(collides.len() % 2 == 0 && collides.chunks(2).all(|p| p == [false, true]), "重建时要么不发，要么成对发 {collides:?}");
        frame(&mut app, Some(json!({ "sprites": [{ "id": "ball", "w": 10, "body": { "shape": "circle", "collide": true } }] })));
        frame(&mut app, None);
        let state = app.world_mut().run_system_once(|q: Query<&Children>| q.iter().count()).unwrap();
        assert_eq!(state, 0);
    }

    #[test]
    fn every_ball_gets_paired_events_when_the_map_rebuilds() {
        let mut app = app();
        let ball = |id: &str, x: f32, y: f32| json!({ "id": id, "w": 10, "x": x, "y": y, "body": { "shape": "circle", "collide": true } });
        let scene = |friction: f32, y: f32| json!({
            "tilemaps": [{ "id": "floor", "texture": "grid", "tile": 10, "cols": 6, "x": -30, "y": 0, "data": [0, 0, 0, 0, 0, 0], "solid": [0], "friction": friction }],
            "sprites": [ball("b1", -10.0, y), ball("b2", 10.0, y)],
        });
        let record = |events: &[(String, serde_json::Value)], log: &mut Vec<String>| {
            for (_, d) in events.iter().filter(|(n, _)| n == "collide") {
                let other = if d["a"] == "floor" { &d["b"] } else { &d["a"] };
                log.push(format!("{}-{}", other.as_str().unwrap(), d["started"]));
            }
        };
        let run = |app: &mut nge_ecs::App, scene: Option<serde_json::Value>, frames: usize| {
            let mut log = Vec::new();
            record(&frame(app, scene), &mut log);
            for _ in 0..frames {
                record(&frame(app, None), &mut log);
            }
            log.sort();
            log
        };
        assert_eq!(run(&mut app, Some(scene(0.5, 20.0)), 10), ["b1-true", "b2-true"]);
        let rebuilt = run(&mut app, Some(scene(0.9, 20.0)), 3);
        assert!(rebuilt.is_empty() || rebuilt == ["b1-false", "b1-true", "b2-false", "b2-true"], "每个球都成对 {rebuilt:?}");
        // 声明的位置变了就瞬移：两个球都抬到半空
        assert_eq!(run(&mut app, Some(scene(0.9, 200.0)), 1), ["b1-false", "b2-false"]);
    }

    #[test]
    fn bad_maps_draw_nothing() {
        let mut app = app();
        frame(&mut app, Some(json!({ "tilemaps": [
            { "id": "a", "texture": "grid", "tile": 10, "cols": 0, "data": [0] },
            { "id": "b", "texture": "grid", "tile": -1, "cols": 1, "data": [0], "solid": [0] },
        ] })));
        assert!(tiles(&mut app).is_empty() && solids(&mut app).is_empty());
    }

    #[test]
    fn balls_land_on_the_map_and_collide_with_its_id() {
        let mut app = app();
        let scene = json!({
            "tilemaps": [{ "id": "floor", "texture": "grid", "tile": 10, "cols": 4, "x": -20, "y": 0, "data": [0, 0, 0, 0], "solid": [0], "collide": true }],
            "sprites": [{ "id": "ball", "w": 10, "y": 30, "body": { "shape": "circle" } }],
        });
        let mut events = frame(&mut app, Some(scene));
        for _ in 0..10 {
            events.extend(frame(&mut app, None));
        }
        let world = app.world_mut();
        let y = world.query::<(&crate::SceneId, &Transform2d)>().iter(world).find(|(id, _)| id.0 == "ball").unwrap().1.translation.y;
        assert!((y - 5.0).abs() < 1.0, "落在地图上面（地图顶边 y=0，球半径 5）{y}");
        let hit = events.iter().find(|(n, _)| n == "collide").expect("撞到地图").1.clone();
        let ids = [hit["a"].as_str().unwrap(), hit["b"].as_str().unwrap()];
        assert!(ids.contains(&"floor") && ids.contains(&"ball"), "{hit}");
    }
}
