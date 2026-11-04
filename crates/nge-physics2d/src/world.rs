use std::collections::HashMap;
use std::sync::Mutex;

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_ecs::Time;
use nge_render2d::{DrawAt, Transform2d};
use rapier2d::math::{Pose, Rotation, Vector};
use rapier2d::prelude::{
    ActiveEvents, ColliderBuilder, ColliderHandle, ColliderSet, CollisionEvent, ContactPair, EventHandler, PhysicsWorld, RigidBodyBuilder,
    RigidBodyHandle, RigidBodySet, SoftBodySet, SoftBodyTearEvent,
};
use rapier2d::prelude::{Cuboid, QueryFilter as RapierFilter, Ray};

use crate::{Collider, Collision, Contact, Impulse, LockRotation, MAX_STEPS, QueryFilter, RayHit, RigidBody, STEP, Shape, Velocity};

/// rapier 的世界，加上实体和刚体 / 碰撞体的对应关系。
#[derive(Resource)]
pub struct Physics {
    world: PhysicsWorld,
    entities: HashMap<Entity, Handles>,
    /// 刚删掉的碰撞体：rapier 下一步才发它们的停止接触，那时已经查不到 user_data 了。
    removed: HashMap<ColliderHandle, Entity>,
    accumulator: f32,
}

/// 一个实体在物理里的东西，以及上一次和组件同步时的值（用来判断游戏改没改）。
struct Handles {
    body: Option<RigidBodyHandle>,
    collider: Option<ColliderHandle>,
    kind: Option<RigidBody>,
    shape: Option<Collider>,
    locked: bool,
    transform: (Vec2, f32),
    velocity: Velocity,
    /// 动态 / kinematic 刚体在上一步、这一步结束时的位置（角度不折回），`DrawAt` 在两者之间插值。
    prev: (Vec2, f32),
    curr: (Vec2, f32),
}

impl Physics {
    pub(crate) fn new(gravity: Vec2, pixels_per_meter: f32) -> Self {
        let mut world = PhysicsWorld::new();
        world.gravity = vector(gravity);
        world.integration_parameters.dt = STEP;
        world.integration_parameters.length_unit = pixels_per_meter;
        Self { world, entities: HashMap::new(), removed: HashMap::new(), accumulator: 0.0 }
    }

    pub fn gravity(&self) -> Vec2 {
        vec2(self.world.gravity)
    }

    pub fn set_gravity(&mut self, gravity: Vec2) {
        self.world.gravity = vector(gravity);
    }

    /// 刚体的质量（由碰撞体的密度和面积算出来；没有碰撞体或算出来是 0 的动态刚体按 1）。
    pub fn mass(&self, entity: Entity) -> Option<f32> {
        let body = self.entities.get(&entity)?.body?;
        Some(self.world.bodies.get(body)?.mass())
    }

    /// 刚体在上一步结束时的速度（没有 `Velocity` 组件的刚体也有，比如被游戏挪的 kinematic）；只有碰撞体的是 `None`。
    pub fn velocity(&self, entity: Entity) -> Option<Velocity> {
        self.entities.get(&entity).filter(|h| h.body.is_some()).map(|h| h.velocity)
    }

    /// 上一步结束时正在接触的碰撞体（交给求解器的接触点，含很近还没碰上的预测点；sensor 的重叠不算）。
    /// 实体还没建到物理里时是空的。`other` 是碰撞体所在的实体（瓦片地图是它下面的子实体）。
    pub fn contacts(&self, entity: Entity) -> Vec<Contact> {
        let Some(collider) = self.entities.get(&entity).and_then(|h| h.collider) else { return Vec::new() };
        self.world
            .contact_pairs_with(collider)
            .filter(|pair| pair.has_any_active_contact())
            .filter_map(|pair| {
                // 和 has_any_active_contact 看同一组：交给求解器的接触点（含预测距离以内的）
                let manifold = pair.solver_manifolds().iter().find(|m| !m.data.solver_contacts.is_empty())?;
                // rapier 的法线从 collider1 指向 collider2
                let (other, normal) = if pair.collider1 == collider { (pair.collider2, -manifold.data.normal) } else { (pair.collider1, manifold.data.normal) };
                Some(Contact { other: self.entity_of(other)?, normal: vec2(normal) })
            })
            .collect()
    }

    /// 从 `origin` 沿 `direction`（会归一化）最远走 `max_distance`，第一个碰到的碰撞体；起点在碰撞体里面时 `distance` 是 0、
    /// `normal` 是 `-direction`。查的是上一步结束时的世界（见 docs/physics2d.md）。方向是 0 或不是有限数、`max_distance` 不是正数时是 `None`。
    pub fn raycast(&self, origin: Vec2, direction: Vec2, max_distance: f32, filter: QueryFilter) -> Option<RayHit> {
        let dir = direction.try_normalize()?;
        if !origin.is_finite() || max_distance.is_nan() || max_distance <= 0.0 {
            return None;
        }
        let ray = Ray::new(vector(origin), vector(dir));
        let (collider, hit) = self.world.cast_ray_and_get_normal(&ray, max_distance, true, self.filter(filter))?;
        // 起点在里面时 parry 给的法线是 0（矩形）或 NaN（圆心）：统一成迎着射线
        let normal = vec2(hit.normal).try_normalize().filter(|_| hit.time_of_impact > 0.0).unwrap_or(-dir);
        Some(RayHit { entity: self.entity_of(collider)?, point: origin + dir * hit.time_of_impact, normal, distance: hit.time_of_impact })
    }

    /// 包含这个点的碰撞体（上一步结束时的世界）。
    pub fn overlap_point(&self, point: Vec2, filter: QueryFilter) -> Vec<Entity> {
        if !point.is_finite() {
            return Vec::new();
        }
        self.world.intersect_point(vector(point), self.filter(filter)).filter_map(|(c, _)| self.entity_of(c)).collect()
    }

    /// 和这个不转的矩形（`half_size` 是半宽半高，取绝对值）重叠的碰撞体（上一步结束时的世界）。
    pub fn overlap_box(&self, center: Vec2, half_size: Vec2, filter: QueryFilter) -> Vec<Entity> {
        let half_size = half_size.abs();
        if !center.is_finite() || !half_size.is_finite() || half_size.min_element() <= 0.0 {
            return Vec::new();
        }
        let shape = Cuboid::new(vector(half_size));
        self.world.intersect_shape(Pose::from_translation(vector(center)), &shape, self.filter(filter)).filter_map(|(c, _)| self.entity_of(c)).collect()
    }

    /// （刚体数，碰撞体数）。
    pub fn len(&self) -> (usize, usize) {
        (self.world.bodies.len(), self.world.colliders.len())
    }

    fn filter(&self, filter: QueryFilter) -> RapierFilter<'static> {
        let mut f = RapierFilter::default();
        if !filter.sensors {
            f = f.exclude_sensors();
        }
        if let Some(handles) = filter.exclude.and_then(|e| self.entities.get(&e)) {
            if let Some(body) = handles.body {
                f = f.exclude_rigid_body(body);
            } else if let Some(collider) = handles.collider {
                f = f.exclude_collider(collider);
            }
        }
        f
    }

    fn entity_of(&self, collider: ColliderHandle) -> Option<Entity> {
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

fn vector(v: Vec2) -> Vector {
    Vector::new(v.x, v.y)
}

fn vec2(v: Vector) -> Vec2 {
    Vec2::new(v.x, v.y)
}

/// 实体删了，或者 `RigidBody` / `Collider` 都去掉了：删掉物理里的东西。只去掉一个的交给 `create` 重建。
pub(crate) fn remove(
    mut physics: ResMut<Physics>,
    mut bodies: RemovedComponents<RigidBody>,
    mut colliders: RemovedComponents<Collider>,
    left: Query<(Has<RigidBody>, Has<Collider>)>,
    mut unlocked: RemovedComponents<LockRotation>,
    mut rebuild: Commands,
) {
    // 去掉了锁：标成变了，`create` 里解锁
    for entity in unlocked.read() {
        if let Ok(mut e) = rebuild.get_entity(entity) {
            e.queue(|mut e: EntityWorldMut| {
                if let Some(mut b) = e.get_mut::<RigidBody>() {
                    b.set_changed();
                }
            });
        }
    }
    let bodies: Vec<_> = bodies.read().collect();
    for &entity in &bodies {
        if let Ok(mut e) = rebuild.get_entity(entity) {
            e.try_remove::<DrawAt>();
        }
    }
    for entity in bodies.into_iter().chain(colliders.read()) {
        match left.get(entity) {
            Ok((false, false)) | Err(_) => drop(physics.forget(entity)),
            // 剩下的那个组件标成变了，`create` 下一帧重建
            Ok(_) => {
                if let Some(h) = physics.entities.get_mut(&entity) {
                    h.kind = None;
                    h.shape = None;
                }
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

type Changed2 = Or<(Changed<RigidBody>, Changed<Collider>, Changed<LockRotation>)>;

/// 新加的刚体 / 碰撞体，或者种类、形状变了的：（重新）建到物理里。值没变的重新插入不动。
pub(crate) fn create(
    mut physics: ResMut<Physics>,
    query: Query<(Entity, Option<&RigidBody>, Option<&Collider>, Option<&Transform2d>, Option<&Velocity>, Has<LockRotation>), Changed2>,
) {
    let physics = &mut *physics;
    for (entity, kind, shape, transform, velocity, locked) in &query {
        let (kind, shape) = (kind.copied(), shape.copied());
        // 只挂了 LockRotation、还没有刚体和碰撞体的实体不建
        if kind.is_none() && shape.is_none() && !physics.entities.contains_key(&entity) {
            continue;
        }
        if !physics.entities.get(&entity).is_some_and(|h| h.kind == kind && h.shape == shape) {
            let transform = transform.copied().unwrap_or_default();
            let old = physics.forget(entity);
            // 重建时没有 Velocity 组件就沿用物理里原来的速度
            let velocity = velocity.copied().or(old.as_ref().map(|h| h.velocity)).unwrap_or_default();
            let at = (transform.translation, transform.rotation);
            let mut handles = build(&mut physics.world, entity, kind, shape, at, velocity);
            // 只换了形状、游戏也没挪它：画面接着原来的插值走，不往前跳一步
            if let Some(old) = old.filter(|h| h.kind == kind && h.transform == at) {
                (handles.prev, handles.curr) = (old.prev, old.curr);
            }
            physics.entities.insert(entity, handles);
        }
        let Some(h) = physics.entities.get_mut(&entity) else { continue };
        if h.locked != locked {
            if let Some(body) = h.body.and_then(|b| physics.world.bodies.get_mut(b)) {
                body.lock_rotations(locked, true);
                // rapier 锁旋转只把转动惯量当成无穷大，不管已有的角速度：一起停下
                if locked {
                    body.set_angvel(0.0, true);
                }
            }
            h.locked = locked;
        }
    }
}

fn build(world: &mut PhysicsWorld, entity: Entity, kind: Option<RigidBody>, shape: Option<Collider>, at: (Vec2, f32), velocity: Velocity) -> Handles {
    let data = u128::from(entity.to_bits());
    let collider = shape.map(|c| {
        let builder = match c.shape {
            Shape::Rect(size) => ColliderBuilder::cuboid(size.x.abs() / 2.0, size.y.abs() / 2.0),
            Shape::Circle(r) => ColliderBuilder::ball(r.abs()),
        };
        let events = if c.events { ActiveEvents::COLLISION_EVENTS } else { ActiveEvents::empty() };
        builder
            .restitution(c.restitution)
            .friction(c.friction)
            .density(c.density.max(0.0))
            .sensor(c.sensor)
            .active_events(events)
            .user_data(data)
    });
    let body = kind.map(|kind| {
        let builder = match kind {
            RigidBody::Dynamic => RigidBodyBuilder::dynamic(),
            RigidBody::Fixed => RigidBodyBuilder::fixed(),
            RigidBody::Kinematic => RigidBodyBuilder::kinematic_position_based(),
        };
        let body = builder
            .translation(vector(at.0))
            .rotation(at.1)
            .linvel(vector(velocity.linear))
            .angvel(velocity.angular)
            .user_data(data);
        world.insert_body(body)
    });
    let collider = collider.map(|c| match body {
        Some(body) => world.insert_collider(c, Some(body)),
        None => world.insert_collider(c.translation(vector(at.0)).rotation(at.1), None),
    });
    // 质量为 0 的动态刚体不受重力、冲量也没用：补上 1 的质量
    if let Some(body) = body.and_then(|b| world.bodies.get_mut(b))
        && body.is_dynamic()
        && body.mass() <= 0.0
    {
        body.set_additional_mass(1.0, true);
    }
    Handles { body, collider, kind, shape, locked: false, transform: at, velocity, prev: at, curr: at }
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

type Synced<'a> = (Entity, &'a mut Transform2d, Option<&'a mut Velocity>, Option<&'a mut Impulse>, Option<&'a mut DrawAt>);

/// 按累计的时间走若干个固定步长：先把游戏改过的位置 / 速度 / 冲量推给物理，走完再写回。
/// 每帧（包括不够一步的帧）给有刚体的实体写 `DrawAt`：动态 / kinematic 刚体在上一步和这一步之间按剩下的时间插值（比物理慢一步），
/// 刷新率比步长高或帧时间抖动时画面也每帧都动。
pub(crate) fn step(
    mut physics: ResMut<Physics>,
    time: Res<Time>,
    mut query: Query<Synced>,
    mut collisions: MessageWriter<Collision>,
    mut commands: Commands,
) {
    let physics = &mut *physics;
    if time.delta.is_finite() {
        physics.accumulator += time.delta.max(0.0);
    }
    let steps = ((physics.accumulator / STEP) as u32).min(MAX_STEPS);
    if steps == 0 {
        draw(physics, &mut query, &mut commands);
        return;
    }
    physics.accumulator = (physics.accumulator - steps as f32 * STEP).min(STEP);

    let mut kinematic = Vec::new();
    for (entity, transform, velocity, impulse, _) in &mut query {
        let Some(h) = physics.entities.get_mut(&entity) else { continue };
        kinematic.extend(push(&mut physics.world, h, &transform, velocity.as_deref(), impulse));
    }
    let events = Events::default();
    for i in 1..=steps {
        // kinematic 的位移平分到这一帧的每一步
        let f = i as f32 / steps as f32;
        for &(body, from, to) in &kinematic {
            if let Some(body) = physics.world.bodies.get_mut(body) {
                body.set_next_kinematic_translation(vector(from.0.lerp(to.0, f)));
                body.set_next_kinematic_rotation(Rotation::new(from.1 + (to.1 - from.1) * f));
            }
        }
        if i == steps {
            // 最后一步之前的位置就是插值的起点
            for h in physics.entities.values_mut() {
                if let Some(body) = moving_body(&physics.world, h) {
                    h.prev = pose(body, h.curr.1);
                }
            }
        }
        physics.world.step_with_events(&(), &events);
    }
    for (entity, mut transform, velocity, ..) in &mut query {
        let Some(h) = physics.entities.get_mut(&entity) else { continue };
        let Some(body) = h.body.and_then(|b| physics.world.bodies.get(b)) else { continue };
        // 绕过变更检测：下一步只把游戏自己改的当成改动
        let t = transform.bypass_change_detection();
        (t.translation, t.rotation) = pose(body, t.rotation);
        h.transform = (t.translation, t.rotation);
        if moving_body(&physics.world, h).is_some() {
            h.curr = h.transform;
        }
        h.velocity = Velocity { linear: vec2(body.linvel()), angular: body.angvel() };
        if let Some(mut v) = velocity {
            *v.bypass_change_detection() = h.velocity;
        }
    }
    draw(physics, &mut query, &mut commands);
    let (colliders, removed) = (&physics.world.colliders, &physics.removed);
    let entity = |c: ColliderHandle| colliders.get(c).map(|c| Entity::from_bits(c.user_data as u64)).or(removed.get(&c).copied());
    for event in events.0.into_inner().unwrap_or_else(|e| e.into_inner()) {
        let (a, b, started) = match event {
            CollisionEvent::Started(a, b, _) => (a, b, true),
            CollisionEvent::Stopped(a, b, _) => (a, b, false),
        };
        if let (Some(a), Some(b)) = (entity(a), entity(b)) {
            collisions.write(Collision { a, b, started });
        }
    }
    physics.removed.clear();
}

/// 动态或 kinematic 刚体（画面要插值的）。
fn moving_body<'a>(world: &'a PhysicsWorld, h: &Handles) -> Option<&'a rapier2d::prelude::RigidBody> {
    h.body.and_then(|b| world.bodies.get(b)).filter(|_| matches!(h.kind, Some(RigidBody::Dynamic | RigidBody::Kinematic)))
}

/// 给有刚体的实体写 `DrawAt`（没有就插上）：动态 / kinematic 按攒下的时间插值，固定刚体画在 `Transform2d` 上。
fn draw(physics: &Physics, query: &mut Query<Synced>, commands: &mut Commands) {
    let alpha = physics.accumulator / STEP;
    for (entity, transform, _, _, draw_at) in query.iter_mut() {
        let Some(h) = physics.entities.get(&entity).filter(|h| h.body.is_some()) else { continue };
        let at = (transform.translation, transform.rotation);
        // 游戏在不够一步的帧里挪了动态刚体（比如每帧拖着走）：直接画在新位置，不等下一步。
        // kinematic 不这样：它的 Transform2d 是目标，画上去会和站在上面的东西差一步
        let dragged = h.kind == Some(RigidBody::Dynamic) && at != h.transform;
        let (translation, rotation) = if moving_body(&physics.world, h).is_some() && !dragged { lerp(h.prev, h.curr, alpha) } else { at };
        let new = DrawAt { translation, rotation };
        match draw_at {
            Some(mut d) => {
                if *d != new {
                    *d = new;
                }
            }
            None => {
                commands.entity(entity).try_insert(new);
            }
        }
    }
}

/// 刚体的位置；rapier 的角度在 (-π, π]，按和 `near` 的差值累加，不让游戏看到跳变。
fn pose(body: &rapier2d::prelude::RigidBody, near: f32) -> (Vec2, f32) {
    (vec2(body.translation()), near + wrap(body.rotation().angle() - near))
}

fn lerp(a: (Vec2, f32), b: (Vec2, f32), t: f32) -> (Vec2, f32) {
    let t = t.clamp(0.0, 1.0);
    (a.0.lerp(b.0, t), a.1 + (b.1 - a.1) * t)
}

/// 角度差规范到 (-π, π]。
fn wrap(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    a - ((a + PI) / TAU).floor() * TAU
}

/// 返回要在这一帧里移动的 kinematic 刚体：（刚体，起点，终点）。
fn push(
    world: &mut PhysicsWorld,
    h: &mut Handles,
    transform: &Transform2d,
    velocity: Option<&Velocity>,
    impulse: Option<Mut<Impulse>>,
) -> Option<(RigidBodyHandle, (Vec2, f32), (Vec2, f32))> {
    let at = (transform.translation, transform.rotation);
    let from = h.transform;
    let moved = at != from;
    h.transform = at;
    let Some(body) = h.body.and_then(|b| world.bodies.get_mut(b)) else {
        if moved && let Some(c) = h.collider.and_then(|c| world.colliders.get_mut(c)) {
            c.set_translation(vector(at.0));
            c.set_rotation(Rotation::new(at.1));
        }
        return None;
    };
    let mut kinematic = None;
    if moved {
        if body.is_kinematic() {
            kinematic = h.body.map(|b| (b, from, at));
        } else {
            body.set_translation(vector(at.0), true);
            body.set_rotation(Rotation::new(at.1), true);
            // 瞬移：插值的两头都换成新位置，画面不从旧位置滑过去
            (h.prev, h.curr) = (at, at);
        }
    }
    if let Some(&v) = velocity
        && v != h.velocity
    {
        body.set_linvel(vector(v.linear), true);
        body.set_angvel(v.angular, true);
        h.velocity = v;
    }
    if let Some(mut impulse) = impulse
        && impulse.0 != Vec2::ZERO
    {
        body.apply_impulse(vector(impulse.0), true);
        impulse.0 = Vec2::ZERO;
    }
    kinematic
}
