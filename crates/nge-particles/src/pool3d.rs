//! 3D 粒子池的模拟（见 docs/particles3d-more.md）：一个发射器一个池子，每个子发射器在下面再挂一个池子。
//! 每帧每个池子走一遍：先模拟自己的粒子（死亡、碰撞事件），再发新粒子（出生事件），再把事件交给子池子。

use std::sync::atomic::{AtomicBool, Ordering};

use glam::{Quat, Vec3};
use nge_assets::ImageId;
use nge_render3d::{Billboard, Blend, FULL_UV, Facing};

use crate::collide3d::{self, Raycast};
use crate::emitter3d::{Particle3d, ParticleEmitter3d, ParticleFacing, SubEmitter3d, SubTrigger};
use crate::noise::{curl, scroll};
use crate::rng::Rng;
use crate::shape3d::{self, EmitShape, MeshSurfaces};

/// 子发射器最多套几层；更深的不模拟。
pub const MAX_SUB_DEPTH: usize = 3;

/// 粒子的一件事：子发射器在这儿 burst。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Event {
    pub on: SubTrigger,
    pub position: Vec3,
    pub velocity: Vec3,
    /// 碰撞时是碰到的面的法线，别的是 0。
    pub normal: Vec3,
}

/// 这一帧所有发射器共用的上限。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Budget {
    /// 还能发多少个粒子。
    pub remaining: usize,
    /// 因为总数上限没发的（单个发射器到了 `max` 的不算）。
    pub dropped: usize,
    /// 这一帧最多查几次场景。
    pub queries: u32,
    /// 这一帧开头要查场景的粒子数（所有池子加起来），查询次数按它分。
    pub load: usize,
    /// 查了几次。
    pub queried: u32,
}

pub(crate) struct Frame<'a> {
    pub dt: f32,
    /// 噪声滚动用的时间（秒）。
    pub time: f64,
    pub rng: &'a mut Rng,
    pub budget: &'a mut Budget,
    pub world: Option<&'a dyn Raycast>,
    pub surfaces: &'a MeshSurfaces,
}

/// 新粒子从哪来。
pub(crate) enum Source<'a> {
    /// 发射器自己：按 `burst`、`rate` 发。
    Root { origin: Vec3, rotation: Quat },
    /// 子发射器：父池子这一帧的事件里对得上的，每个 burst `count` 个。
    Sub { events: &'a [Event], sub: &'a SubEmitter3d },
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Pool {
    pub particles: Vec<Particle3d>,
    pending: f32,
    /// 发过 burst 了：网格还没加载时不算，一次性的发射器也就不会被当成放完删掉。
    pub burst_done: bool,
    warned: bool,
    /// 这一帧的事件，交给子池子；数组留着复用。
    pub events: Vec<Event>,
    pub children: Vec<Pool>,
    /// 场景查询轮到第几个粒子。
    cursor: usize,
}

static WARNED_DEPTH: AtomicBool = AtomicBool::new(false);
static WARNED_BUDGET: AtomicBool = AtomicBool::new(false);

impl Pool {
    /// 包括子池子的粒子数。
    pub fn total(&self) -> usize {
        self.particles.len() + self.children.iter().map(Pool::total).sum::<usize>()
    }

    /// 自己和子池子里要查场景碰撞体的粒子数。
    pub fn scene_load(&self, e: &ParticleEmitter3d, depth: usize) -> usize {
        let own = if e.collision.is_some_and(|c| c.scene) { self.particles.len() } else { 0 };
        let subs = if depth < MAX_SUB_DEPTH { e.sub_emitters.as_slice() } else { &[] };
        own + self.children.iter().zip(subs).map(|(c, s)| c.scene_load(&s.emitter, depth + 1)).sum::<usize>()
    }

    pub fn step(&mut self, e: &ParticleEmitter3d, source: Source, f: &mut Frame, depth: usize) {
        self.events.clear();
        self.simulate(e, f);
        self.emit(e, source, f);
        let subs: &[SubEmitter3d] = if depth < MAX_SUB_DEPTH {
            &e.sub_emitters
        } else {
            if !e.sub_emitters.is_empty() && !WARNED_DEPTH.swap(true, Ordering::Relaxed) {
                log::warn!("3D 粒子的子发射器套了超过 {MAX_SUB_DEPTH} 层，更深的不模拟");
            }
            &[]
        };
        self.children.resize_with(subs.len(), Pool::default);
        let Self { events, children, .. } = self;
        for (child, sub) in children.iter_mut().zip(subs) {
            child.step(&sub.emitter, Source::Sub { events: events.as_slice(), sub }, f, depth + 1);
        }
    }

    /// 老化、受力、移动、碰撞；死亡和碰撞记成事件。
    fn simulate(&mut self, e: &ParticleEmitter3d, f: &mut Frame) {
        let dt = f.dt;
        let drag = (-(if e.drag.is_nan() { 0.0 } else { e.drag.max(0.0) }) * dt).exp();
        let n = self.particles.len();
        // 场景查询：这一帧轮到从 cursor 开始的 quota 个
        let scene = e.collision.is_some_and(|c| c.scene) && f.world.is_some();
        // 按粒子数占的比例分，每个池子至少一次（池子多时可能超出上限几次）
        let share = |q: usize| if q == 0 || n == 0 { 0 } else { (q * n / f.budget.load.max(n)).max(1) };
        let quota = if scene { share(f.budget.queries as usize).min(n) } else { 0 };
        let start = if n == 0 { 0 } else { self.cursor % n };
        let (mut index, mut used) = (0, 0);
        let events = &mut self.events;
        self.particles.retain_mut(|p| {
            let i = index;
            index += 1;
            p.age += dt;
            if p.age >= p.lifetime {
                events.push(Event { on: SubTrigger::Death, position: p.position, velocity: p.velocity, normal: Vec3::ZERO });
                return false;
            }
            p.velocity += e.gravity * dt;
            if let Some(noise) = &e.noise {
                let at = p.position * noise.frequency + Vec3::Y * scroll(f.time, noise.scroll);
                p.velocity += curl(at) * noise.strength * dt;
            }
            p.velocity *= drag;
            p.rotation += p.spin * dt;
            let t = p.age / p.lifetime;
            let speed = e.speed_curve.as_ref().and_then(|c| c.eval(t)).unwrap_or(1.0);
            let next = p.position + p.velocity * speed * dt;
            let Some(c) = &e.collision else {
                p.position = next;
                return true;
            };
            let mut hit = c.ground.and_then(|h| collide3d::ground_hit(p.position, next, h, c.radius));
            let queried = (i + n - start) % n.max(1) < quota;
            if queried {
                used += 1;
                let world = f.world.expect("scene 时才有配额");
                // 只要更近的
                let end = hit.map_or(next, |h| h.point);
                hit = collide3d::scene_hit(world, p.position, end, c).or(hit);
            }
            let Some(hit) = hit else {
                // 这一帧没查场景时不知道还贴不贴着，留着上一帧的
                if queried || !scene {
                    p.touching = false;
                }
                p.position = next;
                return true;
            };
            // 上一帧就贴着表面（停着、滑着）：只修正位置和速度，不再发碰撞
            if !std::mem::replace(&mut p.touching, true) {
                events.push(Event { on: SubTrigger::Collision, position: hit.point, velocity: p.velocity, normal: hit.normal });
            }
            if c.kill {
                events.push(Event { on: SubTrigger::Death, position: hit.point, velocity: p.velocity, normal: Vec3::ZERO });
                return false;
            }
            p.velocity = collide3d::bounce(p.velocity, hit.normal, c.bounce, c.friction);
            p.position = hit.point;
            true
        });
        self.cursor = if n == 0 { 0 } else { start + used };
        f.budget.queried += used as u32;
    }

    /// 发新粒子：发射器自己的 burst、rate，或者父池子的事件。
    fn emit(&mut self, e: &ParticleEmitter3d, source: Source, f: &mut Frame) {
        // 网格还没加载：这一帧不发，burst 和 rate 都留着；加载失败的当没有形状，一个也不发
        let surface = match &e.shape {
            EmitShape::Mesh(mesh) => match f.surfaces.get(mesh) {
                Some(s) => Some(s),
                None if f.surfaces.failed(mesh) => None,
                None => return,
            },
            _ => None,
        };
        let surface = surface.as_deref();
        let wanted = match &source {
            Source::Root { .. } => {
                let mut count = 0;
                if !self.burst_done {
                    self.burst_done = true;
                    count += e.burst as usize;
                }
                self.pending += if e.rate.is_nan() { 0.0 } else { e.rate.max(0.0) } * f.dt;
                let whole = self.pending.floor();
                self.pending -= whole;
                count + whole as usize
            }
            Source::Sub { events, sub } => events.iter().filter(|ev| ev.on == sub.on).count().saturating_mul(sub.count as usize),
        };
        let room = e.max.saturating_sub(self.particles.len());
        if wanted > room && !std::mem::replace(&mut self.warned, true) {
            log::warn!("3D 粒子发射器到了上限 {}，多出的不再发射", e.max);
        }
        let allowed = wanted.min(room).min(f.budget.remaining);
        if allowed < wanted.min(room) && !WARNED_BUDGET.swap(true, Ordering::Relaxed) {
            log::warn!("3D 粒子总数到了软上限，多出的不再发射（ParticleBudget3d::limit）");
        }
        f.budget.dropped += wanted.min(room) - allowed;
        if (matches!(e.shape, EmitShape::Mesh(_)) && surface.is_none()) || allowed == 0 {
            return;
        }
        let spawn = |pool: &mut Self, origin: Vec3, rotation: Quat, inherit: Vec3, rng: &mut Rng| {
            let Some(p) = spawn_one(e, origin, rotation, inherit, surface, rng) else { return 0 };
            pool.events.push(Event { on: SubTrigger::Birth, position: p.position, velocity: p.velocity, normal: Vec3::ZERO });
            pool.particles.push(p);
            1
        };
        let mut born = 0;
        match source {
            Source::Root { origin, rotation } => {
                for _ in 0..allowed {
                    born += spawn(self, origin, rotation, Vec3::ZERO, f.rng);
                }
            }
            Source::Sub { events, sub } => {
                for ev in events.iter().filter(|ev| ev.on == sub.on) {
                    if born >= allowed {
                        break;
                    }
                    // 碰撞：本地 +Y 对着碰到的面的法线
                    let rotation = if ev.on == SubTrigger::Collision { Quat::from_rotation_arc(Vec3::Y, ev.normal) } else { Quat::IDENTITY };
                    for _ in 0..(sub.count as usize).min(allowed - born) {
                        born += spawn(self, ev.position, rotation, ev.velocity * sub.inherit, f.rng);
                    }
                }
            }
        }
        f.budget.remaining -= born;
    }

    /// 自己和子池子的粒子画成方片。
    pub fn billboards(&self, e: &ParticleEmitter3d, out: &mut dyn FnMut(ImageId, Blend, Billboard)) {
        for p in &self.particles {
            out(e.texture, e.blend, billboard(e, p));
        }
        for (child, sub) in self.children.iter().zip(&e.sub_emitters) {
            child.billboards(&sub.emitter, out);
        }
    }
}

fn spawn_one(e: &ParticleEmitter3d, origin: Vec3, rotation: Quat, inherit: Vec3, surface: Option<&shape3d::MeshSurface>, rng: &mut Rng) -> Option<Particle3d> {
    let (local, normal) = shape3d::sample(&e.shape, rng, surface)?;
    let direction = shape3d::direction(e.emit_direction, rng, rotation * e.direction, e.spread, rotation * normal);
    let speed = rng.range(e.speed);
    let spin = rng.range(e.spin);
    let angle = match e.angle {
        Some(range) => rng.range(range),
        None => rng.next_f32() * std::f32::consts::TAU,
    };
    let lifetime = rng.range(e.lifetime).max(f32::EPSILON);
    let frame = match &e.sheet {
        Some(s) if s.random_start => (rng.next_f32() * s.count() as f32).floor(),
        _ => 0.0,
    };
    Some(Particle3d { position: origin + rotation * local, velocity: direction * speed + inherit, rotation: angle, spin, age: 0.0, lifetime, frame, touching: false })
}

fn billboard(e: &ParticleEmitter3d, p: &Particle3d) -> Billboard {
    let t = (p.age / p.lifetime).clamp(0.0, 1.0);
    let size = e.size_curve.as_ref().and_then(|c| c.eval(t)).unwrap_or(e.size.0 + (e.size.1 - e.size.0) * t);
    let mut color = e.color_gradient.as_ref().and_then(|g| g.eval(t)).unwrap_or_else(|| e.color.0.lerp(e.color.1, t));
    color.w *= e.alpha_curve.as_ref().and_then(|c| c.eval(t)).unwrap_or(1.0);
    let facing = match e.facing {
        ParticleFacing::Camera => Facing::Camera,
        ParticleFacing::Velocity { stretch } => {
            let speed = e.speed_curve.as_ref().and_then(|c| c.eval(t)).unwrap_or(1.0);
            Facing::Velocity { velocity: p.velocity * speed, stretch }
        }
        ParticleFacing::Horizontal => Facing::Horizontal,
        ParticleFacing::Fixed(q) => Facing::Fixed(q),
    };
    let (uv, next_uv, frame_mix) = match &e.sheet {
        Some(s) => {
            let (a, b, mix) = s.pick(p.age, p.lifetime, p.frame);
            (s.uv(a), s.uv(b), mix)
        }
        None => (FULL_UV, FULL_UV, 0.0),
    };
    Billboard { position: p.position, size, rotation: p.rotation, color, facing, uv, next_uv, frame_mix, soft: e.soft, lit: e.lit }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use glam::{Quat, Vec3, Vec4};

    use super::*;
    use crate::collide3d::{ParticleCollision, Raycast};
    use crate::curve::{Curve, Gradient};
    use crate::emitter3d::{ParticleEmitter3d, ParticleFacing, SubEmitter3d, SubTrigger};
    use crate::sheet::SpriteSheet;
    use crate::shape3d::MeshSurfaces;

    fn budget() -> Budget {
        Budget { remaining: usize::MAX, dropped: 0, queries: u32::MAX, load: 0, queried: 0 }
    }

    /// 一个发射器单独一帧：要查场景的粒子数就是它自己的。
    fn run(pool: &mut Pool, e: &ParticleEmitter3d, budget: &mut Budget, world: Option<&dyn Raycast>, dt: f32) {
        budget.load = pool.scene_load(e, 0);
        step(pool, e, budget, world, dt);
    }

    fn step(pool: &mut Pool, e: &ParticleEmitter3d, budget: &mut Budget, world: Option<&dyn Raycast>, dt: f32) {
        let surfaces = MeshSurfaces::default();
        let mut rng = Rng::new(9);
        let mut f = Frame { dt, time: 0.0, rng: &mut rng, budget, world, surfaces: &surfaces };
        pool.step(e, Source::Root { origin: Vec3::ZERO, rotation: Quat::IDENTITY }, &mut f, 0);
    }

    fn still(burst: u32, lifetime: f32) -> ParticleEmitter3d {
        ParticleEmitter3d { speed: (0.0, 0.0), lifetime: (lifetime, lifetime), ..ParticleEmitter3d::burst(burst) }
    }

    fn sub(on: SubTrigger, count: u32, emitter: ParticleEmitter3d) -> SubEmitter3d {
        SubEmitter3d { on, count, inherit: 0.0, emitter }
    }

    #[test]
    fn sub_emitters_burst_where_parents_die_or_are_born() {
        let mut e = ParticleEmitter3d { gravity: Vec3::new(0.0, -10.0, 0.0), ..still(3, 0.1) };
        e.sub_emitters = vec![sub(SubTrigger::Death, 5, still(0, 10.0)), sub(SubTrigger::Birth, 2, still(0, 10.0))];
        let mut pool = Pool::default();
        let mut b = budget();
        run(&mut pool, &e, &mut b, None, 0.05);
        assert_eq!((pool.particles.len(), pool.children[0].particles.len(), pool.children[1].particles.len()), (3, 0, 6), "出生时各 2 个");
        run(&mut pool, &e, &mut b, None, 0.05);
        let last = pool.particles[0].position;
        assert!(last.y < 0.0);
        run(&mut pool, &e, &mut b, None, 0.06);
        assert_eq!((pool.particles.len(), pool.children[0].particles.len()), (0, 15), "死的时候各 5 个");
        assert!(pool.children[0].particles.iter().all(|p| p.position == last), "在父粒子死的地方");
        assert_eq!(pool.total(), 21);
        // 子发射器少了：多的池子丢掉
        e.sub_emitters.truncate(1);
        run(&mut pool, &e, &mut b, None, 0.01);
        assert_eq!(pool.children.len(), 1);
    }

    #[test]
    fn children_inherit_the_parent_velocity() {
        let mut e = ParticleEmitter3d { speed: (4.0, 4.0), direction: Vec3::X, spread: 0.0, ..still(1, 10.0) };
        e.sub_emitters = vec![SubEmitter3d { inherit: 0.5, ..sub(SubTrigger::Birth, 1, still(0, 10.0)) }];
        let mut pool = Pool::default();
        run(&mut pool, &e, &mut budget(), None, 0.0);
        assert!((pool.children[0].particles[0].velocity - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn falling_onto_the_ground_kills_and_splashes() {
        let ground = ParticleCollision { ground: Some(0.0), kill: true, ..Default::default() };
        let mut e = ParticleEmitter3d { speed: (10.0, 10.0), direction: Vec3::new(1.0, -1.0, 0.0), spread: 0.0, collision: Some(ground), ..still(4, 10.0) };
        // 溅起来的：本地 +Y 对着地面的法线
        let splash = ParticleEmitter3d { speed: (3.0, 3.0), direction: Vec3::Y, spread: 0.0, ..still(0, 10.0) };
        e.sub_emitters = vec![sub(SubTrigger::Collision, 2, splash), sub(SubTrigger::Death, 1, still(0, 10.0))];
        let mut pool = Pool::default();
        let mut b = budget();
        run(&mut pool, &e, &mut b, None, 0.0);
        // 出生在 y = 0 往下走：第一步就穿过地面
        let mut e2 = e.clone();
        e2.collision = Some(ParticleCollision { ground: Some(-1.0), ..ground });
        run(&mut pool, &e2, &mut b, None, 1.0);
        assert_eq!(pool.particles.len(), 0, "碰到就死");
        let splash = &pool.children[0].particles;
        assert_eq!(splash.len(), 8);
        assert!(splash.iter().all(|p| (p.position.y + 1.0).abs() < 1e-4 && (p.velocity - Vec3::new(0.0, 3.0, 0.0)).length() < 1e-4), "从地面往上溅 {:?}", splash[0]);
        assert_eq!(pool.children[1].particles.len(), 4, "碰死也算死");
    }

    #[test]
    fn bouncing_flips_and_keeps_particles_above_the_ground() {
        let ground = ParticleCollision { ground: Some(0.0), bounce: 1.0, friction: 0.5, radius: 0.1, ..Default::default() };
        let e = ParticleEmitter3d { speed: (5.0, 5.0), direction: Vec3::new(1.0, -1.0, 0.0), spread: 0.0, collision: Some(ground), ..still(1, 10.0) };
        let mut pool = Pool::default();
        run(&mut pool, &e, &mut budget(), None, 0.0);
        pool.particles[0].position.y = 1.0;
        run(&mut pool, &e, &mut budget(), None, 1.0);
        let p = pool.particles[0];
        let v = 5.0 / 2f32.sqrt();
        assert!((p.velocity - Vec3::new(v * 0.5, v, 0.0)).length() < 1e-4, "{:?}", p.velocity);
        assert!(p.position.y >= 0.1 - 1e-5, "停在地面上半径那么高 {:?}", p.position);
    }

    /// 记下每次查询的起点。
    struct Walls(RefCell<Vec<Vec3>>);

    impl Raycast for Walls {
        fn raycast(&self, origin: Vec3, _: Vec3, _: f32, _: &ParticleCollision) -> Option<(Vec3, Vec3)> {
            self.0.borrow_mut().push(origin);
            None
        }
    }

    /// x = 1 处一面朝 -X 的墙。
    struct Wall;

    impl Raycast for Wall {
        fn raycast(&self, origin: Vec3, dir: Vec3, max: f32, _: &ParticleCollision) -> Option<(Vec3, Vec3)> {
            let t = (1.0 - origin.x) / dir.x;
            (dir.x > 0.0 && t <= max).then(|| (origin + dir * t, Vec3::NEG_X))
        }
    }

    #[test]
    fn splashes_point_along_the_normal_of_what_was_hit() {
        let wall = ParticleCollision { scene: true, kill: true, ..Default::default() };
        let mut e = ParticleEmitter3d { speed: (4.0, 4.0), direction: Vec3::X, spread: 0.0, collision: Some(wall), ..still(1, 10.0) };
        let splash = ParticleEmitter3d { speed: (3.0, 3.0), direction: Vec3::Y, spread: 0.0, ..still(0, 10.0) };
        e.sub_emitters = vec![sub(SubTrigger::Collision, 1, splash)];
        let mut pool = Pool::default();
        run(&mut pool, &e, &mut budget(), Some(&Wall), 0.0);
        run(&mut pool, &e, &mut budget(), Some(&Wall), 1.0);
        let p = &pool.children[0].particles[0];
        assert!((p.position - Vec3::X).length() < 1e-3 && (p.velocity - Vec3::new(-3.0, 0.0, 0.0)).length() < 1e-4, "本地 +Y 对着墙的法线 {p:?}");
    }

    #[test]
    fn scene_queries_are_capped_and_take_turns() {
        let e = ParticleEmitter3d { speed: (1.0, 1.0), collision: Some(ParticleCollision { scene: true, ..Default::default() }), ..still(50, 100.0) };
        let mut pool = Pool::default();
        run(&mut pool, &e, &mut budget(), None, 0.0);
        for (i, p) in pool.particles.iter_mut().enumerate() {
            p.position = Vec3::new(i as f32, 0.0, 0.0);
            p.velocity = Vec3::Y;
        }
        let walls = Walls(RefCell::new(Vec::new()));
        let mut seen = std::collections::HashSet::new();
        for _ in 0..3 {
            let mut b = Budget { queries: 20, ..budget() };
            walls.0.borrow_mut().clear();
            run(&mut pool, &e, &mut b, Some(&walls), 0.01);
            assert_eq!((walls.0.borrow().len(), b.queried), (20, 20), "每帧最多 20 次");
            seen.extend(walls.0.borrow().iter().map(|p| p.x as i32));
        }
        assert_eq!(seen.len(), 50, "轮流查，三帧下来每个粒子都查到过");
    }

    #[test]
    fn the_total_budget_and_the_nesting_depth_are_capped() {
        // 一层套一层：每个粒子出生时在下一层再出生一个
        let mut e = still(1, 10.0);
        for _ in 0..6 {
            let mut parent = still(0, 10.0);
            parent.sub_emitters = vec![sub(SubTrigger::Birth, 1, std::mem::replace(&mut e, still(0, 10.0)))];
            e = parent;
        }
        e.burst = 1;
        let mut pool = Pool::default();
        run(&mut pool, &e, &mut budget(), None, 0.0);
        assert_eq!(pool.total(), 1 + MAX_SUB_DEPTH, "最多套 {MAX_SUB_DEPTH} 层");
        // 总数软上限：剩 10 个时只发 10 个，子发射器也算（10 个出生事件要的 10 个也没发）
        let mut e = still(100, 10.0);
        e.sub_emitters = vec![sub(SubTrigger::Birth, 1, still(0, 10.0))];
        let mut pool = Pool::default();
        let mut b = Budget { remaining: 10, ..budget() };
        run(&mut pool, &e, &mut b, None, 0.0);
        assert_eq!((pool.total(), b.remaining, b.dropped), (10, 0, 100));
    }

    #[test]
    fn drag_and_speed_curves_slow_particles_down() {
        let e = ParticleEmitter3d { speed: (2.0, 2.0), direction: Vec3::X, spread: 0.0, drag: 1.0, ..still(1, 10.0) };
        let mut pool = Pool::default();
        run(&mut pool, &e, &mut budget(), None, 0.0);
        for _ in 0..100 {
            run(&mut pool, &e, &mut budget(), None, 0.01);
        }
        assert!((pool.particles[0].velocity.x - 2.0 * (-1.0f32).exp()).abs() < 1e-3, "一秒后剩 1/e：{}", pool.particles[0].velocity.x);
        // 速度曲线是 0：速度照算，位置不动
        let e = ParticleEmitter3d { speed_curve: Some(Curve::new([(0.0, 0.0)])), gravity: Vec3::NEG_Y, ..e };
        let mut pool = Pool::default();
        run(&mut pool, &e, &mut budget(), None, 0.0);
        run(&mut pool, &e, &mut budget(), None, 0.5);
        assert_eq!(pool.particles[0].position, Vec3::ZERO);
        assert!(pool.particles[0].velocity.y < 0.0);
    }

    #[test]
    fn the_start_angle_can_be_fixed() {
        let mut pool = Pool::default();
        run(&mut pool, &ParticleEmitter3d { angle: Some((0.5, 0.5)), ..still(20, 1.0) }, &mut budget(), None, 0.0);
        assert!(pool.particles.iter().all(|p| p.rotation == 0.5));
        let mut pool = Pool::default();
        run(&mut pool, &still(20, 1.0), &mut budget(), None, 0.0);
        assert!(pool.particles.iter().any(|p| p.rotation > 3.0), "不设时一整圈随机");
    }

    #[test]
    fn noise_pushes_particles_around() {
        let e = ParticleEmitter3d { noise: Some(crate::noise::Noise3d { strength: 10.0, frequency: 0.7, scroll: 0.0 }), ..still(20, 10.0) };
        let mut pool = Pool::default();
        run(&mut pool, &e, &mut budget(), None, 0.0);
        for (i, p) in pool.particles.iter_mut().enumerate() {
            p.position = Vec3::new(i as f32 * 0.37, 0.2, 0.1);
        }
        run(&mut pool, &e, &mut budget(), None, 0.1);
        assert!(pool.particles.iter().all(|p| p.velocity.is_finite()));
        assert!(pool.particles.iter().any(|p| p.velocity.length() > 0.05), "有湍流");
    }

    #[test]
    fn billboards_follow_curves_facing_sheets_and_flags() {
        let e = ParticleEmitter3d {
            speed: (2.0, 2.0),
            direction: Vec3::X,
            spread: 0.0,
            size_curve: Some(Curve::new([(0.0, 1.0), (0.5, 3.0), (1.0, 0.0)])),
            color_gradient: Some(Gradient::new([(0.0, Vec4::ONE), (1.0, Vec4::new(1.0, 0.0, 0.0, 1.0))])),
            alpha_curve: Some(Curve::new([(0.0, 0.5)])),
            facing: ParticleFacing::Velocity { stretch: 0.25 },
            sheet: Some(SpriteSheet { columns: 2, rows: 2, ..Default::default() }),
            soft: 3.0,
            lit: true,
            ..still(1, 2.0)
        };
        let mut pool = Pool::default();
        run(&mut pool, &e, &mut budget(), None, 0.0);
        run(&mut pool, &e, &mut budget(), None, 0.5);
        let mut out = Vec::new();
        pool.billboards(&e, &mut |_, _, b| out.push(b));
        let b = out[0];
        // 年龄 0.5、寿命 2：t = 0.25
        assert_eq!(b.size, 2.0);
        assert_eq!(b.color, Vec4::new(1.0, 0.75, 0.75, 0.5));
        let nge_render3d::Facing::Velocity { velocity, stretch } = b.facing else { panic!("{:?}", b.facing) };
        assert!((velocity - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-5 && stretch == 0.25);
        assert_eq!((b.uv, b.next_uv), ([0.5, 0.0, 1.0, 0.5], [0.0, 0.5, 0.5, 1.0]), "4 帧的第 1 帧");
        assert_eq!((b.soft, b.lit), (3.0, true));
    }

    #[test]
    fn pools_reuse_their_buffers() {
        let mut e = ParticleEmitter3d { rate: 200.0, ..still(0, 0.1) };
        e.sub_emitters = vec![sub(SubTrigger::Death, 1, still(0, 0.05))];
        let mut pool = Pool::default();
        for _ in 0..60 {
            run(&mut pool, &e, &mut budget(), None, 0.01);
        }
        let caps = |p: &Pool| [p.particles.capacity(), p.events.capacity(), p.children[0].particles.capacity(), p.children[0].events.capacity()];
        let first = caps(&pool);
        for _ in 0..120 {
            run(&mut pool, &e, &mut budget(), None, 0.01);
        }
        assert_eq!(caps(&pool), first, "稳定以后容量不再增长");
    }

    #[test]
    fn huge_sub_counts_stop_at_the_budget() {
        let mut e = still(1, 10.0);
        e.sub_emitters = vec![sub(SubTrigger::Birth, u32::MAX, still(0, 10.0))];
        let mut pool = Pool::default();
        let mut b = Budget { remaining: 11, ..budget() };
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
        run(&mut pool, &e, &mut b, None, 0.0);
        assert!(std::time::Instant::now() < deadline, "预算满了就停，不空转 count 次");
        // 子发射器自己的上限 10000 挡掉的不算，总数上限挡掉的才算
        assert_eq!((pool.total(), b.dropped), (11, 10_000 - 10));
    }

    #[test]
    fn scene_queries_are_shared_by_every_pool() {
        let col = Some(ParticleCollision { scene: true, ..Default::default() });
        let big = ParticleEmitter3d { speed: (1.0, 1.0), collision: col, ..still(300, 100.0) };
        let mut small = ParticleEmitter3d { speed: (1.0, 1.0), collision: col, ..still(5, 100.0) };
        small.sub_emitters = vec![sub(SubTrigger::Birth, 2, ParticleEmitter3d { speed: (1.0, 1.0), collision: col, ..still(0, 100.0) })];
        let (mut a, mut b) = (Pool::default(), Pool::default());
        run(&mut a, &big, &mut budget(), None, 0.0);
        run(&mut b, &small, &mut budget(), None, 0.0);
        assert_eq!((a.scene_load(&big, 0), b.scene_load(&small, 0)), (300, 15), "子池子也算");
        let (wa, wb) = (Walls(RefCell::new(Vec::new())), Walls(RefCell::new(Vec::new())));
        for _ in 0..10 {
            // 32 × 5 / 315 不到 1：靠保底
            let mut budget = Budget { queries: 32, load: 315, ..budget() };
            step(&mut a, &big, &mut budget, Some(&wa), 1.0 / 60.0);
            step(&mut b, &small, &mut budget, Some(&wb), 1.0 / 60.0);
            assert!(budget.queried <= 32 + 3, "按粒子数分，每个池子保底一次：{}", budget.queried);
        }
        assert!(wa.0.borrow().len() >= 250, "大的照样查得多：{}", wa.0.borrow().len());
        assert!(wb.0.borrow().len() >= 20, "排在后面的、子池子也查得到：{}", wb.0.borrow().len());
    }

    /// y = 0 处朝上的地面。
    struct Floor;

    impl Raycast for Floor {
        fn raycast(&self, origin: Vec3, dir: Vec3, max: f32, _: &ParticleCollision) -> Option<(Vec3, Vec3)> {
            let t = origin.y / -dir.y;
            (dir.y < 0.0 && origin.y >= 0.0 && t <= max).then(|| (origin + dir * t, Vec3::Y))
        }
    }

    #[test]
    fn resting_particles_splash_once() {
        for scene in [false, true] {
            for (bounce, splashes) in [(0.0, 1), (0.8, 2)] {
                let c = ParticleCollision { ground: (!scene).then_some(0.0), scene, bounce, ..Default::default() };
                let mut e = ParticleEmitter3d { gravity: Vec3::new(0.0, -10.0, 0.0), collision: Some(c), ..still(1, 100.0) };
                e.sub_emitters = vec![sub(SubTrigger::Collision, 1, still(0, 100.0))];
                let mut pool = Pool::default();
                run(&mut pool, &e, &mut budget(), Some(&Floor), 0.0);
                pool.particles[0].position.y = 0.5;
                for _ in 0..300 {
                    run(&mut pool, &e, &mut budget(), Some(&Floor), 1.0 / 60.0);
                }
                let n = pool.children[0].particles.len();
                assert!(pool.particles[0].position.y.abs() < 0.05, "停在地上");
                if bounce == 0.0 {
                    assert_eq!(n, splashes, "贴着地面不再发碰撞（scene = {scene}）");
                } else {
                    assert!(n >= splashes && n < 60, "每弹一次发一次（scene = {scene}）：{n}");
                }
            }
        }
    }

    #[test]
    fn particles_pressed_on_a_wall_splash_once_even_when_queries_take_turns() {
        // 两个粒子被推向墙，每帧只查一次场景：没轮到的那帧不知道还贴不贴着，不能当成离开了
        let c = ParticleCollision { scene: true, bounce: 0.0, ..Default::default() };
        let mut e = ParticleEmitter3d { gravity: Vec3::new(10.0, 0.0, 0.0), collision: Some(c), ..still(2, 100.0) };
        e.sub_emitters = vec![sub(SubTrigger::Collision, 1, still(0, 100.0))];
        let mut pool = Pool::default();
        run(&mut pool, &e, &mut budget(), Some(&Wall), 0.0);
        for _ in 0..120 {
            run(&mut pool, &e, &mut Budget { queries: 1, ..budget() }, Some(&Wall), 1.0 / 60.0);
        }
        assert_eq!(pool.children[0].particles.len(), 2);
    }

    #[test]
    fn bursts_wait_for_the_mesh() {
        use std::sync::Arc;

        use crate::shape3d::{MeshSource, MeshSurface};
        let missing = nge_render3d::Assets3d::default().add_model(nge_render3d::Model::default()).unwrap();
        let mut e = ParticleEmitter3d { rate: 10.0, shape: EmitShape::Mesh(MeshSource::Model { model: missing, scale: Vec3::ONE }), ..still(50, 10.0) };
        let mut pool = Pool::default();
        for _ in 0..5 {
            run(&mut pool, &e, &mut budget(), None, 1.0);
        }
        assert_eq!((pool.particles.len(), pool.burst_done), (0, false), "网格还没好：burst 和 rate 都留着");
        let tri = [(Vec3::ZERO, Vec3::Y), (Vec3::X, Vec3::Y), (Vec3::Z, Vec3::Y)];
        e.shape = EmitShape::Mesh(MeshSource::Surface(Arc::new(MeshSurface::new([tri]))));
        run(&mut pool, &e, &mut budget(), None, 0.5);
        assert_eq!(pool.particles.len(), 50 + 5, "好了以后照常发，等的那几秒的 rate 不补");
    }
}
