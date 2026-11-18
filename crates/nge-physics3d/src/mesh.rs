//! 按模型网格生成碰撞体：一个凸包、每个部件一个凸包、三角网格（见 docs/gameplay3d.md）。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy_ecs::prelude::*;
use glam::Vec3;
use nge_render3d::{Assets3d, Model3d, ModelId};
use rapier3d::prelude::SharedShape;

use crate::world::{vec3, vector};
use crate::{Collider, Shape};

/// 凸包顶点超过这么多时报警（照样建）。
pub const MAX_HULL_POINTS: usize = 255;
/// 三角网格三角形超过这么多时报警（照样建）。
pub const MAX_TRIANGLES: usize = 65536;

/// 从网格生成哪种形状。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshShape {
    /// 所有部件的顶点合成一个凸包。
    Convex,
    /// 每个部件（glTF 的一个图元）一个凸包。
    Hulls,
    /// 三角网格，只能用在固定的上。
    TriMesh,
}

/// 等实体有了 `Model3d` 再从模型的网格生成 [`Collider`]：形状按 `kind` 和 `scale` 生成，其余参数照 `collider`。
/// 有它、还没有 `Collider` 的实体，刚体先不建。
#[derive(Component, Debug, Clone, PartialEq)]
pub struct MeshCollider {
    pub kind: MeshShape,
    pub scale: Vec3,
    pub collider: Collider,
}

/// 模型每个部件的顶点（乘部件的 `local`、再乘 `scale`）和三角形。
fn parts(assets: &Assets3d, model: ModelId, scale: Vec3) -> Result<Vec<(Vec<Vec3>, &[u32])>, String> {
    if !scale.is_finite() || scale.abs().min_element() <= 0.0 {
        return Err(format!("缩放 {scale} 不能有 0 或非有限数"));
    }
    let model = assets.model(model).ok_or("没有这个模型")?;
    let parts: Vec<_> = model
        .parts
        .iter()
        .filter_map(|part| {
            let mesh = assets.mesh(part.mesh)?;
            let points = mesh.vertices.iter().map(|v| part.local.transform_point3(Vec3::from(v.position)) * scale).collect();
            Some((points, mesh.indices.as_slice()))
        })
        .collect();
    if parts.is_empty() {
        return Err("模型没有网格".into());
    }
    Ok(parts)
}

/// 这些点的凸包；共面（体积几乎是 0）、点太少时是 `None`。
pub(crate) fn solid_hull(points: &[Vec3]) -> Option<SharedShape> {
    let (min, max) = points.iter().fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), &p| (lo.min(p), hi.max(p)));
    let shape = SharedShape::convex_hull(&points.iter().map(|&p| vector(p)).collect::<Vec<_>>())?;
    // parry 对共面的点也给一个扁的凸包，不能用来碰撞
    (shape.mass_properties(1.0).mass() > 1e-6 * (max - min).length().powi(3)).then_some(shape)
}

/// 这些点的凸包只留外壳上的顶点；共面、点太少时是 `None`。
fn hull(points: &[Vec3]) -> Option<Vec<Vec3>> {
    Some(solid_hull(points)?.as_convex_polyhedron()?.points().iter().map(|&p| vec3(p)).collect())
}

/// 从模型的网格生成形状。没有模型或网格、缩放有 0、凸包生不出来（共面）时返回错误；
/// 每个部件一个凸包时跳过平的部件（各 warn 一次），全都生不出来才算错。
pub fn mesh_shape(assets: &Assets3d, model: ModelId, kind: MeshShape, scale: Vec3) -> Result<Shape, String> {
    let parts = parts(assets, model, scale)?;
    let flat = "点都在一个平面上，生不出凸包";
    match kind {
        MeshShape::Convex => {
            let all: Vec<Vec3> = parts.iter().flat_map(|(p, _)| p.iter().copied()).collect();
            Ok(Shape::Convex(Arc::new(hull(&all).ok_or(flat)?)))
        }
        MeshShape::Hulls => {
            let hulls: Vec<_> = parts
                .iter()
                .enumerate()
                .filter_map(|(i, (p, _))| {
                    let h = hull(p);
                    if h.is_none() {
                        log::warn!("模型的第 {i} 个部件{flat}，跳过");
                    }
                    h
                })
                .collect();
            if hulls.is_empty() {
                return Err(flat.into());
            }
            Ok(Shape::Hulls(Arc::new(hulls)))
        }
        MeshShape::TriMesh => trimesh(&parts, scale),
    }
}

/// 所有部件合成一个三角网格：位置差不多的顶点合并，退化的三角形丢掉；缩放镜像了（奇数个负轴）就把三角形翻回来。
fn trimesh(parts: &[(Vec<Vec3>, &[u32])], scale: Vec3) -> Result<Shape, String> {
    let (min, max) = parts.iter().flat_map(|(p, _)| p.iter()).fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), &p| (lo.min(p), hi.max(p)));
    // 合并的容差跟着模型大小走；-0 和 0 量化以后一样
    let tolerance = ((max - min).length() * 1e-6).max(f32::MIN_POSITIVE);
    let key = |p: Vec3| (p / tolerance).round().to_array().map(|v| v as i64);
    let mirrored = scale.x * scale.y * scale.z < 0.0;
    let (mut points, mut triangles, mut seen) = (Vec::new(), Vec::new(), HashMap::new());
    for (part, indices) in parts {
        if let Some(&bad) = indices.iter().find(|&&i| i as usize >= part.len()) {
            return Err(format!("三角形的下标 {bad} 超出顶点数 {}", part.len()));
        }
        let ids: Vec<u32> = part
            .iter()
            .map(|&p| {
                *seen.entry(key(p)).or_insert_with(|| {
                    points.push(p);
                    points.len() as u32 - 1
                })
            })
            .collect();
        for t in indices.chunks_exact(3) {
            let [a, b, c] = [0, 1, 2].map(|i| ids[t[i] as usize]);
            if a != b && b != c && a != c {
                triangles.push(if mirrored { [a, c, b] } else { [a, b, c] });
            }
        }
    }
    if triangles.is_empty() {
        return Err("没有三角形".into());
    }
    Ok(Shape::TriMesh(Arc::new((points, triangles))))
}

/// 形状太大（凸包顶点超过 [`MAX_HULL_POINTS`]、三角形超过 [`MAX_TRIANGLES`]）时说明原因。
pub fn too_big(shape: &Shape) -> Option<String> {
    let hull = |p: &Vec<Vec3>| (p.len() > MAX_HULL_POINTS).then(|| format!("凸包有 {} 个顶点，超过 {MAX_HULL_POINTS}", p.len()));
    match shape {
        Shape::Convex(p) => hull(p),
        Shape::Hulls(parts) => parts.iter().find_map(hull),
        Shape::TriMesh(mesh) => (mesh.1.len() > MAX_TRIANGLES).then(|| format!("三角网格有 {} 个三角形，超过 {MAX_TRIANGLES}", mesh.1.len())),
        _ => None,
    }
}

/// 等模型等了这么多秒还没生成碰撞体时 warn 一次。
pub(crate) const LONG_WAIT: f32 = 5.0;

type Waiting = (With<MeshCollider>, Without<Collider>);

/// 模型或 `MeshCollider` 变了：重新生成形状，插上 `Collider`。记下还在等的（[`crate::Physics3d::pending_meshes`]），等太久的 warn 一次。
pub(crate) fn build(
    assets: Option<Res<Assets3d>>,
    time: Option<Res<nge_ecs::Time>>,
    mut physics: ResMut<crate::Physics3d>,
    changed: Query<(Entity, &MeshCollider, &Model3d), Or<(Changed<MeshCollider>, Changed<Model3d>)>>,
    waiting: Query<Entity, Waiting>,
    mut commands: Commands,
) {
    let physics = &mut *physics;
    let mut done = HashSet::new();
    if let Some(assets) = assets {
        for (entity, mc, model) in &changed {
            match mesh_shape(&assets, model.0, mc.kind, mc.scale) {
                Ok(shape) => {
                    if let Some(why) = too_big(&shape) {
                        log::warn!("实体 {entity:?} 的网格碰撞体{why}，模拟会变慢");
                    }
                    commands.entity(entity).insert(Collider { shape, ..mc.collider.clone() });
                    physics.mesh_failed.remove(&entity);
                    done.insert(entity);
                }
                Err(e) => {
                    log::warn!("实体 {entity:?} 的网格碰撞体生不出来：{e}");
                    physics.mesh_failed.insert(entity);
                }
            }
        }
    }
    // 还在等的：没有碰撞体、这一帧也没生成、也不是生不出来的
    let dt = time.map_or(0.0, |t| if t.delta.is_finite() { t.delta.max(0.0) } else { 0.0 });
    let old = std::mem::take(&mut physics.waiting);
    for entity in &waiting {
        if done.contains(&entity) || physics.mesh_failed.contains(&entity) {
            continue;
        }
        let waited = old.get(&entity).copied().unwrap_or(0.0) + dt;
        if waited >= LONG_WAIT && physics.warnings.insert((entity, "mesh")) {
            log::warn!("实体 {entity:?} 的网格碰撞体等模型等了 {LONG_WAIT} 秒还没好（模型没加载完或没有 Model3d），刚体一直没建");
        }
        physics.waiting.insert(entity, waited);
    }
    physics.mesh_failed.retain(|e| waiting.contains(*e));
}
