//! 物理调试视图：开着时把碰撞体轮廓、接触点、关节、这一帧的射线和形状投射画进 `Gizmos3d`（见 docs/gameplay3d.md）。

use bevy_ecs::prelude::*;
use glam::{Quat, Vec3, Vec4};
use nge_render3d::{Gizmos3d, Lines};
use rapier3d::math::Pose;
use rapier3d::parry::shape::Shape as ParryShape;

use crate::world::{Trace, quat, vec3, vector};
use crate::{Physics3d, QueryShape};

/// 开关物理调试视图，默认关。`on_top` 为真时全部画在模型前面。
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Physics3dDebug {
    pub enabled: bool,
    pub on_top: bool,
}

// 碰撞体的颜色和 2D 一样
pub(crate) const FIXED: Vec4 = Vec4::new(0.6, 0.6, 0.6, 1.0);
pub(crate) const DYNAMIC: Vec4 = Vec4::new(0.3, 1.0, 0.4, 1.0);
pub(crate) const KINEMATIC: Vec4 = Vec4::new(0.3, 0.6, 1.0, 1.0);
pub(crate) const SENSOR: Vec4 = Vec4::new(1.0, 0.85, 0.2, 0.6);
pub(crate) const JOINT: Vec4 = Vec4::new(1.0, 0.4, 0.9, 1.0);
pub(crate) const CONTACT: Vec4 = Vec4::new(1.0, 0.55, 0.1, 1.0);
pub(crate) const HIT: Vec4 = Vec4::new(1.0, 0.2, 0.2, 1.0);
pub(crate) const MISS: Vec4 = Vec4::new(0.5, 0.1, 0.1, 1.0);
pub(crate) const NORMAL: Vec4 = Vec4::new(0.2, 1.0, 0.3, 1.0);

/// 物理一步之后画。开关跟着 `Physics3dDebug` 设 `Physics3d` 记不记查询；没有 `Gizmos3d` 时什么也不画、不记。
pub(crate) fn draw(debug: Option<Res<Physics3dDebug>>, mut physics: ResMut<Physics3d>, gizmos: Option<ResMut<Gizmos3d>>) {
    let on = debug.as_ref().is_some_and(|d| d.enabled) && gizmos.is_some();
    // 只改开关，不算物理变了
    let physics = physics.bypass_change_detection();
    physics.tracing = on;
    let traces = std::mem::take(&mut *physics.traces.lock().unwrap_or_else(|e| e.into_inner()));
    let (Some(debug), Some(mut gizmos)) = (debug, gizmos) else { return };
    if !on {
        return;
    }
    let lines: &mut Lines = if debug.on_top { gizmos.on_top() } else { &mut gizmos };
    // 一米多少单位：接触点的十字、法线、坐标轴按它定大小
    let unit = physics.world.integration_parameters.length_unit;
    outlines(physics, lines, unit);
    contacts(physics, lines, unit);
    joints(physics, lines);
    for trace in traces {
        draw_trace(lines, trace, unit);
    }
}

fn outlines(physics: &Physics3d, lines: &mut Lines, unit: f32) {
    for (_, collider) in physics.world.colliders.iter() {
        let body = collider.parent().and_then(|b| physics.world.bodies.get(b));
        let color = match body {
            _ if collider.is_sensor() => SENSOR,
            Some(b) if b.is_dynamic() => DYNAMIC,
            Some(b) if b.is_kinematic() => KINEMATIC,
            _ => FIXED,
        };
        shape(lines, collider.shape(), collider.position(), color);
    }
    // 角色额外画坐标轴
    for handles in physics.entities.values().filter(|h| h.mover.is_some()) {
        if let Some(c) = handles.collider.and_then(|c| physics.world.colliders.get(c)) {
            lines.axes(vec3(c.position().translation), quat(c.position().rotation), 0.3 * unit);
        }
    }
}

/// 一个 rapier 形状的轮廓。
fn shape(lines: &mut Lines, shape: &dyn ParryShape, at: &Pose, color: Vec4) {
    let (center, rotation) = (vec3(at.translation), quat(at.rotation));
    let point = |p| vec3(at.transform_point(p));
    if let Some(ball) = shape.as_ball() {
        lines.sphere(center, ball.radius, color);
        // 一条半径看得出转了多少
        lines.line(center, center + rotation * Vec3::X * ball.radius, color);
    } else if let Some(cuboid) = shape.as_cuboid() {
        lines.cuboid(center, rotation, vec3(cuboid.half_extents) * 2.0, color);
    } else if let Some(capsule) = shape.as_capsule() {
        let (a, b) = (point(capsule.segment.a), point(capsule.segment.b));
        let axis = (b - a).try_normalize().unwrap_or(rotation * Vec3::Y);
        lines.capsule((a + b) / 2.0, Quat::from_rotation_arc(Vec3::Y, axis), a.distance(b) / 2.0, capsule.radius, color);
    } else if let Some(hull) = shape.as_convex_polyhedron() {
        let p = hull.points();
        for edge in hull.edges() {
            lines.line(point(p[edge.vertices[0] as usize]), point(p[edge.vertices[1] as usize]), color);
        }
    } else if let Some(mesh) = shape.as_trimesh() {
        let p = mesh.vertices();
        for t in mesh.indices() {
            for i in 0..3 {
                lines.line(point(p[t[i] as usize]), point(p[t[(i + 1) % 3] as usize]), color);
            }
        }
    } else if let Some(compound) = shape.as_compound() {
        for (local, part) in compound.shapes() {
            self::shape(lines, &**part, &(*at * *local), color);
        }
    }
}

/// 接触点：小十字加法线。角色的是走完这一帧贴着的（rapier 不算 kinematic 的接触），点取碰撞体朝对方最远的那个点。
fn contacts(physics: &Physics3d, lines: &mut Lines, unit: f32) {
    let mut mark = |p: Vec3, n: Vec3| {
        let s = 0.03 * unit;
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            lines.line(p - axis * s, p + axis * s, CONTACT);
        }
        lines.line(p, p + n * 0.15 * unit, CONTACT);
    };
    for pair in physics.world.contact_pairs().filter(|p| p.has_any_active_contact()) {
        let Some(c1) = physics.world.colliders.get(pair.collider1) else { continue };
        for manifold in pair.manifolds() {
            let normal = vec3(manifold.data.normal);
            for point in manifold.points.iter().filter(|p| p.dist <= 0.01 * unit) {
                mark(vec3(c1.position().transform_point(point.local_p1)), normal);
            }
        }
    }
    for handles in physics.entities.values() {
        let (Some(mover), Some(c)) = (&handles.mover, handles.collider.and_then(|c| physics.world.colliders.get(c))) else { continue };
        let Some(support) = c.shape().as_support_map() else { continue };
        for contact in &mover.touching {
            let local = c.position().rotation.inverse() * vector(-contact.normal);
            mark(vec3(c.position().transform_point(support.local_support_point(local))), contact.normal);
        }
    }
}

/// 连上了的关节：两个锚点之间一条线。
fn joints(physics: &Physics3d, lines: &mut Lines) {
    for (_, joint) in physics.world.impulse_joints() {
        let anchor = |body, frame: &Pose| physics.world.bodies.get(body).map(|b| vec3(b.position().transform_point(frame.translation)));
        if let (Some(a), Some(b)) = (anchor(joint.body1(), &joint.data.local_frame1), anchor(joint.body2(), &joint.data.local_frame2)) {
            lines.line(a, b, JOINT);
        }
    }
}

/// 射线、形状投射：碰到了是红线到碰撞点加绿的法线，没碰到暗红；形状投射在终点画形状。
fn draw_trace(lines: &mut Lines, trace: Trace, unit: f32) {
    let color = if trace.hit.is_some() { HIT } else { MISS };
    match trace.hit {
        Some((point, normal)) if trace.shape.is_none() => {
            lines.line(trace.from, point, HIT);
            lines.line(point, point + normal * 0.15 * unit, NORMAL);
        }
        Some((point, normal)) => {
            lines.line(trace.from, trace.to, HIT);
            lines.line(point, point + normal * 0.15 * unit, NORMAL);
        }
        None => lines.line(trace.from, trace.to, MISS),
    }
    if let Some((shape, rotation)) = trace.shape {
        match shape {
            QueryShape::Ball(r) => lines.sphere(trace.to, r, color),
            QueryShape::Capsule { half_height, radius } => lines.capsule(trace.to, rotation, half_height, radius, color),
            QueryShape::Box(size) => lines.cuboid(trace.to, rotation, size, color),
        }
    }
}
