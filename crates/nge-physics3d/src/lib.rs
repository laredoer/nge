//! 3D 物理（rapier3d）：给带 `Transform3d` 的实体加 [`RigidBody`] / [`Collider`]，
//! 固定步长模拟，动态刚体的位置、旋转、速度写回组件，碰撞时发 [`Collision3d`] 消息（见 docs/physics3d.md）。

mod world;

pub use world::Physics3d;

use bevy_ecs::prelude::*;
use glam::Vec3;
use nge_ecs::{App, Plugin, PostUpdate};

/// 刚体种类。位置取实体的 `Transform3d`（平移和旋转，不看缩放）。
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RigidBody {
    Dynamic,
    Fixed,
    /// 跟着游戏设的 `Transform3d` 平滑移动，会推开动态刚体。
    Kinematic,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// 整个长宽高。
    Box(Vec3),
    /// 半径。
    Ball(f32),
    /// 竖着（沿 y）的胶囊：中间圆柱的半高、半径。
    Capsule { half_height: f32, radius: f32 },
}

/// 碰撞体。没有 [`RigidBody`] 的实体当成固定碰撞体。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Collider {
    pub shape: Shape,
    pub restitution: f32,
    pub friction: f32,
    pub density: f32,
    /// 只检测重叠，不产生碰撞响应。
    pub sensor: bool,
    /// 碰撞开始 / 结束时发 [`Collision3d`]。
    pub events: bool,
}

impl Collider {
    pub fn cuboid(size: Vec3) -> Self {
        Self::new(Shape::Box(size))
    }

    pub fn ball(radius: f32) -> Self {
        Self::new(Shape::Ball(radius))
    }

    pub fn capsule(half_height: f32, radius: f32) -> Self {
        Self::new(Shape::Capsule { half_height, radius })
    }

    fn new(shape: Shape) -> Self {
        Self { shape, restitution: 0.0, friction: 0.5, density: 1.0, sensor: false, events: false }
    }
}

/// 速度（单位 / s，弧度 / s 绕各轴）。动态刚体每步写回；游戏改了就推给物理。
#[derive(Component, Debug, Default, Clone, Copy, PartialEq)]
pub struct Velocity {
    pub linear: Vec3,
    pub angular: Vec3,
}

/// 冲量，下一步施加到刚体上后清零。
#[derive(Component, Debug, Default, Clone, Copy, PartialEq)]
pub struct Impulse(pub Vec3);

/// 正在接触的另一个碰撞体。`normal` 是从对方指向这个实体的单位向量（放在地上时朝上）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contact {
    pub other: Entity,
    pub normal: Vec3,
}

/// 射线碰到的碰撞体（[`Physics3d::raycast`]）。`normal` 是碰到的面朝外的单位法线，`distance` 是从起点走了多远。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    pub entity: Entity,
    pub point: Vec3,
    pub normal: Vec3,
    pub distance: f32,
}

/// 射线算哪些碰撞体：默认不算 sensor。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueryFilter {
    exclude: Option<Entity>,
    sensors: bool,
}

impl QueryFilter {
    /// 不算这个实体的刚体上的碰撞体（通常是自己）。
    pub fn exclude(self, entity: Entity) -> Self {
        Self { exclude: Some(entity), ..self }
    }

    /// sensor 也算。
    pub fn with_sensors(self) -> Self {
        Self { sensors: true, ..self }
    }
}

/// 两个碰撞体开始（`started`）或停止接触。其中一个开了 [`Collider::events`] 才发。
/// 实体删掉时发的停止接触里，`a` / `b` 可能已经不存在了。
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Collision3d {
    pub a: Entity,
    pub b: Entity,
    pub started: bool,
}

/// 3D 物理系统（PostUpdate）。要在同一帧里读模拟结果的系统排在它后面。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Physics3dStepped;

/// 固定步长。
pub const STEP: f32 = 1.0 / 60.0;
/// 一帧最多补几步：卡顿时宁可变慢，也不越补越多。
pub const MAX_STEPS: u32 = 4;

pub struct Physics3dPlugin {
    pub gravity: Vec3,
    /// 一米多少个单位，交给 rapier 的 `length_unit`，让它的容差跟着缩放。
    pub length_unit: f32,
}

impl Default for Physics3dPlugin {
    fn default() -> Self {
        Self { gravity: Vec3::new(0.0, -9.81, 0.0), length_unit: 1.0 }
    }
}

impl Plugin for Physics3dPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Physics3d::new(self.gravity, self.length_unit))
            .add_message::<Collision3d>()
            .add_systems(PostUpdate, (world::remove, world::create, world::step).chain().in_set(Physics3dStepped));
    }
}

pub mod prelude {
    pub use crate::{Collider, Collision3d, Contact, Impulse, Physics3d, Physics3dPlugin, QueryFilter, RayHit, RigidBody, Shape, Velocity};
}

#[cfg(test)]
mod tests;
