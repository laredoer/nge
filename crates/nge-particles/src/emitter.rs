//! 发射器组件和模拟。

use bevy_ecs::prelude::*;
use glam::{Vec2, Vec4};
use nge_ecs::Time;
use nge_render2d::{SpriteImage, SpriteInstance, SpriteInstances, TextureId, Transform2d};

use crate::rng::Rng;

/// 每个发射器默认最多同时存活的粒子数。
pub const DEFAULT_MAX_PARTICLES: usize = 10_000;

/// 粒子发射器。范围参数写成 `(最小, 最大)`，每个粒子在范围里随机取；`size` / `color` 是 `(出生, 死亡)`，中间线性插值。
#[derive(Component, Debug, Clone, PartialEq)]
#[require(Particles)]
pub struct ParticleEmitter {
    /// 每秒持续发射的数量。
    pub rate: f32,
    /// 第一帧一次性发射的数量（爆炸）。
    pub burst: u32,
    /// 寿命（秒）。
    pub lifetime: (f32, f32),
    /// 初速度（point / 秒）。
    pub speed: (f32, f32),
    /// 发射方向（弧度，0 朝右、逆时针）。
    pub angle: (f32, f32),
    /// 加速度（point / 秒²，y 朝上）。
    pub gravity: Vec2,
    pub size: (f32, f32),
    /// 线性 RGBA。
    pub color: (Vec4, Vec4),
    pub texture: SpriteImage,
    /// 同时存活的上限，到了就不再发射。
    pub max: usize,
    /// 不再持续发射（`rate` 为 0）且粒子全部死完后删除实体。
    pub despawn_when_done: bool,
}

impl Default for ParticleEmitter {
    fn default() -> Self {
        Self {
            rate: 0.0,
            burst: 0,
            lifetime: (1.0, 1.0),
            speed: (50.0, 100.0),
            angle: (0.0, std::f32::consts::TAU),
            gravity: Vec2::ZERO,
            size: (8.0, 0.0),
            color: (Vec4::ONE, Vec4::new(1.0, 1.0, 1.0, 0.0)),
            texture: SpriteImage::Texture(TextureId::WHITE),
            max: DEFAULT_MAX_PARTICLES,
            despawn_when_done: false,
        }
    }
}

impl ParticleEmitter {
    /// 一次性爆炸：发射 `count` 个，死完后删除实体。
    pub fn burst(count: u32) -> Self {
        Self { burst: count, despawn_when_done: true, ..Default::default() }
    }

    /// 持续发射：每秒 `rate` 个，实体一直保留。
    pub fn continuous(rate: f32) -> Self {
        Self { rate, ..Default::default() }
    }
}

/// 一个存活的粒子（世界坐标，发射后不再跟着发射器走）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    pub position: Vec2,
    pub velocity: Vec2,
    pub age: f32,
    pub lifetime: f32,
}

/// 发射器的运行状态，随 [`ParticleEmitter`] 自动加上。
#[derive(Component, Debug, Clone)]
pub struct Particles {
    particles: Vec<Particle>,
    /// 持续发射攒下的零头。
    pending: f32,
    burst_done: bool,
    warned: bool,
    rng: Rng,
}

impl Default for Particles {
    fn default() -> Self {
        static SEED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x9E37_79B9);
        let seed = SEED.fetch_add(0x6D2B_79F5, std::sync::atomic::Ordering::Relaxed);
        Self { particles: Vec::new(), pending: 0.0, burst_done: false, warned: false, rng: Rng::new(seed) }
    }
}

impl Particles {
    pub fn len(&self) -> usize {
        self.particles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Particle> {
        self.particles.iter()
    }

    /// 推进 `dt` 秒：老化、移动、发射新粒子。
    fn step(&mut self, emitter: &ParticleEmitter, origin: Vec2, dt: f32) {
        self.particles.retain_mut(|p| {
            p.age += dt;
            p.velocity += emitter.gravity * dt;
            p.position += p.velocity * dt;
            p.age < p.lifetime
        });
        let mut count = 0;
        if !self.burst_done {
            self.burst_done = true;
            count += emitter.burst as usize;
        }
        self.pending += emitter.rate.max(0.0) * dt;
        let whole = self.pending.floor();
        self.pending -= whole;
        count += whole as usize;

        let room = emitter.max.saturating_sub(self.particles.len());
        if count > room {
            if !self.warned {
                self.warned = true;
                log::warn!("粒子发射器到了上限 {}，多出的不再发射", emitter.max);
            }
            count = room;
        }
        self.particles.reserve(count);
        for _ in 0..count {
            let angle = self.rng.range(emitter.angle);
            let speed = self.rng.range(emitter.speed);
            let lifetime = self.rng.range(emitter.lifetime).max(f32::EPSILON);
            self.particles.push(Particle { position: origin, velocity: Vec2::from_angle(angle) * speed, age: 0.0, lifetime });
        }
    }

    fn finished(&self, emitter: &ParticleEmitter) -> bool {
        self.burst_done && emitter.rate <= 0.0 && self.particles.is_empty()
    }

    fn instances<'a>(&'a self, emitter: &'a ParticleEmitter, z: f32) -> impl Iterator<Item = SpriteInstance> + 'a {
        let (size0, size1) = emitter.size;
        let (color0, color1) = emitter.color;
        self.particles.iter().map(move |p| {
            let t = (p.age / p.lifetime).clamp(0.0, 1.0);
            SpriteInstance {
                position: p.position,
                size: Vec2::splat(size0 + (size1 - size0) * t),
                rotation: 0.0,
                z,
                color: color0.lerp(color1, t),
            }
        })
    }
}

pub(crate) fn simulate(
    mut commands: Commands,
    time: Res<Time>,
    mut instances: ResMut<SpriteInstances>,
    mut emitters: Query<(Entity, &ParticleEmitter, &mut Particles, Option<&Transform2d>)>,
) {
    for (entity, emitter, mut particles, transform) in &mut emitters {
        let transform = transform.copied().unwrap_or_default();
        particles.step(emitter, transform.translation, time.delta);
        if emitter.despawn_when_done && particles.finished(emitter) {
            commands.entity(entity).despawn();
            continue;
        }
        instances.extend(emitter.texture, particles.instances(emitter, transform.z));
    }
}

#[cfg(test)]
mod tests {
    use nge_core::Viewport;
    use nge_ecs::App;
    use nge_render2d::Render2dPlugin;

    use super::*;
    use crate::ParticlePlugin;

    const VIEWPORT: Viewport = Viewport { size: [100.0, 100.0], pixels: [100, 100] };

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(Render2dPlugin).add_plugins(ParticlePlugin);
        app
    }

    fn alive(app: &mut App) -> usize {
        app.world_mut().query::<&Particles>().iter(app.world()).map(Particles::len).sum()
    }

    #[test]
    fn burst_emits_once_and_is_drawn() {
        let mut app = app();
        app.world_mut().spawn((ParticleEmitter { lifetime: (10.0, 10.0), ..ParticleEmitter::burst(100) }, Transform2d::default()));
        app.tick(VIEWPORT, 0.016);
        assert_eq!(alive(&mut app), 100);
        assert_eq!(app.world().resource::<SpriteInstances>().len(), 100);
        app.tick(VIEWPORT, 0.016);
        assert_eq!(alive(&mut app), 100, "只在第一帧发射");
    }

    #[test]
    fn rate_accumulates_fractions() {
        let mut app = app();
        app.world_mut().spawn(ParticleEmitter { lifetime: (10.0, 10.0), ..ParticleEmitter::continuous(30.0) });
        for _ in 0..10 {
            app.tick(VIEWPORT, 0.05);
        }
        assert_eq!(alive(&mut app), 15, "30/秒 × 0.5 秒");
    }

    #[test]
    fn particles_die_and_burst_entity_is_despawned() {
        let mut app = app();
        let entity = app.world_mut().spawn(ParticleEmitter { lifetime: (0.1, 0.1), ..ParticleEmitter::burst(10) }).id();
        let kept = app
            .world_mut()
            .spawn(ParticleEmitter { lifetime: (0.1, 0.1), despawn_when_done: false, ..ParticleEmitter::burst(10) })
            .id();
        app.tick(VIEWPORT, 0.05);
        assert_eq!(alive(&mut app), 20);
        app.tick(VIEWPORT, 0.05);
        app.tick(VIEWPORT, 0.05);
        assert_eq!(alive(&mut app), 0);
        assert!(app.world().get_entity(entity).is_err(), "爆炸发射器死完后删除");
        assert!(app.world().get_entity(kept).is_ok(), "关掉自动删除时保留");
        assert!(app.world().resource::<SpriteInstances>().is_empty());
    }

    #[test]
    fn default_emitter_waits_to_be_configured() {
        let mut app = app();
        let entity = app.world_mut().spawn(ParticleEmitter::default()).id();
        app.tick(VIEWPORT, 0.016);
        assert!(app.world().get_entity(entity).is_ok(), "默认不自动删除，之后再设 rate");
    }

    #[test]
    #[should_panic(expected = "Render2dPlugin")]
    fn plugin_needs_render2d() {
        App::new().add_plugins(ParticlePlugin);
    }

    #[test]
    fn cap_limits_live_particles() {
        let mut app = app();
        app.world_mut().spawn(ParticleEmitter { lifetime: (10.0, 10.0), rate: 1e6, ..ParticleEmitter::burst(20_000) });
        app.tick(VIEWPORT, 0.016);
        assert_eq!(alive(&mut app), DEFAULT_MAX_PARTICLES);
        app.tick(VIEWPORT, 0.016);
        assert_eq!(alive(&mut app), DEFAULT_MAX_PARTICLES);
        let small = app.world_mut().spawn(ParticleEmitter { max: 5, ..ParticleEmitter::burst(50) }).id();
        app.tick(VIEWPORT, 0.016);
        assert_eq!(app.world().get::<Particles>(small).unwrap().len(), 5);
    }

    #[test]
    fn motion_gravity_and_interpolation() {
        let mut particles = Particles::default();
        let emitter = ParticleEmitter {
            speed: (10.0, 10.0),
            angle: (0.0, 0.0),
            gravity: Vec2::new(0.0, -20.0),
            lifetime: (2.0, 2.0),
            size: (10.0, 0.0),
            color: (Vec4::ONE, Vec4::ZERO),
            ..ParticleEmitter::burst(1)
        };
        particles.step(&emitter, Vec2::new(5.0, 5.0), 0.0);
        particles.step(&emitter, Vec2::ZERO, 1.0);
        let p = particles.iter().next().unwrap();
        assert!((p.position - Vec2::new(15.0, -15.0)).length() < 1e-4, "{}", p.position);
        let sprite = particles.instances(&emitter, 3.0).next().unwrap();
        assert_eq!(sprite.size, Vec2::splat(5.0));
        assert_eq!(sprite.color, Vec4::splat(0.5));
        assert_eq!(sprite.z, 3.0);
    }

    #[test]
    fn emitter_position_is_the_origin() {
        let mut app = app();
        app.world_mut().spawn((ParticleEmitter { speed: (0.0, 0.0), ..ParticleEmitter::burst(3) }, Transform2d::from_xy(7.0, -3.0)));
        app.tick(VIEWPORT, 0.016);
        let instances = app.world().resource::<SpriteInstances>();
        assert_eq!(instances.len(), 3);
        let mut particles = app.world_mut().query::<&Particles>();
        assert!(particles.single(app.world()).unwrap().iter().all(|p| p.position == Vec2::new(7.0, -3.0)));
    }
}
