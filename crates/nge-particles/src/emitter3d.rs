//! 3D 粒子发射器（见 docs/particles3d.md、docs/particles3d-more.md）：跟着实体的 `Transform3d`，粒子画成方片。

use bevy_ecs::prelude::*;
use glam::{Quat, Vec3, Vec4};
use nge_assets::ImageId;
use nge_ecs::{ParticleStats, Screen, Time};
use nge_physics3d::Physics3d;
use nge_render3d::{Billboard, Billboards, Blend, Camera3d, Facing, Transform3d};

use crate::collide3d::{ParticleCollision, Raycast};
use crate::curve::{Curve, Gradient};
use crate::emitter::DEFAULT_MAX_PARTICLES;
use crate::noise::Noise3d;
use crate::pool3d::{Budget, Frame, Pool, Source};
use crate::rng::Rng;
use crate::shape3d::{EmitDirection, EmitShape, MeshSurfaces};
use crate::sheet::SpriteSheet;

/// 方片朝哪。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ParticleFacing {
    /// 朝着相机。
    #[default]
    Camera,
    /// 沿速度拉长：长 `size + |速度| × stretch`。
    Velocity { stretch: f32 },
    /// 躺在 xz 平面里（地面冲击波）。
    Horizontal,
    /// 固定的世界朝向：本地 xy 平面的方片乘这个旋转。
    Fixed(Quat),
}

/// 子发射器在父粒子的什么时候 burst。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubTrigger {
    Birth,
    #[default]
    Death,
    Collision,
}

/// 子发射器：父粒子出生、死亡、碰撞时在它的位置 burst `count` 个。`emitter` 的 `rate`、`burst`、`despawn_when_done` 不起作用。
#[derive(Debug, Clone, PartialEq)]
pub struct SubEmitter3d {
    pub on: SubTrigger,
    pub count: u32,
    /// 继承父粒子速度的比例。
    pub inherit: f32,
    pub emitter: ParticleEmitter3d,
}

/// 3D 粒子发射器。范围参数写成 `(最小, 最大)`；`size` / `color` 是 `(出生, 死亡)`，中间线性插值（设了曲线时用曲线）。
#[derive(Component, Debug, Clone, PartialEq)]
#[require(Particles3d)]
pub struct ParticleEmitter3d {
    /// 每秒持续发射的数量。
    pub rate: f32,
    /// 第一帧一次性发射的数量。
    pub burst: u32,
    /// 寿命（秒）。
    pub lifetime: (f32, f32),
    /// 初速度（世界单位 / 秒）。
    pub speed: (f32, f32),
    /// 发射方向（本地坐标，随实体旋转）。
    pub direction: Vec3,
    /// 方向锥的半角（弧度）：0 是一条线，π 是全方向。
    pub spread: f32,
    /// 加速度（世界坐标）。
    pub gravity: Vec3,
    pub size: (f32, f32),
    /// 方片转动的角速度（弧度 / 秒），出生时随机取。
    pub spin: (f32, f32),
    /// 出生时的转角范围（弧度）；不设时在一整圈里随机（序列帧的火苗要正着就设 `(0, 0)`）。
    pub angle: Option<(f32, f32)>,
    /// 线性 RGBA。
    pub color: (Vec4, Vec4),
    pub texture: ImageId,
    pub blend: Blend,
    /// 同时存活的上限，到了就不再发射。
    pub max: usize,
    /// 不再持续发射（`rate` 为 0）且粒子（包括子发射器的）全部死完后删除实体。
    pub despawn_when_done: bool,
    /// 在哪出生（本地坐标）。
    pub shape: EmitShape,
    pub emit_direction: EmitDirection,
    /// 大小随寿命（设了就不用 `size`）。
    pub size_curve: Option<Curve>,
    /// 颜色随寿命（设了就不用 `color`）。
    pub color_gradient: Option<Gradient>,
    /// 乘到透明度上。
    pub alpha_curve: Option<Curve>,
    /// 乘到每帧的位移上。
    pub speed_curve: Option<Curve>,
    /// 速度阻尼：每秒乘 `e^-drag`。
    pub drag: f32,
    pub noise: Option<Noise3d>,
    pub facing: ParticleFacing,
    pub sheet: Option<SpriteSheet>,
    /// 软化距离（世界单位），0 不软化。
    pub soft: f32,
    /// 受环境光和方向光影响。
    pub lit: bool,
    pub collision: Option<ParticleCollision>,
    pub sub_emitters: Vec<SubEmitter3d>,
}

impl Default for ParticleEmitter3d {
    fn default() -> Self {
        Self {
            rate: 0.0,
            burst: 0,
            lifetime: (1.0, 1.0),
            speed: (1.0, 2.0),
            direction: Vec3::Y,
            spread: std::f32::consts::PI,
            gravity: Vec3::ZERO,
            size: (0.2, 0.0),
            spin: (0.0, 0.0),
            angle: None,
            color: (Vec4::ONE, Vec4::new(1.0, 1.0, 1.0, 0.0)),
            texture: ImageId::WHITE,
            blend: Blend::Alpha,
            max: DEFAULT_MAX_PARTICLES,
            despawn_when_done: false,
            shape: EmitShape::Point,
            emit_direction: EmitDirection::Cone,
            size_curve: None,
            color_gradient: None,
            alpha_curve: None,
            speed_curve: None,
            drag: 0.0,
            noise: None,
            facing: ParticleFacing::Camera,
            sheet: None,
            soft: 0.0,
            lit: false,
            collision: None,
            sub_emitters: Vec::new(),
        }
    }
}

impl ParticleEmitter3d {
    /// 一次性爆炸：发射 `count` 个，死完后删除实体。
    pub fn burst(count: u32) -> Self {
        Self { burst: count, despawn_when_done: true, ..Default::default() }
    }

    /// 持续发射：每秒 `rate` 个，实体一直保留。
    pub fn continuous(rate: f32) -> Self {
        Self { rate, ..Default::default() }
    }
}

/// 一个存活的 3D 粒子（世界坐标）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle3d {
    pub position: Vec3,
    pub velocity: Vec3,
    pub rotation: f32,
    pub spin: f32,
    pub age: f32,
    pub lifetime: f32,
    /// 序列帧的起始帧。
    pub frame: f32,
    /// 上一帧碰到了表面：贴着表面时不再发碰撞事件。
    pub touching: bool,
}

/// 所有 3D 粒子共用的上限（资源，`Particles3dPlugin` 加上）。
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct ParticleBudget3d {
    /// 粒子总数的软上限（包括子发射器的）：到了就不再发射。
    pub limit: usize,
    /// 每帧最多查几次场景碰撞体。
    pub max_queries: u32,
}

impl Default for ParticleBudget3d {
    fn default() -> Self {
        Self { limit: 20_000, max_queries: 256 }
    }
}

/// 发射器的运行状态，随 [`ParticleEmitter3d`] 自动加上。
#[derive(Component, Debug, Clone)]
pub struct Particles3d {
    pool: Pool,
    rng: Rng,
}

impl Default for Particles3d {
    fn default() -> Self {
        static SEED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x2545_F491);
        let seed = SEED.fetch_add(0x6D2B_79F5, std::sync::atomic::Ordering::Relaxed);
        Self { pool: Pool::default(), rng: Rng::new(seed) }
    }
}

impl Particles3d {
    /// 粒子数，包括子发射器的。
    pub fn len(&self) -> usize {
        self.pool.total()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 发射器自己的粒子（不含子发射器的）。
    pub fn iter(&self) -> impl Iterator<Item = &Particle3d> {
        self.pool.particles.iter()
    }

    fn advance(&mut self, emitter: &ParticleEmitter3d, transform: &Transform3d, dt: f32, time: f64, budget: &mut Budget, world: Option<&dyn Raycast>, surfaces: &MeshSurfaces) {
        let mut frame = Frame { dt, time, rng: &mut self.rng, budget, world, surfaces };
        self.pool.step(emitter, Source::Root { origin: transform.translation, rotation: transform.rotation }, &mut frame, 0);
    }

    fn finished(&self, emitter: &ParticleEmitter3d) -> bool {
        self.pool.burst_done && emitter.rate <= 0.0 && self.pool.total() == 0
    }
}

/// 方片的外接框（每个按能转到的最大范围算）；没有方片时是空框（不画）。
pub(crate) fn billboard_bounds<'a>(billboards: impl Iterator<Item = &'a Billboard>) -> (Vec3, Vec3) {
    billboards.fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), b| {
        let stretch = match b.facing {
            Facing::Velocity { velocity, stretch } => velocity.length() * stretch.abs(),
            _ => 0.0,
        };
        let r = Vec3::splat(b.size.abs() + stretch);
        (lo.min(b.position - r), hi.max(b.position + r))
    })
}

static WARNED_NO_PHYSICS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn simulate3d(
    mut commands: Commands,
    time: Res<Time>,
    limits: Res<ParticleBudget3d>,
    surfaces: Res<MeshSurfaces>,
    physics: Option<Res<Physics3d>>,
    mut stats: ResMut<ParticleStats>,
    mut billboards: ResMut<Billboards>,
    screen: Res<Screen>,
    cameras: Query<(&Camera3d, &Transform3d)>,
    mut scratch: Local<Vec<(ImageId, Blend, Billboard)>>,
    mut emitters: Query<(Entity, &ParticleEmitter3d, &mut Particles3d, Option<&Transform3d>)>,
) {
    let (camera, eye) = nge_render3d::main_camera(cameras.iter().next());
    let view_proj = camera.view_proj(&eye, screen.size.x / screen.size.y.max(1.0));
    // 软上限按这一帧开头的存活数算，场景查询按这一帧开头要查的粒子数分
    let (alive, load) = emitters.iter().fold((0, 0), |(a, l), (_, e, p, _)| (a + p.len(), l + p.pool.scene_load(e, 0)));
    let mut budget = Budget { remaining: limits.limit.saturating_sub(alive), dropped: 0, queries: limits.max_queries, load, queried: 0 };
    let world = physics.as_deref().map(|p| p as &dyn Raycast);
    if world.is_none() && load > 0 && !WARNED_NO_PHYSICS.swap(true, std::sync::atomic::Ordering::Relaxed) {
        log::warn!("3D 粒子开了 collision.scene，但没有 3D 物理（Physics3dPlugin），只碰地面");
    }
    let mut total = 0;
    for (entity, emitter, mut particles, transform) in &mut emitters {
        let transform = transform.copied().unwrap_or_default();
        particles.advance(emitter, &transform, time.delta, time.elapsed, &mut budget, world, &surfaces);
        if emitter.despawn_when_done && particles.finished(emitter) {
            commands.entity(entity).despawn();
            continue;
        }
        total += particles.len();
        scratch.clear();
        particles.pool.billboards(emitter, &mut |texture, blend, b| scratch.push((texture, blend, b)));
        // 整个发射器（连子发射器）都在视锥外时不交给渲染器
        if nge_render3d::in_view(view_proj, billboard_bounds(scratch.iter().map(|s| &s.2))) {
            for &(texture, blend, b) in scratch.iter() {
                billboards.push(texture, blend, b);
            }
        }
    }
    *stats = ParticleStats { alive: total as u32, dropped: budget.dropped.min(u32::MAX as usize) as u32, queries: budget.queried };
}

#[cfg(test)]
mod tests {
    use nge_core::Viewport;
    use nge_ecs::App;
    use nge_render3d::Render3dPlugin;

    use super::*;
    use crate::Particles3dPlugin;
    use crate::shape3d::cone;
    use crate::SubEmitter3d;

    const VIEWPORT: Viewport = Viewport { size: [100.0, 100.0], pixels: [100, 100] };

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(Render3dPlugin).add_plugins(Particles3dPlugin);
        app
    }

    fn alive(app: &mut App) -> usize {
        app.world_mut().query::<&Particles3d>().iter(app.world()).map(Particles3d::len).sum()
    }

    #[test]
    fn bursts_and_rates_become_billboards() {
        let mut app = app();
        app.world_mut().spawn((ParticleEmitter3d { lifetime: (10.0, 10.0), ..ParticleEmitter3d::burst(40) }, Transform3d::default()));
        app.world_mut().spawn(ParticleEmitter3d { lifetime: (10.0, 10.0), ..ParticleEmitter3d::continuous(30.0) });
        for _ in 0..10 {
            app.tick(VIEWPORT, 0.05);
        }
        assert_eq!(alive(&mut app), 40 + 15, "爆炸只发一次；30/秒 × 0.5 秒");
        assert_eq!(app.world().resource::<Billboards>().quads(), 55, "每帧清空后重新放");
    }

    #[test]
    fn emitters_outside_the_view_still_simulate_but_are_not_drawn() {
        let mut app = app();
        // 默认相机在 (0, 0, 5) 看 -z：背后的那个不画，前面的画
        app.world_mut().spawn((ParticleEmitter3d { lifetime: (10.0, 10.0), speed: (0.0, 0.0), ..ParticleEmitter3d::burst(30) }, Transform3d::from_xyz(0.0, 0.0, 20.0)));
        app.world_mut().spawn((ParticleEmitter3d { lifetime: (10.0, 10.0), speed: (0.0, 0.0), ..ParticleEmitter3d::burst(10) }, Transform3d::default()));
        app.tick(VIEWPORT, 0.05);
        assert_eq!(alive(&mut app), 40, "看不见的照样模拟");
        assert_eq!(app.world().resource::<Billboards>().quads(), 10);
        // 相机转过去以后两个都看得见
        app.world_mut().spawn((nge_render3d::Camera3d::default(), Transform3d::from_xyz(0.0, 0.0, 40.0)));
        app.tick(VIEWPORT, 0.05);
        assert_eq!(app.world().resource::<Billboards>().quads(), 40);
    }

    #[test]
    fn billboard_bounds_cover_stretched_quads() {
        let b = |position: Vec3, facing: Facing| Billboard { position, size: 2.0, facing, ..Default::default() };
        let quads = [b(Vec3::ZERO, Facing::Camera), b(Vec3::X * 10.0, Facing::Velocity { velocity: Vec3::Y * 3.0, stretch: 1.0 })];
        assert_eq!(billboard_bounds(quads.iter()), (Vec3::new(-2.0, -5.0, -5.0), Vec3::new(15.0, 5.0, 5.0)));
        let (lo, hi) = billboard_bounds([].iter());
        assert!(lo.cmpgt(hi).all(), "没有方片是空框");
    }

    #[test]
    fn bursts_despawn_when_done_and_the_cap_holds() {
        let mut app = app();
        let gone = app.world_mut().spawn(ParticleEmitter3d { lifetime: (0.1, 0.1), ..ParticleEmitter3d::burst(5) }).id();
        let kept = app.world_mut().spawn(ParticleEmitter3d { lifetime: (0.1, 0.1), despawn_when_done: false, ..ParticleEmitter3d::burst(5) }).id();
        let capped = app.world_mut().spawn(ParticleEmitter3d { max: 3, lifetime: (10.0, 10.0), rate: 1000.0, ..ParticleEmitter3d::burst(50) }).id();
        app.tick(VIEWPORT, 0.05);
        assert_eq!(app.world().get::<Particles3d>(capped).unwrap().len(), 3);
        for _ in 0..3 {
            app.tick(VIEWPORT, 0.05);
        }
        assert!(app.world().get_entity(gone).is_err());
        assert!(app.world().get_entity(kept).is_ok());
        assert_eq!(app.world().get::<Particles3d>(capped).unwrap().len(), 3);
    }

    /// 不限量地推进 `dt` 秒。
    fn step(particles: &mut Particles3d, emitter: &ParticleEmitter3d, origin: Vec3, rotation: Quat, dt: f32) {
        let mut budget = Budget { remaining: usize::MAX, dropped: 0, queries: 0, load: 0, queried: 0 };
        particles.advance(emitter, &Transform3d { translation: origin, rotation, ..Default::default() }, dt, 0.0, &mut budget, None, &MeshSurfaces::default());
    }

    #[test]
    fn motion_follows_direction_gravity_spin_and_interpolates() {
        let mut particles = Particles3d::default();
        let emitter = ParticleEmitter3d {
            speed: (2.0, 2.0),
            direction: Vec3::X,
            spread: 0.0,
            gravity: Vec3::new(0.0, -4.0, 0.0),
            spin: (1.0, 1.0),
            lifetime: (2.0, 2.0),
            size: (1.0, 0.0),
            color: (Vec4::ONE, Vec4::ZERO),
            ..ParticleEmitter3d::burst(1)
        };
        // 发射器转了 90°（绕 y）：本地 +X 变成世界 -Z
        step(&mut particles, &emitter, Vec3::new(1.0, 1.0, 1.0), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), 0.0);
        let start = particles.iter().next().unwrap().rotation;
        step(&mut particles, &emitter, Vec3::ZERO, Quat::IDENTITY, 1.0);
        let p = particles.iter().next().unwrap();
        assert!((p.position - Vec3::new(1.0, -3.0, -1.0)).length() < 1e-4, "{}", p.position);
        assert!((p.rotation - start - 1.0).abs() < 1e-5);
        let mut out = Vec::new();
        particles.pool.billboards(&emitter, &mut |_, _, b| out.push(b));
        let b = out[0];
        assert_eq!((b.size, b.color), (0.5, Vec4::splat(0.5)));
    }

    #[test]
    fn directions_stay_inside_the_cone_and_fill_it() {
        let mut rng = Rng::new(5);
        let axis = Vec3::new(1.0, 1.0, 0.0).normalize();
        let spread = 30f32.to_radians();
        let dirs: Vec<Vec3> = (0..2000).map(|_| cone(&mut rng, axis * 3.0, spread)).collect();
        assert!(dirs.iter().all(|d| (d.length() - 1.0).abs() < 1e-4));
        let angles: Vec<f32> = dirs.iter().map(|d| d.angle_between(axis)).collect();
        assert!(angles.iter().all(|a| *a <= spread + 1e-3), "都在锥里");
        assert!(angles.iter().any(|a| *a > spread * 0.9), "能到锥的边上");
        // 立体角均匀：半角 30° 的锥里，一半立体角在 21.3° 以内
        let inner = angles.iter().filter(|a| **a < 21.3f32.to_radians()).count() as f32 / angles.len() as f32;
        assert!((inner - 0.5).abs() < 0.05, "{inner}");
        assert!((cone(&mut rng, Vec3::ZERO, 0.0) - Vec3::Y).length() < 1e-5, "零向量当 +Y");
        let all: Vec<Vec3> = (0..2000).map(|_| cone(&mut rng, Vec3::Y, 10.0)).collect();
        assert!(all.iter().any(|d| d.y < -0.9), "超过 π 的当 π，全方向");
        let nan: Vec<Vec3> = (0..2000).map(|_| cone(&mut rng, Vec3::Y, f32::NAN)).collect();
        assert!(nan.iter().all(|d| d.is_finite()) && nan.iter().any(|d| d.y < -0.9), "NaN 当全方向");
    }

    #[test]
    fn the_soft_limit_counts_every_emitter_and_feeds_the_stats() {
        let mut app = app();
        app.insert_resource(ParticleBudget3d { limit: 30, ..Default::default() });
        for _ in 0..2 {
            app.world_mut().spawn(ParticleEmitter3d { lifetime: (10.0, 10.0), ..ParticleEmitter3d::burst(20) });
        }
        app.tick(VIEWPORT, 0.05);
        assert_eq!(alive(&mut app), 30, "两个发射器合起来不超过 30");
        assert_eq!(*app.world().resource::<ParticleStats>(), ParticleStats { alive: 30, dropped: 10, queries: 0 });
        app.tick(VIEWPORT, 0.05);
        assert_eq!(app.world().resource::<ParticleStats>().dropped, 0, "每帧重新算");
    }

    #[test]
    fn despawning_waits_for_the_sub_emitters() {
        let mut app = app();
        let mut e = ParticleEmitter3d { lifetime: (0.05, 0.05), ..ParticleEmitter3d::burst(2) };
        e.sub_emitters = vec![SubEmitter3d { on: SubTrigger::Death, count: 3, inherit: 0.0, emitter: ParticleEmitter3d { lifetime: (0.3, 0.3), ..Default::default() } }];
        let id = app.world_mut().spawn(e).id();
        for _ in 0..3 {
            app.tick(VIEWPORT, 0.05);
        }
        assert_eq!(app.world().get::<Particles3d>(id).unwrap().iter().count(), 0, "自己的死完了");
        assert_eq!(app.world().get::<Particles3d>(id).unwrap().len(), 6);
        assert_eq!(app.world().resource::<Billboards>().quads(), 6, "子发射器的也画");
        for _ in 0..8 {
            app.tick(VIEWPORT, 0.05);
        }
        assert!(app.world().get_entity(id).is_err(), "子发射器的也死完才删");
    }

    #[test]
    fn particles_hit_scene_colliders() {
        use nge_physics3d::{Collider, Physics3dPlugin, RigidBody};
        let mut app = app();
        app.add_plugins(Physics3dPlugin::default());
        // 顶面在 y = 0 的固定盒子
        app.world_mut().spawn((RigidBody::Fixed, Collider::cuboid(Vec3::new(10.0, 1.0, 10.0)), Transform3d::from_xyz(0.0, -0.5, 0.0)));
        let collision = ParticleCollision { scene: true, bounce: 0.0, ..Default::default() };
        let e = ParticleEmitter3d { speed: (5.0, 5.0), direction: Vec3::NEG_Y, spread: 0.0, lifetime: (10.0, 10.0), collision: Some(collision), ..ParticleEmitter3d::burst(5) };
        let id = app.world_mut().spawn((e, Transform3d::from_xyz(0.0, 1.0, 0.0))).id();
        for _ in 0..30 {
            app.tick(VIEWPORT, 1.0 / 60.0);
        }
        let particles = app.world().get::<Particles3d>(id).unwrap();
        assert_eq!(particles.len(), 5);
        assert!(particles.iter().all(|p| p.position.y >= 0.0 && p.position.y < 0.05 && p.velocity.y.abs() < 1e-4), "停在盒子顶上 {:?}", particles.iter().next());
        assert!(app.world().resource::<ParticleStats>().queries > 0);
    }

    #[test]
    fn mesh_shapes_wait_for_the_model_and_emit_from_its_surface() {
        use nge_render3d::{Assets3d, MaterialId, Mesh, Model, Part};
        let mut app = app();
        let model = {
            let mut assets = app.world_mut().resource_mut::<Assets3d>();
            let mesh = assets.add_mesh(Mesh::plane(2.0)).unwrap();
            assets.add_model(Model { parts: vec![Part::new(mesh, MaterialId::DEFAULT, glam::Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0)))], ..Default::default() }).unwrap()
        };
        let shape = EmitShape::Mesh(crate::MeshSource::Model { model, scale: Vec3::splat(2.0) });
        let id = app.world_mut().spawn(ParticleEmitter3d { shape, speed: (0.0, 0.0), lifetime: (10.0, 10.0), ..ParticleEmitter3d::burst(50) }).id();
        app.tick(VIEWPORT, 0.0);
        let particles = app.world().get::<Particles3d>(id).unwrap();
        assert_eq!(particles.len(), 50);
        // 平面边长 2、挪到 x = 5、再放大 2 倍：x 在 8 … 12，z 在 -2 … 2
        assert!(particles.iter().all(|p| (8.0 - 1e-4..=12.0 + 1e-4).contains(&p.position.x) && p.position.z.abs() <= 2.0 + 1e-4 && p.position.y.abs() < 1e-4), "{:?}", particles.iter().next());
    }

    #[test]
    fn mesh_bursts_wait_for_the_model_and_the_cache_follows_the_emitters() {
        use nge_render3d::{Assets3d, MaterialId, Mesh, Model, Part};
        let mut app = app();
        // 还没加进来的模型（编号 0）：一直等，不删、不缓存成失败
        let model = Assets3d::default().add_model(Model::default()).unwrap();
        let shape = EmitShape::Mesh(crate::MeshSource::Model { model, scale: Vec3::ONE });
        let id = app.world_mut().spawn(ParticleEmitter3d { shape, speed: (0.0, 0.0), lifetime: (10.0, 10.0), ..ParticleEmitter3d::burst(20) }).id();
        for _ in 0..3 {
            app.tick(VIEWPORT, 0.1);
        }
        assert!(app.world().get_entity(id).is_ok(), "网格没好时不算放完");
        assert_eq!(app.world().resource::<MeshSurfaces>().len(), 0);
        {
            let mut assets = app.world_mut().resource_mut::<Assets3d>();
            let mesh = assets.add_mesh(Mesh::plane(2.0)).unwrap();
            assets.add_model(Model { parts: vec![Part::new(mesh, MaterialId::DEFAULT, glam::Mat4::IDENTITY)], ..Default::default() }).unwrap();
        }
        app.tick(VIEWPORT, 0.1);
        assert_eq!(app.world().get::<Particles3d>(id).unwrap().len(), 20, "模型加进来以后发出去");
        assert_eq!(app.world().resource::<MeshSurfaces>().len(), 1);
        // 没有三角形的模型：放弃，一次性的照样删
        let empty = app.world_mut().resource_mut::<Assets3d>().add_model(Model::default()).unwrap();
        let shape = EmitShape::Mesh(crate::MeshSource::Model { model: empty, scale: Vec3::ONE });
        let gone = app.world_mut().spawn(ParticleEmitter3d { shape, ..ParticleEmitter3d::burst(20) }).id();
        app.tick(VIEWPORT, 0.1);
        app.tick(VIEWPORT, 0.1);
        assert!(app.world().get_entity(gone).is_err(), "没有三角形的不等");
        // 发射器都没了：缓存清掉
        app.world_mut().despawn(id);
        app.tick(VIEWPORT, 0.1);
        assert_eq!(app.world().resource::<MeshSurfaces>().len(), 0);
    }

    #[test]
    fn scene_collisions_skip_their_group_and_colliders_they_start_in() {
        use nge_physics3d::{Collider, Physics3dPlugin, RigidBody};
        let mut app = app();
        app.add_plugins(Physics3dPlugin::default());
        // 顶面在 y = 0 的地板（分组 3），和一个包住 (20, 10) 处出生点的盒子
        app.world_mut().spawn((RigidBody::Fixed, Collider { group: 3, ..Collider::cuboid(Vec3::new(100.0, 1.0, 100.0)) }, Transform3d::from_xyz(0.0, -0.5, 0.0)));
        app.world_mut().spawn((RigidBody::Fixed, Collider::cuboid(Vec3::splat(4.0)), Transform3d::from_xyz(20.0, 10.0, 0.0)));
        let falling = |group: u8, x: f32, y: f32| {
            let collision = ParticleCollision { scene: true, bounce: 0.0, group, ..Default::default() };
            (ParticleEmitter3d { speed: (5.0, 5.0), direction: Vec3::NEG_Y, spread: 0.0, lifetime: (10.0, 10.0), collision: Some(collision), ..ParticleEmitter3d::burst(1) }, Transform3d::from_xyz(x, y, 0.0))
        };
        let (hits, skips, inside) = (app.world_mut().spawn(falling(0, 0.0, 1.0)).id(), app.world_mut().spawn(falling(3, 0.0, 1.0)).id(), app.world_mut().spawn(falling(0, 20.0, 10.0)).id());
        for _ in 0..180 {
            app.tick(VIEWPORT, 1.0 / 60.0);
        }
        let y = |app: &App, id| app.world().get::<Particles3d>(id).unwrap().iter().next().unwrap().position.y;
        assert!((0.0..0.05).contains(&y(&app, hits)), "落在地板上");
        assert!(y(&app, skips) < -1.0, "同一组的穿过去");
        assert!((0.0..0.05).contains(&y(&app, inside)), "出生在盒子里的不被盒子挡，穿出去落到地板上：{}", y(&app, inside));
    }

    #[test]
    #[should_panic(expected = "Render3dPlugin")]
    fn plugin_needs_render3d() {
        App::new().add_plugins(Particles3dPlugin);
    }
}
