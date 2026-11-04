use std::collections::HashMap;
use std::sync::Mutex;

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_ecs::Time;
use nge_render2d::Transform2d;
use rapier2d::math::{Rotation, Vector};
use rapier2d::prelude::{
    ActiveEvents, ColliderBuilder, ColliderHandle, ColliderSet, CollisionEvent, ContactPair, EventHandler, PhysicsWorld, RigidBodyBuilder,
    RigidBodyHandle, RigidBodySet, SoftBodySet, SoftBodyTearEvent,
};

use crate::{Collider, Collision, Impulse, MAX_STEPS, RigidBody, STEP, Shape, Velocity};

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
    transform: (Vec2, f32),
    velocity: Velocity,
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

    /// （刚体数，碰撞体数）。
    pub fn len(&self) -> (usize, usize) {
        (self.world.bodies.len(), self.world.colliders.len())
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
    mut rebuild: Commands,
) {
    for entity in bodies.read().chain(colliders.read()) {
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

type Changed2 = Or<(Changed<RigidBody>, Changed<Collider>)>;

/// 新加的刚体 / 碰撞体，或者种类、形状变了的：（重新）建到物理里。值没变的重新插入不动。
pub(crate) fn create(
    mut physics: ResMut<Physics>,
    query: Query<(Entity, Option<&RigidBody>, Option<&Collider>, Option<&Transform2d>, Option<&Velocity>), Changed2>,
) {
    for (entity, kind, shape, transform, velocity) in &query {
        let (kind, shape) = (kind.copied(), shape.copied());
        if physics.entities.get(&entity).is_some_and(|h| h.kind == kind && h.shape == shape) {
            continue;
        }
        let transform = transform.copied().unwrap_or_default();
        let old = physics.forget(entity);
        // 重建时没有 Velocity 组件就沿用物理里原来的速度
        let velocity = velocity.copied().or(old.map(|h| h.velocity)).unwrap_or_default();
        let handles = build(&mut physics.world, entity, kind, shape, (transform.translation, transform.rotation), velocity);
        physics.entities.insert(entity, handles);
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
    Handles { body, collider, kind, shape, transform: at, velocity }
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

type Synced<'a> = (Entity, &'a mut Transform2d, Option<&'a mut Velocity>, Option<&'a mut Impulse>);

/// 按累计的时间走若干个固定步长：先把游戏改过的位置 / 速度 / 冲量推给物理，走完再写回。
pub(crate) fn step(mut physics: ResMut<Physics>, time: Res<Time>, mut query: Query<Synced>, mut collisions: MessageWriter<Collision>) {
    let physics = &mut *physics;
    if time.delta.is_finite() {
        physics.accumulator += time.delta.max(0.0);
    }
    let steps = ((physics.accumulator / STEP) as u32).min(MAX_STEPS);
    if steps == 0 {
        return;
    }
    physics.accumulator = (physics.accumulator - steps as f32 * STEP).min(STEP);

    let mut kinematic = Vec::new();
    for (entity, transform, velocity, impulse) in &mut query {
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
        physics.world.step_with_events(&(), &events);
    }
    for (entity, mut transform, velocity, _) in &mut query {
        let Some(h) = physics.entities.get_mut(&entity) else { continue };
        let Some(body) = h.body.and_then(|b| physics.world.bodies.get(b)) else { continue };
        // 绕过变更检测：下一步只把游戏自己改的当成改动
        let t = transform.bypass_change_detection();
        t.translation = vec2(body.translation());
        // rapier 的角度在 (-π, π]，按差值累加，不让游戏看到跳变
        t.rotation += wrap(body.rotation().angle() - t.rotation);
        h.transform = (t.translation, t.rotation);
        h.velocity = Velocity { linear: vec2(body.linvel()), angular: body.angvel() };
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
            collisions.write(Collision { a, b, started });
        }
    }
    physics.removed.clear();
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
