//! 2D 物理（rapier2d）：给带 `Transform2d` 的实体加 [`RigidBody`] / [`Collider`]，
//! 固定步长模拟，动态刚体的位置和速度写回组件，碰撞时发 [`Collision`] 消息。

mod character;
mod debug;
mod joint;
mod oneway;
mod world;

pub use character::{CONTACT_GAP, Character};
pub use debug::PhysicsDebug;
pub use joint::{Joint, JointKind, Motor};
pub use world::Physics;

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_ecs::{App, Plugin, PostUpdate};

/// 刚体种类。位置取实体的 `Transform2d`（平移和旋转，不看缩放）。
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RigidBody {
    Dynamic,
    Fixed,
    /// 跟着游戏设的 `Transform2d` 平滑移动，会推开动态刚体。
    Kinematic,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// 整个宽高。
    Rect(Vec2),
    /// 半径。
    Circle(f32),
    /// 斜坡（瓦片地图的斜坡格）：宽高 `size` 的格子里，底边平、两边竖直，顶边从左边高 `left` 到右边高 `right`（都是格高的几分之几，0 … 1）。
    Slope { size: Vec2, left: f32, right: f32 },
}

impl Shape {
    /// 斜坡的四个角（以中心为原点，逆时针）；别的形状是 `None`。
    pub fn slope_corners(&self) -> Option<[Vec2; 4]> {
        let Shape::Slope { size, left, right } = *self else { return None };
        let size = size.abs();
        let h = size / 2.0;
        let top = |f: f32| -h.y + f.clamp(0.0, 1.0) * size.y;
        Some([Vec2::new(-h.x, -h.y), Vec2::new(h.x, -h.y), Vec2::new(h.x, top(right)), Vec2::new(-h.x, top(left))])
    }
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
    /// 碰撞开始 / 结束时发 [`Collision`]。
    pub events: bool,
    /// 碰撞分组（1 … 31）：同一组的互相不碰撞（两个玩家的角色互相穿过），和别的照常碰；0 是不分组。
    pub group: u8,
    /// 单向平台：只挡从上面（碰撞体自己的 +y 方向）来的，从下面、侧面穿过去（见 docs/gameplay2d.md）。
    pub one_way: bool,
}

impl Collider {
    pub fn rect(size: Vec2) -> Self {
        Self::new(Shape::Rect(size))
    }

    pub fn circle(radius: f32) -> Self {
        Self::new(Shape::Circle(radius))
    }

    /// 斜坡：见 [`Shape::Slope`]。
    pub fn slope(size: Vec2, left: f32, right: f32) -> Self {
        Self::new(Shape::Slope { size, left, right })
    }

    fn new(shape: Shape) -> Self {
        Self { shape, restitution: 0.0, friction: 0.5, density: 1.0, sensor: false, events: false, group: 0, one_way: false }
    }
}

/// 速度（point / s，弧度 / s）。动态刚体每步写回；游戏改了就推给物理。
#[derive(Component, Debug, Default, Clone, Copy, PartialEq)]
pub struct Velocity {
    pub linear: Vec2,
    pub angular: f32,
}

/// 锁住刚体的旋转：碰撞不会让它转起来（比如平台游戏的角色）。
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LockRotation;

/// 正在接触的另一个碰撞体。`normal` 是从对方指向这个实体的单位向量（站在地上时朝上）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contact {
    pub other: Entity,
    pub normal: Vec2,
}

/// 射线碰到的碰撞体（[`Physics::raycast`]）。`normal` 是碰到的面朝外的单位法线，`distance` 是从起点走了多远。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    pub entity: Entity,
    pub point: Vec2,
    pub normal: Vec2,
    pub distance: f32,
}

/// 空间查询算哪些碰撞体：默认不算 sensor。
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

/// 冲量，下一步施加到刚体上后清零。
#[derive(Component, Debug, Default, Clone, Copy, PartialEq)]
pub struct Impulse(pub Vec2);

/// 两个碰撞体开始（`started`）或停止接触。其中一个开了 [`Collider::events`] 才发。
/// 实体删掉时发的停止接触里，`a` / `b` 可能已经不存在了。
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Collision {
    pub a: Entity,
    pub b: Entity,
    pub started: bool,
}

/// 物理系统（PostUpdate）。要在同一帧里读模拟结果的系统排在它后面。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PhysicsStepped;

/// 固定步长。
pub const STEP: f32 = 1.0 / 60.0;
/// 一帧最多补几步：卡顿时宁可变慢，也不越补越多。
pub const MAX_STEPS: u32 = 4;

pub struct PhysicsPlugin {
    /// point / s²。
    pub gravity: Vec2,
    /// 一米多少 point，交给 rapier 的 `length_unit`，让它的容差按 point 缩放。
    pub pixels_per_meter: f32,
}

impl Default for PhysicsPlugin {
    fn default() -> Self {
        Self { gravity: Vec2::new(0.0, -9.81 * 100.0), pixels_per_meter: 100.0 }
    }
}

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Physics::new(self.gravity, self.pixels_per_meter))
            .add_message::<Collision>()
            .init_resource::<PhysicsDebug>()
            .add_systems(PostUpdate, (world::remove, world::create, joint::sync, world::step).chain().in_set(PhysicsStepped))
            .add_systems(PostUpdate, debug::draw.after(PhysicsStepped).run_if(debug::enabled));
    }
}

pub mod prelude {
    pub use crate::{Character, Collider, Collision, Contact, Impulse, Joint, JointKind, LockRotation, Motor, Physics, PhysicsDebug, PhysicsPlugin, QueryFilter, RayHit, RigidBody, Shape, Velocity};
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod joint_tests;
#[cfg(test)]
mod character_tests;
#[cfg(test)]
mod oneway_tests;
#[cfg(test)]
mod slope_tests;
