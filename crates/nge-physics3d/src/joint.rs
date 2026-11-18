//! 3D 关节：把两个刚体（或一个刚体和世界）连起来，用 rapier 的 impulse joint（见 docs/gameplay3d.md）。

use bevy_ecs::prelude::*;
use glam::{Quat, Vec3};
use rapier3d::math::Pose;
use rapier3d::prelude::{
    FixedJointBuilder, GenericJoint, ImpulseJointHandle, MotorModel, PrismaticJointBuilder, RevoluteJointBuilder, RigidBodyBuilder, RigidBodyHandle, RopeJointBuilder,
    SphericalJointBuilder, SpringJointBuilder,
};

use crate::Physics3d;
use crate::world::{rotation, vector};

/// 马达跟速度跟得多紧（rapier 的 damping，按加速度算），和 2D 一样。
const MOTOR_GAIN: f32 = 1000.0;

/// 关节，挂在单独的实体上。`a` 是 `None` 时连到世界上（`anchor_a` 是世界坐标）。
/// 两头都要是建到物理里的刚体，还不是的时候先不连，成了刚体再连上；刚体重建（换了种类或形状）后自动重连。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Joint3d {
    pub a: Option<Entity>,
    pub b: Entity,
    pub kind: Joint3dKind,
    /// 在 a 自己的坐标里（没转过、原点在 a 的中心）。
    pub anchor_a: Vec3,
    /// 在 b 自己的坐标里。
    pub anchor_b: Vec3,
    /// b 相对 a 的朝向：焊死、滑轨保持它，铰链的 `limits` 从它算起。
    pub rotation: Quat,
    /// 连着的两个刚体之间还碰撞。默认不碰。
    pub collide: bool,
}

impl Joint3d {
    pub fn new(a: Option<Entity>, b: Entity, kind: Joint3dKind) -> Self {
        Self { a, b, kind, anchor_a: Vec3::ZERO, anchor_b: Vec3::ZERO, rotation: Quat::IDENTITY, collide: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Joint3dKind {
    /// 焊死：位置和朝向都不变。
    Fixed,
    /// 球窝：绕锚点随便转。
    Ball,
    /// 铰链：绕 `axis`（a 的坐标里，会归一化）转。`limits` 是相对角度的范围（弧度），`motor` 的速度是弧度 / 秒。
    Revolute { axis: Vec3, limits: Option<[f32; 2]>, motor: Option<Motor3d> },
    /// 滑轨：沿 `axis`（a 的坐标里，会归一化）滑，不转。`limits` 是沿轴的距离范围，`motor` 的速度是单位 / 秒。
    Prismatic { axis: Vec3, limits: Option<[f32; 2]>, motor: Option<Motor3d> },
    /// 绳子：两个锚点最远 `length`，可以靠近。
    Rope { length: f32 },
    /// 弹簧：拉向 `length`。`stiffness`、`damping` 按加速度算，和质量无关。
    Spring { length: f32, stiffness: f32, damping: f32 },
}

/// 马达：把相对速度拉到 `speed`，最多用 `max_force` 的力（`f32::INFINITY` 不限）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motor3d {
    pub speed: f32,
    pub max_force: f32,
}

/// 处理过的关节和它的声明：`handle` 是 `None` 时参数不对、没连（声明不变就不再试，也不再 warn）。
pub(crate) struct Linked {
    pub handle: Option<ImpulseJointHandle>,
    pub joint: Joint3d,
}

/// 每帧对一遍：没了的拆掉，新的、变了的重建；刚体删掉或重建时 rapier 顺带拆了关节，这里也重建。
pub(crate) fn sync(mut physics: ResMut<Physics3d>, joints: Query<(Entity, &Joint3d)>) {
    let physics = &mut *physics;
    physics.joints.retain(|entity, linked| {
        let keep = joints.contains(*entity);
        if !keep && let Some(handle) = linked.handle {
            physics.world.remove_impulse_joint(handle);
        }
        keep
    });
    for (entity, joint) in &joints {
        let alive = |l: &Linked| l.handle.is_none_or(|h| physics.world.impulse_joints.get(h).is_some());
        let old = physics.joints.get(&entity).filter(|l| alive(l)).map(|l| (l.handle, l.joint));
        if old.is_some_and(|(_, j)| j == *joint) {
            continue;
        }
        let Some(data) = generic(joint) else {
            log::warn!("3D 关节 {entity:?} 的参数不对（不是有限的数、长度不是正数、范围反了或超过一圈、轴或朝向是 0，或者两头是同一个刚体），不连");
            if let Some(handle) = physics.joints.remove(&entity).and_then(|l| l.handle) {
                physics.world.remove_impulse_joint(handle);
            }
            physics.joints.insert(entity, Linked { handle: None, joint: *joint });
            continue;
        };
        // 两头没换、关节还在：原地换参数（比如每帧改马达速度），不丢求解器攒下的冲量
        if let Some((Some(handle), before)) = old
            && (before.a, before.b) == (joint.a, joint.b)
            && let Some(existing) = physics.world.impulse_joints.get_mut(handle, true)
        {
            existing.data = data;
            physics.joints.insert(entity, Linked { handle: Some(handle), joint: *joint });
            continue;
        }
        if let Some(handle) = physics.joints.remove(&entity).and_then(|l| l.handle) {
            // 刚体删掉时 rapier 已经顺带拆了，句柄可能早就没了
            physics.world.remove_impulse_joint(handle);
        }
        let Some((a, b)) = bodies(physics, joint) else { continue };
        let handle = physics.world.insert_impulse_joint(a, b, data);
        physics.joints.insert(entity, Linked { handle: Some(handle), joint: *joint });
    }
}

/// 两头的刚体句柄；`a` 是世界时用一个固定在原点的刚体。还没建到物理里时是 `None`。
fn bodies(physics: &mut Physics3d, joint: &Joint3d) -> Option<(RigidBodyHandle, RigidBodyHandle)> {
    let body = |physics: &Physics3d, e: Entity| physics.entities.get(&e).and_then(|h| h.body);
    let b = body(physics, joint.b)?;
    let a = match joint.a {
        Some(a) => body(physics, a)?,
        None => *physics.ground.get_or_insert_with(|| physics.world.insert_body(RigidBodyBuilder::fixed())),
    };
    Some((a, b))
}

fn generic(joint: &Joint3d) -> Option<GenericJoint> {
    let finite = joint.anchor_a.is_finite() && joint.anchor_b.is_finite() && joint.rotation.is_finite();
    if !finite || joint.rotation.length_squared() < 1e-12 || joint.a == Some(joint.b) {
        return None;
    }
    let motor_ok = |m: &Option<Motor3d>| m.is_none_or(|m| m.speed.is_finite() && !m.max_force.is_nan() && m.max_force >= 0.0);
    let limits_ok = |l: &Option<[f32; 2]>| l.is_none_or(|[lo, hi]| lo.is_finite() && hi.is_finite() && lo <= hi);
    // rapier 里铰链的范围超过一整圈就等于不限
    let turn_ok = |l: &Option<[f32; 2]>| l.is_none_or(|[lo, hi]| hi - lo <= std::f32::consts::TAU);
    // 铰链转锚点坐标系的 x 轴、滑轨沿它滑：frame1 的 x 轴转到 a 坐标里的 axis，frame2 再乘上 b 相对 a 的朝向的逆（两个 frame 的朝向一致）
    let (mut data, turn): (GenericJoint, Quat) = match joint.kind {
        JointKind3::Fixed => (FixedJointBuilder::new().into(), Quat::IDENTITY),
        JointKind3::Ball => (SphericalJointBuilder::new().into(), Quat::IDENTITY),
        JointKind3::Revolute { axis, limits, motor } => {
            let axis = axis.try_normalize()?;
            if !limits_ok(&limits) || !turn_ok(&limits) || !motor_ok(&motor) {
                return None;
            }
            let mut b = RevoluteJointBuilder::new(vector(Vec3::X));
            if let Some(l) = limits {
                b = b.limits(l);
            }
            if let Some(m) = motor {
                b = b.motor_velocity(m.speed, MOTOR_GAIN).motor_max_force(m.max_force);
            }
            (b.into(), Quat::from_rotation_arc(Vec3::X, axis))
        }
        JointKind3::Prismatic { axis, limits, motor } => {
            let axis = axis.try_normalize()?;
            if !limits_ok(&limits) || !motor_ok(&motor) {
                return None;
            }
            let mut b = PrismaticJointBuilder::new(vector(Vec3::X));
            if let Some(l) = limits {
                b = b.limits(l);
            }
            if let Some(m) = motor {
                b = b.motor_velocity(m.speed, MOTOR_GAIN).motor_max_force(m.max_force);
            }
            (b.into(), Quat::from_rotation_arc(Vec3::X, axis))
        }
        JointKind3::Rope { length } => {
            if !(length.is_finite() && length > 0.0) {
                return None;
            }
            (RopeJointBuilder::new(length).into(), Quat::IDENTITY)
        }
        JointKind3::Spring { length, stiffness, damping } => {
            let ok = length.is_finite() && length >= 0.0 && stiffness.is_finite() && stiffness >= 0.0 && damping.is_finite() && damping >= 0.0;
            if !ok {
                return None;
            }
            (SpringJointBuilder::new(length, stiffness, damping).spring_model(MotorModel::AccelerationBased).into(), Quat::IDENTITY)
        }
    };
    let (turn1, turn2) = match joint.kind {
        // 绳子、弹簧、球窝只管距离，不管朝向
        JointKind3::Rope { .. } | JointKind3::Spring { .. } | JointKind3::Ball => (Quat::IDENTITY, Quat::IDENTITY),
        _ => (turn, joint.rotation.normalize().inverse() * turn),
    };
    data.set_local_frame1(Pose::from_parts(vector(joint.anchor_a), rotation(turn1)));
    data.set_local_frame2(Pose::from_parts(vector(joint.anchor_b), rotation(turn2)));
    data.set_contacts_enabled(joint.collide);
    Some(data)
}

use Joint3dKind as JointKind3;
