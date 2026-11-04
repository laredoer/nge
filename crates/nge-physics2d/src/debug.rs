//! 物理调试视图：开着时把每个碰撞体的轮廓画出来（见 docs/gizmos.md）。

use bevy_ecs::prelude::*;
use glam::{Vec2, Vec4};
use nge_render2d::{DrawAt, Gizmos, SpriteInstances};

use crate::{Collider, Physics, RigidBody, Shape};

/// 开关物理调试视图，默认关。
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PhysicsDebug {
    pub enabled: bool,
}

pub(crate) const WIDTH: f32 = 1.5;
pub(crate) const FIXED: Vec4 = Vec4::new(0.6, 0.6, 0.6, 1.0);
pub(crate) const DYNAMIC: Vec4 = Vec4::new(0.3, 1.0, 0.4, 1.0);
pub(crate) const KINEMATIC: Vec4 = Vec4::new(0.3, 0.6, 1.0, 1.0);
pub(crate) const SENSOR: Vec4 = Vec4::new(1.0, 0.85, 0.2, 0.6);

pub(crate) fn enabled(debug: Res<PhysicsDebug>) -> bool {
    debug.enabled
}

/// 物理一步之后画；有 `DrawAt` 的画在插值过的位置上，和精灵贴着。没有 2D 渲染时什么也不做。
pub(crate) fn draw(physics: Res<Physics>, drawn: Query<Option<&DrawAt>, With<Collider>>, instances: Option<ResMut<SpriteInstances>>) {
    let Some(instances) = instances else { return };
    let mut gizmos = Gizmos::from(instances);
    // 按实体排、sensor 最后画：增删实体时重叠处谁盖住谁不跟着变
    let mut outlines: Vec<_> = physics.outlines().collect();
    outlines.sort_by_key(|&(entity, _, _, sensor, _)| (sensor, entity));
    for (entity, shape, kind, sensor, (center, rotation)) in outlines {
        // 这一帧刚删掉的（物理下一帧才移除）不画
        let Ok(at) = drawn.get(entity) else { continue };
        let (center, rotation) = at.map_or((center, rotation), |d| (d.translation, d.rotation));
        let color = match (sensor, kind) {
            (true, _) => SENSOR,
            (_, Some(RigidBody::Dynamic)) => DYNAMIC,
            (_, Some(RigidBody::Kinematic)) => KINEMATIC,
            _ => FIXED,
        };
        match shape {
            Shape::Rect(size) => gizmos.rect_outline(center, size, rotation, WIDTH, color),
            Shape::Circle(r) => {
                gizmos.circle(center, r.abs(), WIDTH, color);
                // 一条半径看得出转了多少
                gizmos.line(center, center + Vec2::from_angle(rotation) * r.abs(), WIDTH, color);
            }
        }
    }
}
