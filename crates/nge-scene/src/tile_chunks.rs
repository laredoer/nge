//! 瓦片地图按块改：碰撞体按横条（`chunk` 行、整张地图宽）建，改一格只重建它那一条；
//! 瓦片层、导航网格、地面地形、点地图的数据原地改（见 docs/big-scene-perf.md）。

use std::collections::BTreeSet;

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_ecs::CommandKey;
use nge_physics2d::Collider;
use nge_render2d::Transform2d;
use serde::Deserialize;

use crate::SceneId;
use crate::sync::Declared;
use crate::tilemap::{MapParts, Run, TileKind, TileSolid, TilemapNode, ground_map, merge, nav_grid};

/// 改地图上的格子：`tiles` 是一串 `[列, 行, 帧号]`，帧号 -1 是挖空。改到下一次声明里这一格变了、或者整张重建为止。
pub const SET_TILES: CommandKey<SetTiles> = CommandKey::new("setTiles");

#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
pub struct SetTiles {
    pub id: String,
    /// 写错的那一项跳过。
    #[serde(default, deserialize_with = "crate::lenient::set_tiles")]
    pub tiles: Vec<[i64; 3]>,
}

/// 组件：地图的碰撞体，一条一组。有坡跨过的接缝两边的条合成一组，碰撞体记在这一组的第一条上。
#[derive(Component, Debug, Default)]
pub(crate) struct TileBodies {
    bands: Vec<Vec<(Entity, Vec2, Collider)>>,
    /// `joined[k]`：第 k 条和第 k + 1 条之间的接缝有坡跨过。
    joined: Vec<bool>,
    /// 一条几行。
    rows: usize,
    /// 一共重建过几组（整张建的不算）。
    rebuilt: u64,
}

impl TileBodies {
    /// 整张建：每组的碰撞体。
    pub(crate) fn build(commands: &mut Commands, map: Entity, node: &TilemapNode, data: &[i32], rows: usize) -> Self {
        let total = data.len() / (node.cols as usize).max(1);
        let n = total.div_ceil(rows.max(1));
        let joined = (1..n).map(|k| seam_joined(node, data, k * rows)).collect();
        let mut bodies = Self { bands: vec![Vec::new(); n], joined, rows: rows.max(1), rebuilt: 0 };
        let mut b = 0;
        while b < n {
            let group = bodies.group(b);
            bodies.bands[b] = spawn(commands, map, band_colliders(node, data, bodies.band_rows(&group, total)));
            b = group.end;
        }
        bodies
    }

    /// 第 `b` 条所在的组（条的下标范围）。
    fn group(&self, b: usize) -> std::ops::Range<usize> {
        let (mut first, mut last) = (b, b);
        while first > 0 && self.joined[first - 1] {
            first -= 1;
        }
        while last < self.joined.len() && self.joined[last] {
            last += 1;
        }
        first..last + 1
    }

    fn band_rows(&self, group: &std::ops::Range<usize>, total: usize) -> std::ops::Range<usize> {
        group.start * self.rows..(group.end * self.rows).min(total)
    }

    #[cfg(test)]
    pub(crate) fn rebuilt(&self) -> u64 {
        self.rebuilt
    }

    /// 现在有几组。
    #[cfg(test)]
    pub(crate) fn groups(&self) -> usize {
        self.joined.iter().filter(|j| !**j).count() + usize::from(!self.bands.is_empty())
    }
}

fn spawn(commands: &mut Commands, map: Entity, colliders: Vec<(Vec2, Collider)>) -> Vec<(Entity, Vec2, Collider)> {
    colliders.into_iter().map(|(at, c)| (commands.spawn((c, Transform2d::from_xy(at.x, at.y), TileSolid, ChildOf(map))).id(), at, c)).collect()
}

/// 第 `seam` 行上面那一行和它之间有没有连着的坡（同一条直线上相邻的两格分在接缝两边）。
fn seam_joined(node: &TilemapNode, data: &[i32], seam: usize) -> bool {
    let cols = node.cols as usize;
    let kinds: Vec<TileKind> = data[(seam - 1) * cols..(seam + 1) * cols].iter().map(|&v| node.checked_kind(v)).collect();
    crate::tile_slopes::crosses(&kinds, cols)
}

/// 新算的碰撞体和旧的比：一样的（形状、位置都一样）从 `pool` 里拿出原来的实体留着（物理的句柄、接触都不变），变了的新建。
fn reuse(commands: &mut Commands, map: Entity, pool: &mut Vec<(Entity, Vec2, Collider)>, new: Vec<(Vec2, Collider)>) -> Vec<(Entity, Vec2, Collider)> {
    let mut out = Vec::with_capacity(new.len());
    let mut fresh = Vec::new();
    for (at, c) in new {
        match pool.iter().position(|&(_, a, o)| a == at && o == c) {
            Some(i) => out.push(pool.swap_remove(i)),
            None => fresh.push((at, c)),
        }
    }
    out.extend(spawn(commands, map, fresh));
    out
}

/// 第 `rows` 行这一条的碰撞体（中心、碰撞体）。斜坡连着的并成一块，盖住的实心格子不再单独合并；
/// 实心格子按材质分开、横条优先合并；单向平台一行一块。
pub(crate) fn band_colliders(node: &TilemapNode, data: &[i32], rows: std::ops::Range<usize>) -> Vec<(Vec2, Collider)> {
    let cols = node.cols as usize;
    let (origin, tile, r0) = (Vec2::new(node.x, node.y), node.tile, rows.start);
    let data = &data[rows.start * cols..rows.end * cols];
    let center = |c: f32, r: f32| origin + Vec2::new(c * tile, -(r + r0 as f32) * tile);
    let kinds: Vec<TileKind> = data.iter().map(|&v| node.checked_kind(v)).collect();
    let mut out = Vec::new();
    let (pieces, covered) = crate::tile_slopes::slope_pieces(&kinds, cols);
    for p in pieces {
        let (friction, restitution) = node.material(data[p.first]);
        let size = Vec2::new(p.w as f32, p.h) * tile;
        let collider = Collider { restitution, friction, events: node.collide, ..Collider::slope(size, p.left, p.right) };
        let bottom = origin.y - (p.bottom + r0 + 1) as f32 * tile;
        out.push((Vec2::new(origin.x + (p.col as f32 + p.w as f32 / 2.0) * tile, bottom + size.y / 2.0), collider));
    }
    let kinds: Vec<TileKind> = kinds.iter().zip(&covered).map(|(&k, &c)| if c { TileKind::Empty } else { k }).collect();
    let mut materials: Vec<(f32, f32)> = Vec::new();
    for (&v, k) in data.iter().zip(&kinds) {
        let m = node.material(v);
        if matches!(k, TileKind::Solid | TileKind::OneWay) && !materials.contains(&m) {
            materials.push(m);
        }
    }
    for &(friction, restitution) in &materials {
        let base = Collider { restitution, friction, events: node.collide, ..Collider::rect(Vec2::ZERO) };
        let mask = |kind: TileKind| -> Vec<bool> { data.iter().zip(&kinds).map(|(&v, &k)| k == kind && node.material(v) == (friction, restitution)).collect() };
        for Run { col, row, w, h } in merge(&mask(TileKind::Solid), cols) {
            let size = Vec2::new(w as f32, h as f32) * tile;
            out.push((center(col as f32 + w as f32 / 2.0, row as f32 + h as f32 / 2.0), Collider { shape: Collider::rect(size).shape, ..base }));
        }
        let one_way = mask(TileKind::OneWay);
        for row in 0..rows.len() {
            for run in merge(&one_way[row * cols..(row + 1) * cols], cols) {
                let size = Vec2::new(run.w as f32, 1.0) * tile;
                out.push((center(run.col as f32 + run.w as f32 / 2.0, row as f32 + 0.5), Collider { shape: Collider::rect(size).shape, one_way: true, ..base }));
            }
        }
    }
    out
}

/// 改一串格子（列，行，帧号）：瓦片层标脏那一块，导航网格、地面地形只在那一格真的变了时改，碰撞体只重建改到的组。返回改了几格。
pub(crate) fn edit_map(commands: &mut Commands, map: Entity, node: &TilemapNode, parts: MapParts, edits: impl Iterator<Item = (usize, usize, i32)>) -> usize {
    let (layer, mut nav, mut ground, bodies) = parts;
    let mut dirty = BTreeSet::new();
    let mut seams = BTreeSet::new();
    let (mut changed, mut need_nav, mut need_ground) = (0, false, false);
    let band = bodies.rows;
    for (col, row, frame) in edits {
        if layer.set(col, row, frame).is_none() {
            continue;
        }
        changed += 1;
        let kind = node.checked_kind(frame);
        let cell = (col as i32, row as i32);
        // 只读不改时不标改过：寻路图看 GroundMap 改没改决定要不要重建
        match nav.as_mut() {
            Some(nav) if nav.grid.open(cell) == kind.blocks_flyers() => {
                nav.grid.set(cell, kind.blocks_flyers());
            }
            Some(_) => {}
            None => need_nav |= kind.blocks_flyers(),
        }
        match ground.as_mut() {
            Some(g) if g.terrain.at(cell) != kind.ground() => {
                g.terrain.set(cell, kind.ground());
            }
            Some(_) => {}
            None => need_ground |= kind != TileKind::Empty,
        }
        dirty.insert(row / band);
        // 这一格在条的第一行或最后一行：它和上面 / 下面那条之间的接缝可能变了
        if row % band == 0 && row > 0 {
            seams.insert(row / band - 1);
        }
        if (row + 1) % band == 0 && row / band < bodies.joined.len() {
            seams.insert(row / band);
        }
    }
    // 原来没有导航网格 / 地面地形的地图改出了实心格子：整张建一个
    if need_nav || need_ground {
        let kinds: Vec<TileKind> = layer.frames().iter().map(|&v| node.checked_kind(v)).collect();
        let mut entity = commands.entity(map);
        if let Some(nav) = nav_grid(node, &kinds).filter(|_| need_nav) {
            entity.insert(nav);
        }
        if let Some(g) = ground_map(node, &kinds).filter(|_| need_ground) {
            entity.insert(g);
        }
    }
    rebuild_groups(commands, map, node, layer.frames(), bodies, dirty, seams);
    changed
}

/// 重建改到的条所在的组；接缝连上、断开时新旧两种分组涉及的条一起重建。
fn rebuild_groups(commands: &mut Commands, map: Entity, node: &TilemapNode, data: &[i32], bodies: &mut TileBodies, dirty: BTreeSet<usize>, seams: BTreeSet<usize>) {
    let before: Vec<bool> = bodies.joined.clone();
    for k in seams {
        bodies.joined[k] = seam_joined(node, data, (k + 1) * bodies.rows);
    }
    let old = TileBodies { joined: before, ..Default::default() };
    // 新旧分组里和改到的条连着的所有条
    let mut touched = dirty;
    loop {
        let more: BTreeSet<usize> = touched.iter().flat_map(|&b| old.group(b).chain(bodies.group(b))).collect();
        if more.len() == touched.len() {
            break;
        }
        touched = more;
    }
    let mut pool = Vec::new();
    for &b in &touched {
        pool.append(&mut bodies.bands[b]);
    }
    let total = data.len() / (node.cols as usize).max(1);
    for &b in &touched {
        let group = bodies.group(b);
        if group.start == b {
            let new = band_colliders(node, data, bodies.band_rows(&group, total));
            bodies.bands[b] = reuse(commands, map, &mut pool, new);
            bodies.rebuilt += 1;
        }
    }
    for (e, ..) in pool {
        commands.entity(e).despawn();
    }
}

/// `setTiles` 命令（Update）。
pub(crate) fn set_tiles(
    mut commands: Commands,
    mut reader: MessageReader<SetTiles>,
    declared: Query<(Entity, &SceneId), With<Declared<TilemapNode>>>,
    mut maps: Query<(&Declared<TilemapNode>, &mut nge_render2d::TileLayer, Option<Mut<nge_ai::NavGrid>>, Option<Mut<nge_ai::GroundMap>>, &mut TileBodies)>,
) {
    for c in reader.read() {
        let Some((map, _)) = declared.iter().find(|m| m.1.0 == c.id) else {
            crate::lenient::warn_once(format!("setTiles：没有 id 为 {:?} 的瓦片地图", c.id));
            continue;
        };
        let Ok((node, mut layer, nav, ground, mut bodies)) = maps.get_mut(map) else {
            crate::lenient::warn_once(format!("setTiles：瓦片地图 {:?} 本身无效（cols 是 0 或 tile 不是正数），没建出来，改不了", c.id));
            continue;
        };
        let (cols, rows) = (layer.cols(), layer.rows());
        let edits: Vec<(usize, usize, i32)> = c
            .tiles
            .iter()
            .filter_map(|&[col, row, frame]| {
                let ok = (0..cols as i64).contains(&col) && (0..rows as i64).contains(&row) && i32::try_from(frame).is_ok();
                if !ok {
                    crate::lenient::warn_once(format!("setTiles：地图 {:?} 的格子 [{col}, {row}, {frame}] 在地图外或帧号太大，跳过", c.id));
                }
                ok.then_some((col as usize, row as usize, frame as i32))
            })
            .collect();
        let parts: MapParts = (&mut layer, nav, ground, &mut bodies);
        edit_map(&mut commands, map, &node.0, parts, edits.into_iter());
    }
}
