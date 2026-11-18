//! 3D 角色控制器：游戏给速度，引擎负责重力、贴地、沿墙滑、爬坡、上台阶、被脚下的平台带着走（见 docs/gameplay3d.md）。

use bevy_ecs::prelude::*;
use glam::{Quat, Vec3};

use crate::Contact;

/// 组件：角色控制器。和 `RigidBody::Kinematic`、`Collider`（一般是胶囊）、`Velocity` 一起用，游戏只改 `Velocity`。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
#[require(CharacterState3d)]
pub struct Character3d {
    /// 能爬的最陡的坡（弧度），更陡的会滑下来。
    pub max_slope: f32,
    /// 自动迈上去的台阶最高多高（世界单位），0 不自动上。
    pub step: f32,
    /// 离地多近就往下贴（世界单位），下坡、下台阶时不飞出去；0 不贴。
    pub snap: f32,
    pub gravity_scale: f32,
    /// 站在转动的东西上时朝向也跟着转；默认只跟着走、朝向不变（游戏自己管朝哪）。
    pub turn_with_ground: bool,
}

/// 默认值按 point 给（和引擎的重力 -981、一米 100 一致）：贴地 4 ≈ 4 厘米。
/// 改了 `length_unit`、按米算的游戏自己设 `snap`、`step`。
impl Default for Character3d {
    fn default() -> Self {
        Self { max_slope: 50f32.to_radians(), step: 0.0, snap: 4.0, gravity_scale: 1.0, turn_with_ground: false }
    }
}

/// 角色这一帧的结果，每帧物理走完写一次：游戏拿去判断能不能跳、设动画状态机的参数。
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct CharacterState3d {
    pub grounded: bool,
    /// 脚下地面的法线；没着地是 0。
    pub ground_normal: Vec3,
    /// 这一帧实际的位移 ÷ 时间（含被平台带着走的、被墙挡掉的）。
    pub velocity: Vec3,
    /// 这一帧碰到的东西：法线从对方指向角色。
    pub hits: Vec<Contact>,
}

use rapier3d::control::{CharacterAutostep, CharacterCollision, CharacterLength, KinematicCharacterController};
use rapier3d::parry::bounding_volume::BoundingVolume;
use rapier3d::parry::query::contact;
use rapier3d::prelude::{ColliderHandle, QueryFilter as RapierFilter, RigidBodyHandle};

use crate::Physics3d;
use crate::world::{quat, rotation, vec3, vector};

/// 贴着多近（乘 `length_unit`）算接触。
const CONTACT_GAP: f32 = 0.02;
/// 法线和竖直方向差多少算顶天花板（cos 45°）。
const SIDE: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// 物理里记着的角色状态。
#[derive(Debug, Clone, Default)]
pub(crate) struct Mover {
    pub params: Character3d,
    pub grounded: bool,
    /// 脚下的刚体（站着的那个里动得最快的），每一步跟着它这一步的位移走。
    pub ground: Option<RigidBodyHandle>,
    pub ground_normal: Vec3,
    /// 走完这一帧之后贴着的碰撞体。
    pub touching: Vec<Contact>,
    /// 这一帧控制器碰到的（每个碰撞体留最后一次）。
    pub hits: Vec<(ColliderHandle, Vec3)>,
    /// 这一帧开始时的位置，算实际速度用。
    pub start: Vec3,
}

/// 参数里不是有限数的换成默认值。
fn slope(c: &Character3d) -> f32 {
    if c.max_slope.is_finite() { c.max_slope } else { Character3d::default().max_slope }
}

fn controller(c: &Character3d) -> KinematicCharacterController {
    let valid = |v: f32| v.is_finite() && v > 0.0;
    KinematicCharacterController {
        autostep: valid(c.step).then(|| CharacterAutostep { max_height: CharacterLength::Absolute(c.step), include_dynamic_bodies: false, ..Default::default() }),
        max_slope_climb_angle: slope(c),
        min_slope_slide_angle: slope(c),
        snap_to_ground: valid(c.snap).then_some(CharacterLength::Absolute(c.snap)),
        ..Default::default()
    }
}

/// 脚下的东西这一步怎么动：点 p 移到 `turn * (p - pivot) + to`。
#[derive(Debug, Clone, Copy)]
struct Ride {
    turn: Quat,
    pivot: Vec3,
    to: Vec3,
}

impl Ride {
    /// kinematic 按游戏设好的下一个位姿，动态刚体按线速度和绕质心的角速度。
    fn of(g: &rapier3d::prelude::RigidBody, dt: f32) -> Self {
        if g.is_kinematic() {
            let (now, next) = (g.position(), g.next_position());
            Self { turn: quat(next.rotation) * quat(now.rotation).inverse(), pivot: vec3(now.translation), to: vec3(next.translation) }
        } else {
            let com = vec3(g.center_of_mass());
            Self { turn: Quat::from_scaled_axis(vec3(g.angvel()) * dt), pivot: com, to: com + vec3(g.linvel()) * dt }
        }
    }

    fn apply(&self, p: Vec3) -> Vec3 {
        self.turn * (p - self.pivot) + self.to
    }
}

/// 物理走一步之前：按速度移动每个角色，设好 kinematic 的下一个位置，改速度。
pub(crate) fn advance(physics: &mut Physics3d, dt: f32) {
    let gravity = vec3(physics.world.gravity);
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
        if !(v * dt).is_finite() {
            h.velocity.linear = Vec3::ZERO;
            continue;
        }
        let ride = mover.ground.and_then(|g| world.bodies.get(g)).map(|g| Ride::of(g, dt));
        let ctl = controller(&mover.params);
        // 同一个碰撞分组的不挡路
        let filter = RapierFilter::default().exclude_sensors().exclude_rigid_body(body).groups(groups);
        let mut hits: Vec<CharacterCollision> = Vec::new();
        // 先走自己的这段：脚下的东西这一步还没挪，相对它走
        let out = ctl.move_shape(dt, &world.query_pipeline_with_filter(filter), &*shape, &pose, vector(v * dt), |hit| hits.push(hit));
        // 再从走到的地方跟着脚下的东西走，查碰撞时不算它（它还在旧位置，往下走的平台会挡住角色）
        let mut moved = pose;
        moved.translation += out.translation;
        // 脚下的东西这一步的整个移动（含转动）作用到走到的位置上：转盘上沿圆弧走
        let carry = ride.map_or(Vec3::ZERO, |r| r.apply(vec3(moved.translation)) - vec3(moved.translation));
        let carry = if carry.is_finite() { carry } else { Vec3::ZERO };
        if carry != Vec3::ZERO {
            let ground = mover.ground;
            let not_ground = |_: ColliderHandle, c: &rapier3d::prelude::Collider| c.parent() != ground;
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
            if let Some(r) = ride.filter(|r| mover.params.turn_with_ground && r.turn.is_finite()) {
                b.set_next_kinematic_rotation(rotation((r.turn * quat(*b.rotation())).normalize()));
            }
        }
        // 控制器说着地还不够：得踩着够平的面（这一步碰到的，或上一步贴着的），不然陡坡上也算站住了
        let floor = slope(&mover.params).cos();
        let on_floor = mover.ground_normal != Vec3::ZERO || hits.iter().any(|hit| vec3(hit.hit.normal1).y >= floor);
        mover.grounded = out.grounded && on_floor;
        if mover.grounded && v.y < 0.0 {
            v.y = 0.0;
        }
        for hit in &hits {
            // normal1 是碰到的东西表面朝外的法线（从对方指向角色），世界坐标
            let n = vec3(hit.hit.normal1);
            if n.y < -SIDE && v.y > 0.0 {
                v.y = 0.0;
            }
            // 比能爬的坡还陡的算墙：去掉朝墙的水平分量，沿墙的留着
            let side = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
            let into = Vec3::new(v.x, 0.0, v.z).dot(side);
            if n.y > -SIDE && n.y < slope(&mover.params).cos() && into < 0.0 {
                v -= side * into;
            }
            mover.hits.retain(|(c, _)| *c != hit.handle);
            mover.hits.push((hit.handle, n));
        }
        h.velocity.linear = v;
    }
}

/// 物理走完这一帧的步之后：查贴着角色的碰撞体当接触，记下脚下的刚体，下一帧跟着它走。
pub(crate) fn touch(physics: &mut Physics3d) {
    let gap = CONTACT_GAP * physics.world.integration_parameters.length_unit;
    let movers: Vec<Entity> = physics.entities.iter().filter(|(_, h)| h.mover.is_some()).map(|(&e, _)| e).collect();
    for entity in movers {
        let h = &physics.entities[&entity];
        let (Some(body), Some(collider)) = (h.body, h.collider) else { continue };
        let Some(col) = physics.world.colliders.get(collider) else { continue };
        let (pose, shape) = (*col.position(), col.shared_shape().clone());
        let filter = RapierFilter::default().exclude_sensors().exclude_rigid_body(body).groups(col.collision_groups());
        let queries = physics.world.query_pipeline_with_filter(filter);
        let aabb = shape.compute_aabb(&pose).loosened(gap);
        let touching: Vec<Contact> = queries
            .intersect_aabb_conservative(aabb)
            .filter_map(|(handle, other)| {
                let c = contact(&pose, &*shape, other.position(), other.shape(), gap).ok()??;
                Some(Contact { other: physics.entity_of(handle)?, normal: -vec3(c.normal1) })
            })
            .collect();
        let max_slope = h.mover.as_ref().map_or(Character3d::default().max_slope, |m| slope(&m.params));
        let floor: Vec<&Contact> = touching.iter().filter(|c| c.normal.y >= max_slope.cos()).collect();
        let ground_normal = floor.iter().map(|c| c.normal).max_by(|a, b| a.y.total_cmp(&b.y)).unwrap_or(Vec3::ZERO);
        let ground = floor
            .iter()
            .filter_map(|c| {
                // 别的角色不当地面：它这一步走没走取决于处理的先后
                let h = physics.entities.get(&c.other).filter(|h| h.mover.is_none())?;
                let b = h.body.filter(|_| matches!(h.kind, Some(crate::RigidBody::Kinematic | crate::RigidBody::Dynamic)))?;
                // 按角色所在处的速度比：转动的东西离轴越远走得越快
                let speed = physics.world.bodies.get(b).map_or(0.0, |rb| vec3(rb.velocity_at_point(pose.translation)).length_squared());
                Some((b, speed))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(b, _)| b);
        let mover = physics.entities.get_mut(&entity).and_then(|h| h.mover.as_mut()).expect("刚查过");
        // 走完贴着的里没有够平的就不算着地：下一步加重力，陡坡上滑下去
        mover.grounded &= ground_normal != Vec3::ZERO;
        (mover.touching, mover.ground, mover.ground_normal) = (touching, ground, ground_normal);
    }
}

/// 这一帧的结果：`elapsed` 是这一帧走了多少秒的物理。
pub(crate) fn report(physics: &Physics3d, mover: &Mover, now: Vec3, elapsed: f32) -> CharacterState3d {
    let hits = mover.hits.iter().filter_map(|&(c, normal)| Some(Contact { other: physics.entity_of(c)?, normal })).collect();
    let grounded = mover.grounded;
    // 着地时一定贴着够平的面（touch 里保证了），不着地是 0
    let ground_normal = if grounded { mover.ground_normal } else { Vec3::ZERO };
    let velocity = if elapsed > 0.0 { (now - mover.start) / elapsed } else { Vec3::ZERO };
    CharacterState3d { grounded, ground_normal, velocity, hits }
}
