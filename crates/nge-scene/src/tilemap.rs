//! 瓦片地图（`scene.tilemaps`）：按图集的格子画出来（按块裁剪），实心格子合并成静态碰撞体（按块重建，见 docs/big-scene-perf.md）。

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_render2d::{SpriteImage, Transform2d};
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
    /// 哪些帧号是单向平台（从下面能跳上去，见 docs/gameplay2d.md）。
    #[serde(default)]
    pub one_way: Vec<i32>,
    /// 斜坡瓦片；写错的那一条丢掉。
    #[serde(default, deserialize_with = "crate::lenient::tile_slopes")]
    pub slopes: Vec<SlopeTile>,
    /// 动画瓦片：这个帧号的格子按帧序列循环。
    #[serde(default, deserialize_with = "crate::lenient::tile_animated")]
    pub animated: Vec<AnimatedTile>,
    /// 每种瓦片的摩擦、弹性（冰面），没写的用地图的。
    #[serde(default, deserialize_with = "crate::lenient::tile_materials")]
    pub materials: Vec<TileMaterial>,
    /// 点地图时发 `tap`，带点中的格子。
    #[serde(default)]
    pub tap: bool,
    /// 块的边长（格）：画的时候按块裁剪，改格子按块重建（见 docs/big-scene-perf.md）。不写是 32。
    #[serde(default, deserialize_with = "crate::lenient::tile_chunk")]
    pub chunk: Option<u32>,
}

/// 斜坡瓦片：底边平、两边竖直，顶边从左边高 `left` 到右边高 `right`（格高的几分之几，0 … 1）。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct SlopeTile {
    pub frame: i32,
    pub left: f32,
    pub right: f32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AnimatedTile {
    pub frame: i32,
    pub frames: Vec<u32>,
    pub fps: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct TileMaterial {
    pub frame: i32,
    pub friction: Option<f32>,
    pub restitution: Option<f32>,
}

/// 一格是什么：同一个帧号写在几处时斜坡优先，然后单向平台，最后实心。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TileKind {
    Empty,
    Solid,
    OneWay,
    Slope(f32, f32),
}

impl TileKind {
    /// 挡不挡会飞的。
    pub(crate) fn blocks_flyers(self) -> bool {
        matches!(self, TileKind::Solid | TileKind::Slope(..))
    }

    pub(crate) fn ground(self) -> nge_ai::Ground {
        match self {
            TileKind::Empty => nge_ai::Ground::Empty,
            TileKind::Solid => nge_ai::Ground::Solid,
            TileKind::OneWay => nge_ai::Ground::OneWay,
            TileKind::Slope(left, right) => nge_ai::Ground::Slope { left, right },
        }
    }
}

impl TilemapNode {
    fn kind(&self, frame: i32) -> TileKind {
        if frame < 0 {
            return TileKind::Empty;
        }
        if let Some(s) = self.slopes.iter().find(|s| s.frame == frame) {
            return TileKind::Slope(s.left, s.right);
        }
        if self.one_way.contains(&frame) {
            TileKind::OneWay
        } else if self.solid.contains(&frame) {
            TileKind::Solid
        } else {
            TileKind::Empty
        }
    }

    /// 同 `kind`，斜坡的高写错了当空格（warn 一次）。
    pub(crate) fn checked_kind(&self, frame: i32) -> TileKind {
        match self.kind(frame) {
            TileKind::Slope(left, right) if !(left.is_finite() && right.is_finite() && (0.0..=1.0).contains(&left) && (0.0..=1.0).contains(&right) && left.max(right) > 0.0) => {
                crate::lenient::warn_once(format!("瓦片地图 {:?} 的斜坡帧 {frame} 左右的高要是 0 … 1、不能都是 0，当空格", self.id));
                TileKind::Empty
            }
            kind => kind,
        }
    }

    /// 既是斜坡又写在单向平台里的帧号（按斜坡算，单向平台那条不起作用）。
    pub(crate) fn slopes_also_one_way(&self) -> Vec<i32> {
        let mut frames: Vec<i32> = self.slopes.iter().map(|s| s.frame).filter(|f| self.one_way.contains(f)).collect();
        frames.dedup();
        frames
    }

    /// 这种瓦片的（摩擦，弹性）。
    pub(crate) fn material(&self, frame: i32) -> (f32, f32) {
        let m = self.materials.iter().find(|m| m.frame == frame);
        let pick = |v: Option<f32>, fallback: f32| v.filter(|v| v.is_finite()).unwrap_or(fallback);
        (pick(m.and_then(|m| m.friction), self.friction), pick(m.and_then(|m| m.restitution), self.restitution))
    }
}

/// 点地图用：地图在哪、每格是什么帧（只给 `tap` 的地图挂）。
#[derive(Component, Debug, Clone, PartialEq)]
pub(crate) struct TileTap {
    pub origin: Vec2,
    pub tile: f32,
    pub cols: usize,
    pub data: Vec<i32>,
    pub z: f32,
    pub parallax: Option<nge_render2d::Parallax>,
}

impl TileTap {
    /// 画面上的世界坐标 `p` 落在哪一格（列，行）和那一格的帧号；地图外、空格是 `None`。
    pub fn at(&self, p: Vec2, camera: &nge_render2d::Camera2d) -> Option<([i32; 2], i32)> {
        let origin = nge_render2d::drawn(&Transform2d::from_xy(self.origin.x, self.origin.y), None, self.parallax.as_ref(), camera).translation;
        let local = Vec2::new(p.x - origin.x, origin.y - p.y) / self.tile;
        let (c, r) = (local.x.floor(), local.y.floor());
        let rows = self.data.len() / self.cols.max(1);
        if !(c >= 0.0 && r >= 0.0 && (c as usize) < self.cols && (r as usize) < rows) {
            return None;
        }
        let (c, r) = (c as usize, r as usize);
        let frame = self.data[r * self.cols + c];
        (frame >= 0).then_some(([c as i32, r as i32], frame))
    }
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

/// 块的边长（格）：写错、超出 4 … 256 的夹到范围里。
pub(crate) fn chunk_of(node: &TilemapNode) -> usize {
    match node.chunk {
        None => nge_render2d::DEFAULT_CHUNK,
        Some(c) if (MIN_CHUNK..=MAX_CHUNK).contains(&c) => c as usize,
        Some(c) => {
            crate::lenient::warn_once(format!("瓦片地图 {:?} 的 chunk {c} 超出 {MIN_CHUNK} … {MAX_CHUNK}，夹到范围里", node.id));
            c.clamp(MIN_CHUNK, MAX_CHUNK) as usize
        }
    }
}

const MIN_CHUNK: u32 = 4;
const MAX_CHUNK: u32 = 256;

/// 地图上的东西要不要导航网格、地面地形：带视差的不要（纯装饰的层不该让追击的角色以为一路畅通）。
fn walkable(node: &TilemapNode) -> bool {
    ParallaxNode::resolve(node.parallax, &node.id).is_none_or(|p| p.0 == Vec2::ONE)
}

/// 除了 `data` 都一样（`data` 长度也一样）：只改了格子，按块局部改。
fn only_cells_changed(a: &TilemapNode, b: &TilemapNode) -> bool {
    type Shape<'a> = (&'a str, &'a str, Option<[u32; 2]>, u32, u32, u32, u32, u32, Option<ParallaxNode>, bool, Option<u32>);
    type Rules<'a> = (&'a [i32], u32, u32, bool, &'a [i32], &'a [SlopeTile], &'a [AnimatedTile], &'a [TileMaterial]);
    fn shape(n: &TilemapNode) -> Shape<'_> {
        (&n.id, &n.texture, n.grid, n.tile.to_bits(), n.cols, n.x.to_bits(), n.y.to_bits(), n.z.to_bits(), n.parallax, n.tap, n.chunk)
    }
    fn rules(n: &TilemapNode) -> Rules<'_> {
        (&n.solid, n.friction.to_bits(), n.restitution.to_bits(), n.collide, &n.one_way, &n.slopes, &n.animated, &n.materials)
    }
    a.data.len() == b.data.len() && shape(a) == shape(b) && rules(a) == rules(b)
}

pub(crate) type MapParts<'a> = (&'a mut nge_render2d::TileLayer, Option<&'a mut nge_ai::NavGrid>, Option<&'a mut nge_ai::GroundMap>, Option<&'a mut TileTap>, &'a mut crate::tile_chunks::TileBodies);

pub(crate) fn sync_tilemaps(
    mut commands: Commands,
    scene: Res<Scene>,
    mut assets: SceneAssets,
    nodes: Query<(Entity, &Declared<TilemapNode>)>,
    mut maps: Query<(&mut nge_render2d::TileLayer, Option<&mut nge_ai::NavGrid>, Option<&mut nge_ai::GroundMap>, Option<&mut TileTap>, &mut crate::tile_chunks::TileBodies)>,
) {
    sync_list(&mut commands, &scene.tilemaps, &nodes, |commands, map, before, node| {
        // 只改了格子：按块局部改（见 docs/big-scene-perf.md）
        if let Some(before) = before.filter(|b| only_cells_changed(b, node))
            && let Ok((mut layer, mut nav, mut ground, mut tap, mut bodies)) = maps.get_mut(map)
        {
            let cols = node.cols as usize;
            let edits = before.data.iter().zip(&node.data).enumerate().filter(|(_, (a, b))| a != b).map(|(i, (_, &f))| (i % cols, i / cols, f));
            let parts: MapParts = (&mut layer, nav.as_deref_mut(), ground.as_deref_mut(), tap.as_deref_mut(), &mut bodies);
            crate::tile_chunks::edit_map(commands, map, node, parts, edits);
            return;
        }
        build_map(commands, &mut assets, map, node);
    });
}

/// 整张重建：瓦片层、导航网格、地面地形、点地图的数据、一条条的碰撞体。
fn build_map(commands: &mut Commands, assets: &mut SceneAssets, map: Entity, node: &TilemapNode) {
    commands.entity(map).despawn_related::<Children>().remove::<(nge_ai::NavGrid, nge_ai::GroundMap, TileTap, nge_render2d::TileLayer, crate::tile_chunks::TileBodies)>();
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
    let data = &node.data[..rows * cols];
    // 动画瓦片跟着场景的时钟走：改了地图的规则、整张重建也不从头播
    let anims = node
        .animated
        .iter()
        .filter_map(|a| {
            if a.frames.is_empty() || !(a.fps > 0.0 && a.fps.is_finite()) {
                crate::lenient::warn_once(format!("瓦片地图 {:?} 的动画瓦片 {} 要有帧、fps 要是正数，不动", node.id, a.frame));
                return None;
            }
            Some(nge_render2d::TileAnim { frame: a.frame, frames: a.frames.clone(), fps: a.fps })
        })
        .collect();
    let chunk = chunk_of(node);
    let mut layer = nge_render2d::TileLayer::new(image, origin, tile, cols, data.to_vec(), chunk, anims, node.z);
    layer.parallax = parallax.map(|p| p.0);
    for frame in node.slopes_also_one_way() {
        crate::lenient::warn_once(format!("瓦片地图 {:?} 的帧 {frame} 既是斜坡又写在 oneWay 里，按斜坡算（斜坡不能单向）", node.id));
    }
    let kinds: Vec<TileKind> = data.iter().map(|&v| node.checked_kind(v)).collect();
    if parallax.is_some_and(|p| p.0 != Vec2::ONE) && kinds.iter().any(|k| *k != TileKind::Empty) {
        log::warn!("瓦片地图 {:?} 设了视差又有实心格子：碰撞体在声明的位置，和画面对不上", node.id);
    }
    let mut entity = commands.entity(map);
    entity.insert(layer);
    if let Some(nav) = nav_grid(node, &kinds) {
        entity.insert(nav);
    }
    if let Some(ground) = ground_map(node, &kinds) {
        entity.insert(ground);
    }
    if node.tap {
        entity.insert(TileTap { origin, tile, cols, data: data.to_vec(), z: node.z, parallax });
    }
    let bodies = crate::tile_chunks::TileBodies::build(commands, map, node, data, chunk);
    commands.entity(map).insert(bodies);
}

/// 导航网格只给有实心格子、不带视差的地图。会飞的从单向平台穿过去，斜坡挡住。
pub(crate) fn nav_grid(node: &TilemapNode, kinds: &[TileKind]) -> Option<nge_ai::NavGrid> {
    let solid = kinds.iter().any(|k| matches!(k, TileKind::Solid | TileKind::Slope(..)));
    (solid && walkable(node)).then(|| nge_ai::NavGrid { grid: nge_ai::Grid::new(node.cols as usize, kinds.iter().map(|k| k.blocks_flyers()).collect()), origin: Vec2::new(node.x, node.y), tile: node.tile })
}

/// 走地面的寻路用的地形（见 docs/gameplay2d.md）：同样只给不带视差的地图。
pub(crate) fn ground_map(node: &TilemapNode, kinds: &[TileKind]) -> Option<nge_ai::GroundMap> {
    let any = kinds.iter().any(|k| *k != TileKind::Empty);
    (any && walkable(node)).then(|| nge_ai::GroundMap { terrain: nge_ai::Terrain::new(node.cols as usize, kinds.iter().map(|k| k.ground()).collect()), origin: Vec2::new(node.x, node.y), tile: node.tile })
}

#[cfg(test)]
mod tests {
    use bevy_ecs::system::RunSystemOnce;
    use nge_physics2d::{Collider, Physics, Shape};
    use serde_json::json;

    use super::*;
    use crate::testing::{app, frame};

    /// 每张地图画出来的格子：（格子中心，帧号）。
    pub(crate) fn tiles(app: &mut nge_ecs::App) -> Vec<(Vec2, u32)> {
        let world = app.world_mut();
        let mut v: Vec<(Vec2, u32)> = Vec::new();
        for layer in world.query::<&nge_render2d::TileLayer>().iter(world) {
            for r in 0..layer.rows() {
                for c in 0..layer.cols() {
                    if let Some(f) = layer.drawn_frame(c, r, 0.0) {
                        v.push((layer.origin() + Vec2::new((c as f32 + 0.5) * layer.tile(), -(r as f32 + 0.5) * layer.tile()), f));
                    }
                }
            }
        }
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
        let layer = world.query::<&nge_render2d::TileLayer>().iter(world).next().unwrap();
        assert_eq!((layer.tile(), layer.z, layer.chunk()), (10.0, 2.0, 32));
        assert!(solids(&mut app).is_empty(), "不写 solid 就不碰撞");
    }

    #[test]
    fn parallax_applies_to_every_tile_but_not_the_colliders() {
        let mut app = app();
        frame(&mut app, Some(json!({ "tilemaps": [{ "id": "m", "texture": "grid", "tile": 10, "cols": 2, "data": [0, 1, 1, -1], "solid": [1], "parallax": [0.4, 1] }] })));
        let world = app.world_mut();
        let drawn: Vec<Option<Vec2>> = world.query::<&nge_render2d::TileLayer>().iter(world).map(|l| l.parallax).collect();
        assert_eq!(drawn, [Some(Vec2::new(0.4, 1.0))]);
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
