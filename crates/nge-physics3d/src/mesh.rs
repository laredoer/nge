//! 按模型网格生成碰撞体：一个凸包、每个部件一个凸包、三角网格（见 docs/gameplay3d.md）。

use std::collections::HashMap;
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

/// 这些点的凸包只留外壳上的顶点；共面（体积几乎是 0）、点太少时是 `None`。
fn hull(points: &[Vec3]) -> Option<Vec<Vec3>> {
    let (min, max) = points.iter().fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), &p| (lo.min(p), hi.max(p)));
    let points: Vec<_> = points.iter().map(|&p| vector(p)).collect();
    let shape = SharedShape::convex_hull(&points)?;
    // parry 对共面的点也给一个扁的凸包，不能用来碰撞
    if shape.mass_properties(1.0).mass() <= 1e-6 * (max - min).length().powi(3) {
        return None;
    }
    Some(shape.as_convex_polyhedron()?.points().iter().map(|&p| vec3(p)).collect())
}

/// 从模型的网格生成形状（纯函数）。没有模型或网格、缩放有 0、凸包生不出来（共面）时返回错误。
pub fn mesh_shape(assets: &Assets3d, model: ModelId, kind: MeshShape, scale: Vec3) -> Result<Shape, String> {
    let parts = parts(assets, model, scale)?;
    let flat = "点都在一个平面上，生不出凸包";
    match kind {
        MeshShape::Convex => {
            let all: Vec<Vec3> = parts.iter().flat_map(|(p, _)| p.iter().copied()).collect();
            Ok(Shape::Convex(Arc::new(hull(&all).ok_or(flat)?)))
        }
        MeshShape::Hulls => {
            let hulls: Option<Vec<_>> = parts.iter().map(|(p, _)| hull(p)).collect();
            Ok(Shape::Hulls(Arc::new(hulls.ok_or(flat)?)))
        }
        MeshShape::TriMesh => {
            // glTF 每个面各有一份顶点：位置一样的合并，退化的三角形丢掉
            let (mut points, mut triangles, mut seen) = (Vec::new(), Vec::new(), HashMap::new());
            for (part, indices) in &parts {
                let ids: Vec<u32> = part
                    .iter()
                    .map(|p| {
                        *seen.entry(p.to_array().map(f32::to_bits)).or_insert_with(|| {
                            points.push(*p);
                            points.len() as u32 - 1
                        })
                    })
                    .collect();
                for t in indices.chunks_exact(3) {
                    let [a, b, c] = [0, 1, 2].map(|i| ids[t[i] as usize]);
                    if a != b && b != c && a != c {
                        triangles.push([a, b, c]);
                    }
                }
            }
            if triangles.is_empty() {
                return Err("没有三角形".into());
            }
            Ok(Shape::TriMesh(Arc::new((points, triangles))))
        }
    }
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

/// 模型或 `MeshCollider` 变了：重新生成形状，插上 `Collider`。
pub(crate) fn build(assets: Option<Res<Assets3d>>, query: Query<(Entity, &MeshCollider, &Model3d), Or<(Changed<MeshCollider>, Changed<Model3d>)>>, mut commands: Commands) {
    let Some(assets) = assets else { return };
    for (entity, mc, model) in &query {
        match mesh_shape(&assets, model.0, mc.kind, mc.scale) {
            Ok(shape) => {
                if let Some(why) = too_big(&shape) {
                    log::warn!("实体 {entity:?} 的网格碰撞体{why}，模拟会变慢");
                }
                commands.entity(entity).insert(Collider { shape, ..mc.collider.clone() });
            }
            Err(e) => log::warn!("实体 {entity:?} 的网格碰撞体生不出来：{e}"),
        }
    }
}
