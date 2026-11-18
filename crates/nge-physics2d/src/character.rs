//! 角色控制器：游戏给速度，引擎负责重力、贴地、沿墙滑、爬坡、上台阶、被脚下的平台带着走（见 docs/character.md）。

use bevy_ecs::prelude::*;
use glam::Vec2;
use rapier2d::parry::bounding_volume::BoundingVolume;
use rapier2d::control::{CharacterAutostep, CharacterLength, KinematicCharacterController};
use rapier2d::parry::query::contact;
use rapier2d::prelude::QueryFilter as RapierFilter;

use crate::{Contact, oneway};
use crate::world::{Physics, vec2, vector};

/// 贴着多近（point）算接触。
pub const CONTACT_GAP: f32 = 2.0;
/// 法线和水平 / 竖直方向差多少算撞墙、顶天花板（cos 45°）。
const SIDE: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// 组件：角色控制器。和 `RigidBody::Kinematic`、`Collider`、`Velocity` 一起用，游戏只改 `Velocity`。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Character {
    /// 能爬的最陡的坡（弧度），更陡的会滑下来。
    pub max_slope: f32,
    /// 自动迈上去的台阶最高多高（point），0 不自动上。
    pub step: f32,
    /// 离地多近就往下贴（point），下坡、下台阶时不飞出去；0 不贴。
    pub snap: f32,
    pub gravity_scale: f32,
}

impl Default for Character {
    fn default() -> Self {
        Self { max_slope: 50f32.to_radians(), step: 0.0, snap: 4.0, gravity_scale: 1.0 }
    }
}

/// 物理里记着的角色状态。
#[derive(Debug, Clone, Default)]
pub(crate) struct Mover {
    pub params: Character,
    pub grounded: bool,
    /// 脚下的刚体（站着的那个里动得最快的），每一步跟着它这一步的位移走。
    pub ground: Option<rapier2d::prelude::RigidBodyHandle>,
    pub touching: Vec<Contact>,
}

fn controller(c: &Character) -> KinematicCharacterController {
    let valid = |v: f32| v.is_finite() && v > 0.0;
    let slope = if c.max_slope.is_finite() { c.max_slope } else { Character::default().max_slope };
    KinematicCharacterController {
        autostep: valid(c.step).then(|| CharacterAutostep { max_height: CharacterLength::Absolute(c.step), include_dynamic_bodies: false, ..Default::default() }),
        max_slope_climb_angle: slope,
        min_slope_slide_angle: slope,
        snap_to_ground: valid(c.snap).then_some(CharacterLength::Absolute(c.snap)),
        ..Default::default()
    }
}

/// 物理走一步之前：按速度移动每个角色，设好 kinematic 的下一个位置，改速度。
pub(crate) fn advance(physics: &mut Physics, dt: f32) {
    let gravity = vec2(physics.world.gravity);
    let movers: Vec<Entity> = physics.entities.iter().filter(|(_, h)| h.mover.is_some()).map(|(&e, _)| e).collect();
    for entity in movers {
        let h = physics.entities.get_mut(&entity).expect("刚收集的");
        let (Some(body), Some(collider), Some(mover)) = (h.body, h.collider, h.mover.as_mut()) else { continue };
        let world = &mut physics.world;
        let (Some(pose), Some(col)) = (world.bodies.get(body).map(|b| *b.position()), world.colliders.get(collider)) else { continue };
        let (shape, mass, groups) = (col.shared_shape().clone(), col.mass(), col.collision_groups());
        let mut v = h.velocity.linear;
        // 站在地上、没往上走时不加重力：不然每步都往地里压一下
        if !(mover.grounded && v.y <= 0.0) && mover.params.gravity_scale.is_finite() {
            v += gravity * mover.params.gravity_scale * dt;
        }
        // 脚下的东西这一步要走的位移：kinematic 是游戏设好的下一个位置，动态刚体按速度
        let carry = mover.ground.and_then(|g| world.bodies.get(g)).map_or(Vec2::ZERO, |g| {
            if g.is_kinematic() { vec2(g.next_position().translation - g.translation()) } else { vec2(g.linvel()) * dt }
        });
        if !(v * dt + carry).is_finite() {
            h.velocity.linear = Vec2::ZERO;
            continue;
        }
        let ctl = controller(&mover.params);
        // 同一个碰撞分组的不挡路
        let filter = RapierFilter::default().exclude_sensors().exclude_rigid_body(body).groups(groups);
        let dropping = &physics.dropping;
        // 单向平台：角色整个在它上面、又没在往下穿时才挡（按这一步开始时的位置算）
        let solid = |h: rapier2d::prelude::ColliderHandle, c: &rapier2d::prelude::Collider| oneway::blocks(dropping, collider, &*shape, &pose, h, c);
        let mut hits = Vec::new();
        // 先走自己的这段：脚下的东西这一步还没挪，相对它走
        let out = ctl.move_shape(dt, &world.query_pipeline_with_filter(filter.predicate(&solid)), &*shape, &pose, vector(v * dt), |hit| hits.push(hit));
        // 再从走到的地方跟着脚下的东西走，查碰撞时不算它（它还在旧位置，往下走的平台会挡住角色）
        let mut moved = pose;
        moved.translation += out.translation;
        if carry != Vec2::ZERO {
            let ground = mover.ground;
            let not_ground = |h: rapier2d::prelude::ColliderHandle, c: &rapier2d::prelude::Collider| c.parent() != ground && solid(h, c);
            let ride = KinematicCharacterController { autostep: None, snap_to_ground: None, ..ctl };
            let query = world.query_pipeline_with_filter(filter.predicate(&not_ground));
            let carried = ride.move_shape(dt, &query, &*shape, &moved, vector(carry), |hit| hits.push(hit));
            moved.translation += carried.translation;
        }
        // 推开动态刚体
        let mut queries = world.broad_phase.as_query_pipeline_mut(world.narrow_phase.query_dispatcher(), &mut world.bodies, &mut world.colliders, filter);
        ctl.solve_character_collision_impulses(dt, &mut queries, &*shape, mass, &hits);
        if let Some(b) = world.bodies.get_mut(body) {
            b.set_next_kinematic_translation(moved.translation);
        }
        mover.grounded = out.grounded;
        if mover.grounded && v.y < 0.0 {
            v.y = 0.0;
        }
        for hit in &hits {
            // normal1 是碰到的东西表面朝外的法线（从对方指向角色），世界坐标
            let n = vec2(hit.hit.normal1);
            if n.y < -SIDE && v.y > 0.0 {
                v.y = 0.0;
            }
            // 比能爬的坡还陡的算墙
            if n.y > -SIDE && n.y < mover.params.max_slope.cos() && v.x * n.x < 0.0 {
                v.x = 0.0;
            }
        }
        h.velocity.linear = v;
    }
}

/// 物理走完这一帧的步之后：查贴着角色的碰撞体当接触，记下脚下的刚体，下一帧跟着它走。
pub(crate) fn touch(physics: &mut Physics) {
    let movers: Vec<Entity> = physics.entities.iter().filter(|(_, h)| h.mover.is_some()).map(|(&e, _)| e).collect();
    for entity in movers {
        let h = &physics.entities[&entity];
        let (Some(body), Some(collider)) = (h.body, h.collider) else { continue };
        let Some(col) = physics.world.colliders.get(collider) else { continue };
        let (pose, shape) = (*col.position(), col.shared_shape().clone());
        let dropping = &physics.dropping;
        let solid = |h: rapier2d::prelude::ColliderHandle, c: &rapier2d::prelude::Collider| oneway::blocks(dropping, collider, &*shape, &pose, h, c);
        let filter = RapierFilter::default().exclude_sensors().exclude_rigid_body(body).groups(col.collision_groups()).predicate(&solid);
        let queries = physics.world.query_pipeline_with_filter(filter);
        let aabb = shape.compute_aabb(&pose).loosened(CONTACT_GAP);
        let touching: Vec<Contact> = queries
            .intersect_aabb_conservative(aabb)
            .filter_map(|(handle, other)| {
                let c = contact(&pose, &*shape, other.position(), other.shape(), CONTACT_GAP).ok()??;
                Some(Contact { other: physics.entity_of(handle)?, normal: -vec2(c.normal1) })
            })
            .collect();
        let max_slope = h.mover.as_ref().map_or(0.0, |m| m.params.max_slope);
        let max_slope = if max_slope.is_finite() { max_slope } else { Character::default().max_slope };
        let ground = touching
            .iter()
            .filter(|c| c.normal.y >= max_slope.cos())
            .filter_map(|c| {
                // 别的角色不当地面：它这一步走没走取决于处理的先后
                let h = physics.entities.get(&c.other).filter(|h| h.mover.is_none())?;
                h.body.filter(|_| matches!(h.kind, Some(crate::RigidBody::Kinematic | crate::RigidBody::Dynamic))).map(|b| (b, h.velocity.linear))
            })
            .max_by(|a, b| a.1.length_squared().total_cmp(&b.1.length_squared()))
            .map(|(b, _)| b);
        let mover = physics.entities.get_mut(&entity).and_then(|h| h.mover.as_mut()).expect("刚查过");
        (mover.touching, mover.ground) = (touching, ground);
    }
}
