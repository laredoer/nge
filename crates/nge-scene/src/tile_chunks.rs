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
use crate::tilemap::{MapParts, Run, TileKind, TileSolid, TileTap, TilemapNode, ground_map, merge, nav_grid};

/// 改地图上的格子：`tiles` 是一串 `[列, 行, 帧号]`，帧号 -1 是挖空。改到下一次声明里这一格变了、或者整张重建为止。
pub const SET_TILES: CommandKey<SetTiles> = CommandKey::new("setTiles");

#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
pub struct SetTiles {
    pub id: String,
    /// 写错的那一项跳过。
    #[serde(default, deserialize_with = "crate::lenient::set_tiles")]
    pub tiles: Vec<[i64; 3]>,
}

/// 组件：地图的碰撞体，一条一组。
#[derive(Component, Debug, Default)]
pub(crate) struct TileBodies {
    bands: Vec<Vec<Entity>>,
    /// 一条几行。
    rows: usize,
    /// 一共重建过几条（整张建的不算）。
    rebuilt: u64,
}

impl TileBodies {
    /// 整张建：每条的碰撞体。
    pub(crate) fn build(commands: &mut Commands, map: Entity, node: &TilemapNode, data: &[i32], rows: usize) -> Self {
        let cols = node.cols as usize;
        let total = data.len() / cols.max(1);
        let bands = (0..total.div_ceil(rows)).map(|b| spawn_band(commands, map, node, data, b * rows..((b + 1) * rows).min(total))).collect();
        Self { bands, rows, rebuilt: 0 }
    }

    #[cfg(test)]
    pub(crate) fn rebuilt(&self) -> u64 {
        self.rebuilt
    }
}

fn spawn_band(commands: &mut Commands, map: Entity, node: &TilemapNode, data: &[i32], rows: std::ops::Range<usize>) -> Vec<Entity> {
    band_colliders(node, data, rows).into_iter().map(|(at, collider)| commands.spawn((collider, Transform2d::from_xy(at.x, at.y), TileSolid, ChildOf(map))).id()).collect()
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

/// 改一串格子（列，行，帧号）：瓦片层标脏那一块，导航网格、地面地形、点地图原地改，碰撞体只重建改到的条。返回改了几格。
pub(crate) fn edit_map(commands: &mut Commands, map: Entity, node: &TilemapNode, parts: MapParts, edits: impl Iterator<Item = (usize, usize, i32)>) -> usize {
    let (layer, mut nav, mut ground, mut tap, bodies) = parts;
    let mut dirty = BTreeSet::new();
    let (mut changed, mut need_nav, mut need_ground) = (0, false, false);
    for (col, row, frame) in edits {
        if layer.set(col, row, frame).is_none() {
            continue;
        }
        changed += 1;
        let kind = node.checked_kind(frame);
        let cell = (col as i32, row as i32);
        match nav.as_deref_mut() {
            Some(nav) => {
                nav.grid.set(cell, kind.blocks_flyers());
            }
            None => need_nav |= kind.blocks_flyers(),
        }
        match ground.as_deref_mut() {
            Some(ground) => {
                ground.terrain.set(cell, kind.ground());
            }
            None => need_ground |= kind != TileKind::Empty,
        }
        if let Some(tap) = tap.as_deref_mut() {
            tap.data[row * tap.cols + col] = frame;
        }
        dirty.insert(row / bodies.rows.max(1));
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
    let total = layer.rows();
    for b in dirty {
        for e in std::mem::take(&mut bodies.bands[b]) {
            commands.entity(e).despawn();
        }
        let rows = b * bodies.rows..((b + 1) * bodies.rows).min(total);
        bodies.bands[b] = spawn_band(commands, map, node, layer.frames(), rows);
        bodies.rebuilt += 1;
    }
    changed
}

/// `setTiles` 命令（Update）。
pub(crate) fn set_tiles(
    mut commands: Commands,
    mut reader: MessageReader<SetTiles>,
    mut maps: Query<(Entity, &SceneId, &Declared<TilemapNode>, &mut nge_render2d::TileLayer, Option<&mut nge_ai::NavGrid>, Option<&mut nge_ai::GroundMap>, Option<&mut TileTap>, &mut TileBodies)>,
) {
    for c in reader.read() {
        let Some((map, _, node, mut layer, mut nav, mut ground, mut tap, mut bodies)) = maps.iter_mut().find(|m| m.1.0 == c.id) else {
            crate::lenient::warn_once(format!("setTiles：没有 id 为 {:?} 的瓦片地图", c.id));
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
        let parts: MapParts = (&mut layer, nav.as_deref_mut(), ground.as_deref_mut(), tap.as_deref_mut(), &mut bodies);
        edit_map(&mut commands, map, &node.0, parts, edits.into_iter());
    }
}
