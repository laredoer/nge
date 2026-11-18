//! 空间查询：形状投射、重叠、最近点（见 docs/gameplay3d.md）。射线在 world.rs。都查上一步结束时的世界。

use bevy_ecs::prelude::*;
use glam::{Quat, Vec3};
use rapier3d::math::Pose;
use rapier3d::parry::query::{ShapeCastOptions, contact};
use rapier3d::parry::shape::{Ball, Capsule, Cuboid, Shape as ParryShape};

use crate::world::{Trace, pose, vec3, vector};
use crate::{Physics3d, PointHit, QueryFilter, QueryShape, ShapeHit};

impl QueryShape {
    /// 换成 parry 的形状；大小不是正的有限数时是 `None`。
    pub(crate) fn parry(self) -> Option<Box<dyn ParryShape>> {
        let ok = |v: f32| v.is_finite() && v > 0.0;
        match self {
            QueryShape::Ball(r) if ok(r) => Some(Box::new(Ball::new(r))),
            // 中间段可以是 0（就是个球）
            QueryShape::Capsule { half_height, radius } if ok(radius) && half_height.is_finite() && half_height >= 0.0 => Some(Box::new(Capsule::new_y(half_height, radius))),
            QueryShape::Box(size) if size.is_finite() && size.min_element() > 0.0 => Some(Box::new(Cuboid::new(vector(size / 2.0)))),
            _ => None,
        }
    }
}

/// 位置和朝向能用：都是有限数，四元数不是 0。
fn placed(at: Vec3, rotation: Quat) -> Option<Pose> {
    (at.is_finite() && rotation.is_finite() && rotation.length_squared() > 1e-12).then(|| pose((at, rotation)))
}

impl Physics3d {
    /// 把 `shape` 放在 `origin`（朝向 `rotation`），沿 `direction`（会归一化）最远扫 `max_distance`，第一个碰到的碰撞体。
    /// 一开始就重叠时 `distance` 是 0、`normal` 是 `-direction`。坏输入是 `None`。
    pub fn cast_shape(&self, shape: QueryShape, origin: Vec3, rotation: Quat, direction: Vec3, max_distance: f32, filter: QueryFilter) -> Option<ShapeHit> {
        let dir = direction.try_normalize()?;
        let (parry, at) = (shape.parry()?, placed(origin, rotation)?);
        if max_distance.is_nan() || max_distance <= 0.0 {
            return None;
        }
        // 一开始就重叠的：parry 从里面扫时有时报、有时不报（看往哪个方向扫），先单独查。压得最深的那个算碰到，
        // 法线迎着方向，碰撞点是起点
        let rapier_filter = self.filter(filter);
        let inside = self
            .world
            .intersect_shape(at, &*parry, rapier_filter)
            .map(|(handle, c)| (contact(&at, &*parry, c.position(), c.shape(), 0.0).ok().flatten().map_or(0.0, |c| c.dist), handle))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .and_then(|(_, handle)| self.entity_of(handle))
            .map(|entity| ShapeHit { entity, point: origin, normal: -dir, distance: 0.0 });
        let hit = inside.or_else(|| {
            let options = ShapeCastOptions { max_time_of_impact: max_distance, stop_at_penetration: true, ..Default::default() };
            let (collider, hit) = self.world.cast_shape(&at, vector(dir), &*parry, options, self.filter(filter))?;
            let normal = vec3(hit.normal1).try_normalize().unwrap_or(-dir);
            Some(ShapeHit { entity: self.entity_of(collider)?, point: vec3(hit.witness1), normal, distance: hit.time_of_impact })
        });
        let end = origin + dir * hit.map_or(max_distance, |h| h.distance);
        self.trace(Trace { from: origin, to: end, shape: Some((shape, rotation)), hit: hit.map(|h| (h.point, h.normal)) });
        hit
    }

    /// 和 `shape`（放在 `at`，朝向 `rotation`）重叠的实体，按实体排好。坏输入是空的。
    pub fn overlap(&self, shape: QueryShape, at: Vec3, rotation: Quat, filter: QueryFilter) -> Vec<Entity> {
        let (Some(parry), Some(at)) = (shape.parry(), placed(at, rotation)) else { return Vec::new() };
        let mut found: Vec<Entity> = self.world.intersect_shape(at, &*parry, self.filter(filter)).filter_map(|(c, _)| self.entity_of(c)).collect();
        found.sort();
        found.dedup();
        found
    }

    /// 包含这个点的实体，按实体排好。
    pub fn overlap_point(&self, point: Vec3, filter: QueryFilter) -> Vec<Entity> {
        if !point.is_finite() {
            return Vec::new();
        }
        let mut found: Vec<Entity> = self.world.intersect_point(vector(point), self.filter(filter)).filter_map(|(c, _)| self.entity_of(c)).collect();
        found.sort();
        found.dedup();
        found
    }

    /// `max_distance` 以内最近的碰撞体表面上的点；点在实心碰撞体里面时 `inside` 为真、`point` 是点自己、`distance` 是 0。
    pub fn closest_point(&self, point: Vec3, max_distance: f32, filter: QueryFilter) -> Option<PointHit> {
        if !point.is_finite() || max_distance.is_nan() || max_distance < 0.0 {
            return None;
        }
        let (collider, projection) = self.world.project_point(vector(point), max_distance, true, self.filter(filter))?;
        let found = vec3(projection.point);
        let distance = if projection.is_inside { 0.0 } else { found.distance(point) };
        Some(PointHit { entity: self.entity_of(collider)?, point: found, distance, inside: projection.is_inside })
    }
}
