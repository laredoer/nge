//! 3D 物理（rapier3d）：给带 `Transform3d` 的实体加 [`RigidBody`] / [`Collider`]，
//! 固定步长模拟，动态刚体的位置、旋转、速度写回组件，碰撞时发 [`Collision3d`] 消息（见 docs/physics3d.md）；
//! 角色控制器、关节、网格碰撞体、空间查询、调试画线见 docs/gameplay3d.md。

mod character;
mod debug;
mod joint;
mod mesh;
mod query;
mod world;

pub use character::{Character3d, CharacterState3d};
pub use debug::Physics3dDebug;
pub use joint::{Joint3d, Joint3dKind, Motor3d};
pub use mesh::{MAX_HULL_POINTS, MAX_TRIANGLES, MeshCollider, MeshShape, mesh_shape, too_big};
pub use world::Physics3d;

use std::sync::Arc;

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

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// 整个长宽高。
    Box(Vec3),
    /// 半径。
    Ball(f32),
    /// 竖着（沿 y）的胶囊：中间圆柱的半高、半径。
    Capsule { half_height: f32, radius: f32 },
    /// 这些点的凸包（一般直接存凸包的顶点）。
    Convex(Arc<Vec<Vec3>>),
    /// 几个凸包合成的一个形状。
    Hulls(Arc<Vec<Vec<Vec3>>>),
    /// 三角网格（顶点、三角形）。只能用在固定的上：挂在动态、kinematic 刚体上时换成这些顶点的凸包。
    TriMesh(Arc<(Vec<Vec3>, Vec<[u32; 3]>)>),
}

/// 碰撞体。没有 [`RigidBody`] 的实体当成固定碰撞体。
#[derive(Component, Debug, Clone, PartialEq)]
pub struct Collider {
    pub shape: Shape,
    pub restitution: f32,
    pub friction: f32,
    pub density: f32,
    /// 只检测重叠，不产生碰撞响应。
    pub sensor: bool,
    /// 碰撞开始 / 结束时发 [`Collision3d`]。
    pub events: bool,
    /// 碰撞分组（1 … 31）：同一组的互相不碰撞，和别的照常碰；0 是不分组。
    pub group: u8,
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

    /// 任意形状，其余参数用默认值。
    pub fn new(shape: Shape) -> Self {
        Self { shape, restitution: 0.0, friction: 0.5, density: 1.0, sensor: false, events: false, group: 0 }
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

/// 形状投射碰到的碰撞体（[`Physics3d::cast_shape`]）。`point` 在对方表面上，`normal` 是对方表面朝外的单位法线，
/// `distance` 是形状沿方向走了多远。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeHit {
    pub entity: Entity,
    pub point: Vec3,
    pub normal: Vec3,
    pub distance: f32,
}

/// 最近点（[`Physics3d::closest_point`]）：`point` 在碰撞体表面上；查的点在实心碰撞体里面时 `inside` 为真、`point` 是查的点、`distance` 是 0。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointHit {
    pub entity: Entity,
    pub point: Vec3,
    pub distance: f32,
    pub inside: bool,
}

/// 查询用的形状。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueryShape {
    /// 半径。
    Ball(f32),
    /// 竖着（沿 y）的胶囊：中间圆柱的半高、半径。
    Capsule { half_height: f32, radius: f32 },
    /// 整个长宽高。
    Box(Vec3),
}

/// 空间查询算哪些碰撞体：默认不算 sensor、所有分组都算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryFilter {
    exclude: Option<Entity>,
    sensors: bool,
    group: u8,
    only: u32,
}

impl Default for QueryFilter {
    fn default() -> Self {
        Self { exclude: None, sensors: false, group: 0, only: u32::MAX }
    }
}

impl QueryFilter {
    /// 换成 rapier 的分组：属于 `group` 那一位（不写时全部），只和 `only` 里的位碰；同组的由碰撞体那边的掩码挡掉。
    pub(crate) fn groups(&self) -> rapier3d::prelude::InteractionGroups {
        use rapier3d::prelude::{Group, InteractionGroups, InteractionTestMode};
        let own = if (1..=31).contains(&self.group) { Group::from_bits_truncate(1 << self.group) } else { Group::empty() };
        let member = if own.is_empty() { Group::ALL } else { own };
        InteractionGroups::new(member, Group::from_bits_truncate(self.only), InteractionTestMode::And)
    }

    /// 不算这个实体的刚体上的碰撞体（通常是自己）。
    pub fn exclude(self, entity: Entity) -> Self {
        Self { exclude: Some(entity), ..self }
    }

    /// sensor 也算。
    pub fn with_sensors(self) -> Self {
        Self { sensors: true, ..self }
    }

    /// 像碰撞分组 `group` 的碰撞体一样查：跳过同一组的（1 … 31；0 不跳过）。
    pub fn group(self, group: u8) -> Self {
        Self { group, ..self }
    }

    /// 只查这些分组：第 g 位是分组 g，第 0 位是不分组的。
    pub fn only(self, mask: u32) -> Self {
        Self { only: mask, ..self }
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
            .init_resource::<Physics3dDebug>()
            .add_systems(PostUpdate, (mesh::build, world::remove, world::create, joint::sync, world::step).chain().in_set(Physics3dStepped))
            .add_systems(PostUpdate, debug::draw.after(Physics3dStepped));
    }
}

pub mod prelude {
    pub use crate::{Character3d, CharacterState3d, Collider, Joint3d, Joint3dKind, MeshCollider, MeshShape, Motor3d, Collision3d, Contact, Impulse, Physics3d, Physics3dDebug, Physics3dPlugin, PointHit, QueryFilter, QueryShape, RayHit, RigidBody, Shape, ShapeHit, Velocity};
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod query_tests;
#[cfg(test)]
mod character_tests;
#[cfg(test)]
mod joint_tests;
#[cfg(test)]
mod mesh_tests;
#[cfg(test)]
mod debug_tests;
