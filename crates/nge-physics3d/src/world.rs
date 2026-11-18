use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use bevy_ecs::prelude::*;
use glam::{Quat, Vec3};
use nge_ecs::Time;
use nge_render3d::Transform3d;
use rapier3d::math::{Pose, Rotation, Vector};
use rapier3d::prelude::{
    ColliderBuilder, ColliderHandle, ColliderSet, CollisionEvent, ContactPair, EventHandler, PhysicsWorld, QueryFilter as RapierFilter, Ray, RigidBodyBuilder,
    RigidBodyHandle, RigidBodySet, SoftBodySet, SoftBodyTearEvent, ActiveEvents,
};
use rapier3d::prelude::{ActiveCollisionTypes, Group, InteractionGroups, InteractionTestMode};

use crate::character::{self, Mover};
use crate::mesh::solid_hull;
use crate::{Character3d, CharacterState3d, Collider, MeshCollider, Collision3d, Contact, Impulse, MAX_STEPS, QueryFilter, RayHit, RigidBody, STEP, Shape, Velocity};

/// rapier 的世界，加上实体和刚体 / 碰撞体的对应关系。
#[derive(Resource)]
pub struct Physics3d {
    pub(crate) world: PhysicsWorld,
    pub(crate) entities: HashMap<Entity, Handles>,
    /// 关节实体 → 建好的关节。
    pub(crate) joints: HashMap<Entity, crate::joint::Linked>,
    /// 连到世界上的关节用的固定刚体（原点，没有碰撞体），用到时才建。
    pub(crate) ground: Option<RigidBodyHandle>,
    /// 刚删掉的碰撞体：rapier 下一步才发它们的停止接触，那时已经查不到 user_data 了。
    removed: HashMap<ColliderHandle, Entity>,
    accumulator: f32,
    /// 调试视图开着时记下这一帧打过的射线和形状投射（查询只拿 `&self`，所以放在锁里）。
    pub(crate) tracing: bool,
    pub(crate) traces: Mutex<Vec<Trace>>,
    /// 有 `MeshCollider`、还在等模型生成碰撞体的实体 → 等了几秒。
    pub(crate) waiting: HashMap<Entity, f32>,
    /// 网格碰撞体生不出来的（模型或缩放改了再试）。
    pub(crate) mesh_failed: HashSet<Entity>,
    /// 已经 warn 过的（实体，哪种）：每种只说一次。
    pub(crate) warnings: HashSet<(Entity, &'static str)>,
}

/// 一次射线或形状投射：从哪到哪（碰到了是到碰撞时的位置），碰到的点和法线。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Trace {
    pub from: Vec3,
    pub to: Vec3,
    /// 形状投射的形状和朝向；射线是 `None`。
    pub shape: Option<(crate::QueryShape, Quat)>,
    pub hit: Option<(Vec3, Vec3)>,
}

/// 一个实体在物理里的东西，以及上一次和组件同步时的值（用来判断游戏改没改）。
pub(crate) struct Handles {
    pub(crate) body: Option<RigidBodyHandle>,
    pub(crate) collider: Option<ColliderHandle>,
    pub(crate) kind: Option<RigidBody>,
    pub(crate) shape: Option<Collider>,
    transform: (Vec3, Quat),
    pub(crate) velocity: Velocity,
    /// 有 `Character3d` 的：角色控制器的状态。
    pub(crate) mover: Option<Mover>,
}

impl Physics3d {
    pub(crate) fn new(gravity: Vec3, length_unit: f32) -> Self {
        let mut world = PhysicsWorld::new();
        world.gravity = vector(gravity);
        world.integration_parameters.dt = STEP;
        world.integration_parameters.length_unit = length_unit;
        Self { world, entities: HashMap::new(), joints: HashMap::new(), ground: None, removed: HashMap::new(), accumulator: 0.0, tracing: false, traces: Mutex::new(Vec::new()), waiting: HashMap::new(), mesh_failed: HashSet::new(), warnings: HashSet::new() }
    }

    pub fn gravity(&self) -> Vec3 {
        vec3(self.world.gravity)
    }

    pub fn set_gravity(&mut self, gravity: Vec3) {
        self.world.gravity = vector(gravity);
    }

    /// 刚体在上一步结束时的速度（没有 `Velocity` 组件的也有）；只有碰撞体的是 `None`。
    pub fn velocity(&self, entity: Entity) -> Option<Velocity> {
        self.entities.get(&entity).filter(|h| h.body.is_some()).map(|h| h.velocity)
    }

    /// 上一步结束时正在接触的碰撞体（交给求解器的接触点；sensor 的重叠不算）。
    pub fn contacts(&self, entity: Entity) -> Vec<Contact> {
        // 角色：走完这一帧之后贴着它的碰撞体（rapier 不算 kinematic 和固定、kinematic 之间的接触）
        if let Some(mover) = self.entities.get(&entity).and_then(|h| h.mover.as_ref()) {
            return mover.touching.clone();
        }
        let Some(collider) = self.entities.get(&entity).and_then(|h| h.collider) else { return Vec::new() };
        self.world
            .contact_pairs_with(collider)
            .filter(|pair| pair.has_any_active_contact())
            .filter_map(|pair| {
                let manifold = pair.solver_manifolds().iter().find(|m| !m.data.solver_contacts.is_empty())?;
                // rapier 的法线从 collider1 指向 collider2
                let (other, normal) = if pair.collider1 == collider { (pair.collider2, -manifold.data.normal) } else { (pair.collider1, manifold.data.normal) };
                Some(Contact { other: self.entity_of(other)?, normal: vec3(normal) })
            })
            .collect()
    }

    /// 从 `origin` 沿 `direction`（会归一化）最远走 `max_distance`，第一个碰到的碰撞体；起点在碰撞体里面时 `distance` 是 0、
    /// `normal` 是 `-direction`。查的是上一步结束时的世界。方向是 0 或不是有限数、`max_distance` 不是正数时是 `None`。
    pub fn raycast(&self, origin: Vec3, direction: Vec3, max_distance: f32, filter: QueryFilter) -> Option<RayHit> {
        let dir = direction.try_normalize()?;
        if !origin.is_finite() || max_distance.is_nan() || max_distance <= 0.0 {
            return None;
        }
        let ray = Ray::new(vector(origin), vector(dir));
        let found = self.world.cast_ray_and_get_normal(&ray, max_distance, true, self.filter(filter));
        let hit = found.and_then(|(collider, hit)| {
            // 起点在里面时 parry 给的法线是 0 或 NaN：统一成迎着射线
            let normal = vec3(hit.normal).try_normalize().filter(|_| hit.time_of_impact > 0.0).unwrap_or(-dir);
            Some(RayHit { entity: self.entity_of(collider)?, point: origin + dir * hit.time_of_impact, normal, distance: hit.time_of_impact })
        });
        let to = hit.map_or(origin + dir * max_distance.min(1e6), |h| h.point);
        self.trace(Trace { from: origin, to, shape: None, hit: hit.map(|h| (h.point, h.normal)) });
        hit
    }

    /// 调试视图开着时记下一次查询。
    pub(crate) fn trace(&self, trace: Trace) {
        if self.tracing {
            self.traces.lock().unwrap_or_else(|e| e.into_inner()).push(trace);
        }
    }

    /// （刚体数，碰撞体数）。连到世界上的关节用的固定刚体不算。
    pub fn len(&self) -> (usize, usize) {
        (self.world.bodies.len() - usize::from(self.ground.is_some()), self.world.colliders.len())
    }

    /// 还在等模型生成碰撞体的 `MeshCollider` 个数（生不出来的不算）。游戏可以等它变成 0 再放角色，免得地面还没有先掉下去。
    pub fn pending_meshes(&self) -> usize {
        self.waiting.len()
    }

    #[cfg(test)]
    pub(crate) fn warned(&self, entity: Entity) -> bool {
        self.warnings.iter().any(|(e, _)| *e == entity)
    }

    /// 物理里把这个实体当角色控制器（kinematic 刚体、有 `Character3d`）。
    pub fn is_character(&self, entity: Entity) -> bool {
        self.entities.get(&entity).is_some_and(|h| h.mover.is_some())
    }

    /// 这个关节实体已经连上了（两头都是刚体、参数没问题）。
    pub fn is_linked(&self, joint: Entity) -> bool {
        self.joints.get(&joint).is_some_and(|l| l.handle.is_some())
    }

    /// 建到物理里的关节数。
    pub fn joints(&self) -> usize {
        self.world.impulse_joints.len()
    }

    pub(crate) fn filter(&self, filter: QueryFilter) -> RapierFilter<'static> {
        let mut f = RapierFilter::default();
        if !filter.sensors {
            f = f.exclude_sensors();
        }
        f = f.groups(filter.groups());
        if let Some(handles) = filter.exclude.and_then(|e| self.entities.get(&e)) {
            if let Some(body) = handles.body {
                f = f.exclude_rigid_body(body);
            } else if let Some(collider) = handles.collider {
                f = f.exclude_collider(collider);
            }
        }
        f
    }

    pub(crate) fn entity_of(&self, collider: ColliderHandle) -> Option<Entity> {
        self.world.colliders.get(collider).map(|c| Entity::from_bits(c.user_data as u64))
    }

    fn forget(&mut self, entity: Entity) -> Option<Handles> {
        let handles = self.entities.remove(&entity)?;
        if let Some(c) = handles.collider {
            self.removed.insert(c, entity);
        }
        match handles.body {
            Some(body) => drop(self.world.remove_body_with_colliders(body, true)),
            None => drop(handles.collider.map(|c| self.world.remove_collider(c))),
        }
        Some(handles)
    }
}

// rapier 用 glam 0.33，引擎用 0.34：按数组转
pub(crate) fn vector(v: Vec3) -> Vector {
    Vector::from_array(v.to_array())
}

pub(crate) fn vec3(v: Vector) -> Vec3 {
    Vec3::from_array(v.to_array())
}

pub(crate) fn rotation(q: Quat) -> Rotation {
    // 游戏手写的四元数可能没归一化
    Rotation::from_array(q.normalize().to_array())
}

pub(crate) fn quat(r: Rotation) -> Quat {
    Quat::from_array(r.to_array())
}

pub(crate) fn pose(at: (Vec3, Quat)) -> Pose {
    Pose::from_parts(vector(at.0), rotation(at.1))
}

/// 实体删了，或者 `RigidBody` / `Collider` 都去掉了：删掉物理里的东西。只去掉一个的交给 `create` 重建。
pub(crate) fn remove(
    mut physics: ResMut<Physics3d>,
    mut bodies: RemovedComponents<RigidBody>,
    mut colliders: RemovedComponents<Collider>,
    left: Query<(Has<RigidBody>, Has<Collider>)>,
    mut rebuild: Commands,
) {
    let gone: Vec<Entity> = bodies.read().chain(colliders.read()).collect();
    for entity in gone {
        match left.get(entity) {
            Ok((false, false)) | Err(_) => {
                physics.forget(entity);
                physics.warnings.retain(|(e, _)| *e != entity);
            }
            // 剩下的那个组件标成变了，`create` 在这一帧接着重建（中间会应用 commands）
            Ok(_) => {
                rebuild.entity(entity).queue(|mut e: EntityWorldMut| {
                    if let Some(mut b) = e.get_mut::<RigidBody>() {
                        b.set_changed();
                    }
                    if let Some(mut c) = e.get_mut::<Collider>() {
                        c.set_changed();
                    }
                });
            }
        }
    }
}

type Changed3 = Or<(Changed<RigidBody>, Changed<Collider>)>;

/// 新加的刚体 / 碰撞体，或者种类、形状变了的：（重新）建到物理里。值没变的重新插入不动。
/// 有 `MeshCollider`、还没生成 `Collider` 的先不建（不然动态刚体没有碰撞体会先掉下去）。
pub(crate) fn create(mut physics: ResMut<Physics3d>, query: Query<(Entity, Option<&RigidBody>, Option<&Collider>, Option<&Transform3d>, Option<&Velocity>, Has<MeshCollider>), Changed3>) {
    let physics = &mut *physics;
    for (entity, kind, shape, transform, velocity, waiting) in &query {
        if waiting && shape.is_none() {
            continue;
        }
        let (kind, shape) = (kind.copied(), shape.cloned());
        if physics.entities.get(&entity).is_some_and(|h| h.kind == kind && h.shape == shape) {
            continue;
        }
        let transform = transform.copied().unwrap_or_default();
        let old = physics.forget(entity);
        // 重建时没有 Velocity 组件就沿用物理里原来的速度
        let velocity = velocity.copied().or(old.map(|h| h.velocity)).unwrap_or_default();
        // 碰撞体生不出来的刚体也不建：和等模型的网格碰撞体一样，不然没有碰撞体的动态刚体会一直往下掉
        if let Some(handles) = build(&mut physics.world, entity, kind, shape, (transform.translation, transform.rotation), velocity) {
            physics.entities.insert(entity, handles);
        }
    }
}

fn build(world: &mut PhysicsWorld, entity: Entity, kind: Option<RigidBody>, shape: Option<Collider>, at: (Vec3, Quat), velocity: Velocity) -> Option<Handles> {
    let data = u128::from(entity.to_bits());
    let builder = match &shape {
        Some(c) => Some(shape_builder(entity, &c.shape, matches!(kind, None | Some(RigidBody::Fixed)))?),
        None => None,
    };
    let collider = shape.as_ref().zip(builder).map(|(c, builder)| {
        let events = if c.events { ActiveEvents::COLLISION_EVENTS } else { ActiveEvents::empty() };
        builder
            .restitution(c.restitution)
            .friction(c.friction)
            .density(c.density.max(0.0))
            .sensor(c.sensor)
            .collision_groups(groups(c.group))
            .active_events(events)
            .user_data(data)
    });
    let body = kind.map(|kind| {
        let builder = match kind {
            RigidBody::Dynamic => RigidBodyBuilder::dynamic(),
            RigidBody::Fixed => RigidBodyBuilder::fixed(),
            RigidBody::Kinematic => RigidBodyBuilder::kinematic_position_based(),
        };
        world.insert_body(builder.pose(pose(at)).linvel(vector(velocity.linear)).angvel(vector(velocity.angular)).user_data(data))
    });
    let collider = collider.map(|c| match body {
        Some(body) => world.insert_collider(c, Some(body)),
        None => world.insert_collider(c.position(pose(at)), None),
    });
    // 质量为 0 的动态刚体不受重力、冲量也没用：补上 1 的质量
    if let Some(body) = body.and_then(|b| world.bodies.get_mut(b))
        && body.is_dynamic()
        && body.mass() <= 0.0
    {
        body.set_additional_mass(1.0, true);
    }
    Some(Handles { body, collider, kind, shape, transform: at, velocity, mover: None })
}

/// 碰撞分组换成 rapier 的掩码：组 g 属于第 g 位、不和第 g 位碰；不分组（0、超出范围）的属于第 0 位、和谁都碰。
/// 碰撞结果和 2D 一样（2D 不分组的属于全部位），好处是查询能按第 0 位挑出不分组的。
pub(crate) fn groups(group: u8) -> InteractionGroups {
    if !(1..=31).contains(&group) {
        return InteractionGroups::new(Group::GROUP_1, Group::ALL, InteractionTestMode::And);
    }
    let bit = Group::from_bits_truncate(1 << group);
    InteractionGroups::new(bit, bit.complement(), InteractionTestMode::And)
}

/// 形状换成 rapier 的。网格类的生不出来（点太少、共面）时 warn，返回 `None`；三角网格不是固定的时换成凸包。
fn shape_builder(entity: Entity, shape: &Shape, fixed: bool) -> Option<ColliderBuilder> {
    let points = |p: &[Vec3]| p.iter().map(|&v| vector(v)).collect::<Vec<Vector>>();
    let built = match shape {
        Shape::Box(size) => {
            let h = size.abs() / 2.0;
            Some(ColliderBuilder::cuboid(h.x, h.y, h.z))
        }
        Shape::Ball(r) => Some(ColliderBuilder::ball(r.abs())),
        Shape::Capsule { half_height, radius } => Some(ColliderBuilder::capsule_y(half_height.abs(), radius.abs())),
        Shape::Convex(p) => solid_hull(p).map(ColliderBuilder::new),
        // 生不出来的部件跳过，全都生不出来才算
        Shape::Hulls(parts) => {
            let shapes: Vec<_> = parts.iter().filter_map(|p| solid_hull(p).map(|s| (Pose::IDENTITY, s))).collect();
            (!shapes.is_empty()).then(|| ColliderBuilder::compound(shapes))
        }
        // 下标越界 parry 会 panic：先查
        Shape::TriMesh(mesh) if mesh.1.iter().flatten().any(|&i| i as usize >= mesh.0.len()) => None,
        Shape::TriMesh(mesh) if fixed => ColliderBuilder::trimesh(points(&mesh.0), mesh.1.clone()).ok(),
        Shape::TriMesh(mesh) => {
            log::warn!("实体 {entity:?} 的三角网格碰撞体只能用在固定的上，换成凸包");
            solid_hull(&mesh.0).map(ColliderBuilder::new)
        }
    };
    if built.is_none() {
        log::warn!("实体 {entity:?} 的碰撞体生不出来（点太少、都在一个平面上，或三角形的下标越界），刚体也不建");
    }
    built
}

/// 收集 rapier 的碰撞事件。
#[derive(Default)]
struct Events(Mutex<Vec<CollisionEvent>>);

impl EventHandler for Events {
    fn handle_collision_event(&self, _: &RigidBodySet, _: &ColliderSet, event: CollisionEvent, _: Option<&ContactPair>) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(event);
    }

    fn handle_contact_force_event(&self, _: f32, _: &RigidBodySet, _: &ColliderSet, _: &ContactPair, _: f32) {}

    fn handle_soft_body_tear_event(&self, _: &SoftBodySet, _: &SoftBodyTearEvent) {}
}

type Synced<'a> = (Entity, &'a mut Transform3d, Option<&'a mut Velocity>, Option<&'a mut Impulse>, Option<&'a mut CharacterState3d>);

/// 按累计的时间走若干个固定步长：先把游戏改过的位置 / 速度 / 冲量推给物理，走完再写回。
pub(crate) fn step(
    mut physics: ResMut<Physics3d>,
    time: Res<Time>,
    mut query: Query<Synced>,
    mut collisions: MessageWriter<Collision3d>,
    characters: Query<&Character3d>,
) {
    let physics = &mut *physics;
    // 角色控制器的参数跟着组件走；去掉了组件就不再是角色
    for (entity, h) in physics.entities.iter_mut() {
        match (characters.get(*entity), h.mover.as_mut()) {
            (Ok(&c), Some(m)) => m.params = c,
            (Ok(&c), None) if h.kind == Some(RigidBody::Kinematic) => h.mover = Some(Mover { params: c, ..Default::default() }),
            (Ok(_), None) => {
                if physics.warnings.insert((*entity, "character")) {
                    log::warn!("实体 {entity:?} 的 Character3d 要和 RigidBody::Kinematic 一起用，这个刚体是 {:?}，不当角色", h.kind);
                }
            }
            (Err(_), _) => h.mover = None,
        }
        // 角色碰到固定、kinematic 的 sensor 也要发碰撞事件：rapier 默认只算有动态刚体的一对
        let types = if h.mover.is_some() { ActiveCollisionTypes::all() } else { ActiveCollisionTypes::default() };
        if let Some(c) = h.collider.and_then(|c| physics.world.colliders.get_mut(c))
            && c.active_collision_types() != types
        {
            c.set_active_collision_types(types);
        }
    }
    if time.delta.is_finite() {
        physics.accumulator += time.delta.max(0.0);
    }
    let steps = ((physics.accumulator / STEP) as u32).min(MAX_STEPS);
    if steps == 0 {
        return;
    }
    physics.accumulator = (physics.accumulator - steps as f32 * STEP).min(STEP);

    let mut kinematic = Vec::new();
    for (entity, transform, velocity, impulse, _) in &mut query {
        let Some(h) = physics.entities.get_mut(&entity) else { continue };
        kinematic.extend(push(&mut physics.world, h, &transform, velocity.as_deref(), impulse));
        // 角色这一帧从这里开始算实际速度（瞬移不算）
        if let Some(m) = h.mover.as_mut() {
            (m.start, m.hits) = (transform.translation, Vec::new());
        }
    }
    let events = Events::default();
    for i in 1..=steps {
        // kinematic 的位移平分到这一帧的每一步
        let f = i as f32 / steps as f32;
        for &(body, from, to) in &kinematic {
            if let Some(body) = physics.world.bodies.get_mut(body) {
                body.set_next_kinematic_position(pose((from.0.lerp(to.0, f), from.1.slerp(to.1, f))));
            }
        }
        character::advance(physics, STEP);
        physics.world.step_with_events(&(), &events);
    }
    character::touch(physics);
    for (entity, mut transform, velocity, _, state) in &mut query {
        let Some(h) = physics.entities.get(&entity) else { continue };
        let Some(body) = h.body.and_then(|b| physics.world.bodies.get(b)) else { continue };
        let at = (vec3(body.translation()), quat(*body.rotation()));
        // 角色的速度是控制器算的，kinematic 刚体的速度里还带着脚下平台的
        let velocity_now = if h.mover.is_some() { h.velocity } else { Velocity { linear: vec3(body.linvel()), angular: vec3(body.angvel()) } };
        if let (Some(mut state), Some(m)) = (state, h.mover.as_ref()) {
            *state = character::report(physics, m, at.0, steps as f32 * STEP);
        }
        let h = physics.entities.get_mut(&entity).expect("刚查过");
        // 绕过变更检测：下一帧只把游戏自己改的当成改动
        let t = transform.bypass_change_detection();
        (t.translation, t.rotation) = at;
        h.transform = at;
        h.velocity = velocity_now;
        if let Some(mut v) = velocity {
            *v.bypass_change_detection() = h.velocity;
        }
    }
    let (colliders, removed) = (&physics.world.colliders, &physics.removed);
    let entity = |c: ColliderHandle| colliders.get(c).map(|c| Entity::from_bits(c.user_data as u64)).or(removed.get(&c).copied());
    for event in events.0.into_inner().unwrap_or_else(|e| e.into_inner()) {
        let (a, b, started) = match event {
            CollisionEvent::Started(a, b, _) => (a, b, true),
            CollisionEvent::Stopped(a, b, _) => (a, b, false),
        };
        if let (Some(a), Some(b)) = (entity(a), entity(b)) {
            collisions.write(Collision3d { a, b, started });
        }
    }
    physics.removed.clear();
}

/// 把游戏改过的位置、速度、冲量推给物理。返回要在这一帧里移动的 kinematic 刚体：（刚体，起点，终点）。
fn push(world: &mut PhysicsWorld, h: &mut Handles, transform: &Transform3d, velocity: Option<&Velocity>, impulse: Option<Mut<Impulse>>) -> Option<(RigidBodyHandle, (Vec3, Quat), (Vec3, Quat))> {
    let (from, at) = (h.transform, (transform.translation, transform.rotation));
    let moved = at != from;
    h.transform = at;
    let Some(body) = h.body.and_then(|b| world.bodies.get_mut(b)) else {
        if moved && let Some(c) = h.collider.and_then(|c| world.colliders.get_mut(c)) {
            c.set_position(pose(at));
        }
        return None;
    };
    let mut kinematic = None;
    if moved {
        if body.is_kinematic() && h.mover.is_none() {
            kinematic = h.body.map(|b| (b, from, at));
        } else {
            body.set_position(pose(at), true);
            // 角色瞬移：不再跟着原来脚下的东西走
            if let Some(m) = h.mover.as_mut() {
                (m.ground, m.grounded, m.ground_normal) = (None, false, Vec3::ZERO);
            }
        }
    }
    // 只有动态刚体的速度由游戏推：kinematic 的速度是 rapier 按位移算的；角色的速度给控制器用
    if let Some(&v) = velocity
        && v != h.velocity
        && (body.is_dynamic() || h.mover.is_some())
    {
        body.set_linvel(vector(v.linear), true);
        body.set_angvel(vector(v.angular), true);
        h.velocity = v;
    }
    if let Some(mut impulse) = impulse
        && impulse.0 != Vec3::ZERO
    {
        body.apply_impulse(vector(impulse.0), true);
        impulse.0 = Vec3::ZERO;
    }
    kinematic
}
