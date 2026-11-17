//! 关节：把两个刚体（或一个刚体和世界）连起来，用 rapier 的 impulse joint。

use bevy_ecs::prelude::*;
use glam::Vec2;
use rapier2d::math::{Pose, Rotation};
use rapier2d::prelude::{
    FixedJointBuilder, GenericJoint, ImpulseJointHandle, MotorModel, PrismaticJointBuilder, RevoluteJointBuilder, RigidBodyBuilder, RigidBodyHandle, RopeJointBuilder,
    SpringJointBuilder,
};

use crate::world::vector;
use crate::Physics;

/// 关节，挂在单独的实体上。`a` 是 `None` 时连到世界上（`anchor_a` 是世界坐标）。
/// 两头都要是建到物理里的刚体，还不是的时候先不连，成了刚体再连上；刚体重建（换了种类或形状）后自动重连。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Joint {
    pub a: Option<Entity>,
    pub b: Entity,
    pub kind: JointKind,
    /// 在 a 自己的坐标里（没转过、原点在 a 的中心）。
    pub anchor_a: Vec2,
    /// 在 b 自己的坐标里。
    pub anchor_b: Vec2,
    /// b 相对 a 的角度（弧度）：固定、滑动关节保持这个角度，转动关节的 `limits` 从这里算起。
    pub angle: f32,
    /// 连着的两个刚体之间还碰撞。默认不碰。
    pub collide: bool,
}

impl Joint {
    pub fn new(a: Option<Entity>, b: Entity, kind: JointKind) -> Self {
        Self { a, b, kind, anchor_a: Vec2::ZERO, anchor_b: Vec2::ZERO, angle: 0.0, collide: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JointKind {
    /// 焊死：位置和角度都不变。
    Fixed,
    /// 绕锚点转。`limits` 是相对角度的范围（弧度），`motor` 的速度是弧度 / 秒。
    Revolute { limits: Option<[f32; 2]>, motor: Option<Motor> },
    /// 沿 `axis`（a 的坐标里，会归一化）滑，不转。`limits` 是沿轴的距离范围，`motor` 的速度是 point / s。
    Prismatic { axis: Vec2, limits: Option<[f32; 2]>, motor: Option<Motor> },
    /// 绳子：两个锚点最远 `length`，可以靠近。
    Rope { length: f32 },
    /// 弹簧：拉向 `length`。`stiffness`、`damping` 按加速度算，和质量无关。
    Spring { length: f32, stiffness: f32, damping: f32 },
}

impl JointKind {
    /// 不限角度、没有马达的转动关节。
    pub fn revolute() -> Self {
        Self::Revolute { limits: None, motor: None }
    }
}

/// 马达：把相对速度拉到 `speed`，最多用 `max_force` 的力（`f32::INFINITY` 不限）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motor {
    pub speed: f32,
    pub max_force: f32,
}

/// 马达跟速度跟得多紧（rapier 的 damping，按加速度算）。
const MOTOR_GAIN: f32 = 1000.0;

/// 处理过的关节和它的声明：`handle` 是 `None` 时参数不对、没连（声明不变就不再试，也不再 warn）。
pub(crate) struct Linked {
    handle: Option<ImpulseJointHandle>,
    joint: Joint,
}

impl Linked {
    pub(crate) fn is_linked(&self) -> bool {
        self.handle.is_some()
    }
}

/// 每帧对一遍：没了的拆掉，新的、变了的重建；刚体删掉或重建时 rapier 顺带拆了关节，这里也重建。
pub(crate) fn sync(mut physics: ResMut<Physics>, joints: Query<(Entity, &Joint)>) {
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
            log::warn!("关节 {entity:?} 的参数不对（不是有限的数、长度不是正数、范围反了或超过一圈、轴是 0，或者两头是同一个刚体），不连");
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
fn bodies(physics: &mut Physics, joint: &Joint) -> Option<(RigidBodyHandle, RigidBodyHandle)> {
    let body = |physics: &Physics, e: Entity| physics.entities.get(&e).and_then(|h| h.body);
    let b = body(physics, joint.b)?;
    let a = match joint.a {
        Some(a) => body(physics, a)?,
        None => *physics.ground.get_or_insert_with(|| physics.world.insert_body(RigidBodyBuilder::fixed())),
    };
    Some((a, b))
}

fn generic(joint: &Joint) -> Option<GenericJoint> {
    let finite = joint.anchor_a.is_finite() && joint.anchor_b.is_finite() && joint.angle.is_finite();
    if !finite || joint.a == Some(joint.b) {
        return None;
    }
    let motor_ok = |m: &Option<Motor>| m.is_none_or(|m| m.speed.is_finite() && !m.max_force.is_nan() && m.max_force >= 0.0);
    let limits_ok = |l: &Option<[f32; 2]>| l.is_none_or(|[lo, hi]| lo.is_finite() && hi.is_finite() && lo <= hi);
    // rapier 里转动关节的范围超过一整圈就等于不限
    let turn_ok = |l: &Option<[f32; 2]>| l.is_none_or(|[lo, hi]| hi - lo <= std::f32::consts::TAU);
    // 滑动关节的轴是锚点坐标系的 x 轴：frame1 转到 a 坐标里的轴向，frame2 再减去 b 相对 a 的角度（两个 frame 的朝向要一致）
    let (mut data, base): (GenericJoint, f32) = match joint.kind {
        JointKind::Fixed => (FixedJointBuilder::new().into(), 0.0),
        JointKind::Revolute { limits, motor } => {
            if !limits_ok(&limits) || !turn_ok(&limits) || !motor_ok(&motor) {
                return None;
            }
            let mut b = RevoluteJointBuilder::new();
            if let Some(l) = limits {
                b = b.limits(l);
            }
            if let Some(m) = motor {
                b = b.motor_velocity(m.speed, MOTOR_GAIN).motor_max_force(m.max_force);
            }
            (b.into(), 0.0)
        }
        JointKind::Prismatic { axis, limits, motor } => {
            let axis = axis.try_normalize()?;
            if !limits_ok(&limits) || !motor_ok(&motor) {
                return None;
            }
            let mut b = PrismaticJointBuilder::new(vector(Vec2::X));
            if let Some(l) = limits {
                b = b.limits(l);
            }
            if let Some(m) = motor {
                b = b.motor_velocity(m.speed, MOTOR_GAIN).motor_max_force(m.max_force);
            }
            (b.into(), axis.to_angle())
        }
        JointKind::Rope { length } => {
            if !(length.is_finite() && length > 0.0) {
                return None;
            }
            (RopeJointBuilder::new(length).into(), 0.0)
        }
        JointKind::Spring { length, stiffness, damping } => {
            let ok = length.is_finite() && length >= 0.0 && stiffness.is_finite() && stiffness >= 0.0 && damping.is_finite() && damping >= 0.0;
            if !ok {
                return None;
            }
            (SpringJointBuilder::new(length, stiffness, damping).spring_model(MotorModel::AccelerationBased).into(), 0.0)
        }
    };
    let (turn1, turn2) = match joint.kind {
        // 绳子和弹簧只管距离，不管角度
        JointKind::Rope { .. } | JointKind::Spring { .. } => (0.0, 0.0),
        _ => (base, base - joint.angle),
    };
    data.set_local_frame1(Pose::from_parts(vector(joint.anchor_a), Rotation::new(turn1)));
    data.set_local_frame2(Pose::from_parts(vector(joint.anchor_b), Rotation::new(turn2)));
    data.set_contacts_enabled(joint.collide);
    Some(data)
}
