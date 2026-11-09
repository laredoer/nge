//! 3D 粒子发射器（见 docs/particles3d.md）：跟着实体的 `Transform3d`，粒子画成朝着相机的方片。

use bevy_ecs::prelude::*;
use glam::{Quat, Vec3, Vec4};
use nge_assets::ImageId;
use nge_ecs::Time;
use nge_render3d::{Billboard, Billboards, Blend, Transform3d};

use crate::emitter::DEFAULT_MAX_PARTICLES;
use crate::rng::Rng;

/// 3D 粒子发射器。范围参数写成 `(最小, 最大)`；`size` / `color` 是 `(出生, 死亡)`，中间线性插值。
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
    /// 线性 RGBA。
    pub color: (Vec4, Vec4),
    pub texture: ImageId,
    pub blend: Blend,
    /// 同时存活的上限，到了就不再发射。
    pub max: usize,
    /// 不再持续发射（`rate` 为 0）且粒子全部死完后删除实体。
    pub despawn_when_done: bool,
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
            color: (Vec4::ONE, Vec4::new(1.0, 1.0, 1.0, 0.0)),
            texture: ImageId::WHITE,
            blend: Blend::Alpha,
            max: DEFAULT_MAX_PARTICLES,
            despawn_when_done: false,
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
}

/// 发射器的运行状态，随 [`ParticleEmitter3d`] 自动加上。
#[derive(Component, Debug, Clone)]
pub struct Particles3d {
    particles: Vec<Particle3d>,
    pending: f32,
    burst_done: bool,
    warned: bool,
    rng: Rng,
}

impl Default for Particles3d {
    fn default() -> Self {
        static SEED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x2545_F491);
        let seed = SEED.fetch_add(0x6D2B_79F5, std::sync::atomic::Ordering::Relaxed);
        Self { particles: Vec::new(), pending: 0.0, burst_done: false, warned: false, rng: Rng::new(seed) }
    }
}

/// 在以 `axis` 为轴、半角 `spread` 的锥形里按立体角均匀取一个方向；`axis` 是零向量时当成 +Y。
fn cone(rng: &mut Rng, axis: Vec3, spread: f32) -> Vec3 {
    // NaN 当成全方向
    let spread = if spread.is_nan() { std::f32::consts::PI } else { spread.clamp(0.0, std::f32::consts::PI) };
    // cosθ 在 [cos(spread), 1] 里均匀取，立体角就是均匀的
    let cos = 1.0 - rng.next_f32() * (1.0 - spread.cos());
    let sin = (1.0 - cos * cos).max(0.0).sqrt();
    let phi = rng.next_f32() * std::f32::consts::TAU;
    let local = Vec3::new(sin * phi.cos(), cos, sin * phi.sin());
    let axis = axis.try_normalize().unwrap_or(Vec3::Y);
    Quat::from_rotation_arc(Vec3::Y, axis) * local
}

impl Particles3d {
    pub fn len(&self) -> usize {
        self.particles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Particle3d> {
        self.particles.iter()
    }

    /// 推进 `dt` 秒：老化、移动、发射新粒子。`rotation` 是发射器的朝向。
    fn step(&mut self, emitter: &ParticleEmitter3d, origin: Vec3, rotation: Quat, dt: f32) {
        self.particles.retain_mut(|p| {
            p.age += dt;
            p.velocity += emitter.gravity * dt;
            p.position += p.velocity * dt;
            p.rotation += p.spin * dt;
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
                log::warn!("3D 粒子发射器到了上限 {}，多出的不再发射", emitter.max);
            }
            count = room;
        }
        let axis = rotation * emitter.direction;
        self.particles.reserve(count);
        for _ in 0..count {
            let direction = cone(&mut self.rng, axis, emitter.spread);
            let speed = self.rng.range(emitter.speed);
            let spin = self.rng.range(emitter.spin);
            let rotation = self.rng.next_f32() * std::f32::consts::TAU;
            let lifetime = self.rng.range(emitter.lifetime).max(f32::EPSILON);
            self.particles.push(Particle3d { position: origin, velocity: direction * speed, rotation, spin, age: 0.0, lifetime });
        }
    }

    fn finished(&self, emitter: &ParticleEmitter3d) -> bool {
        self.burst_done && emitter.rate <= 0.0 && self.particles.is_empty()
    }

    fn billboards<'a>(&'a self, emitter: &'a ParticleEmitter3d) -> impl Iterator<Item = Billboard> + 'a {
        let (size0, size1) = emitter.size;
        let (color0, color1) = emitter.color;
        self.particles.iter().map(move |p| {
            let t = (p.age / p.lifetime).clamp(0.0, 1.0);
            Billboard { position: p.position, size: size0 + (size1 - size0) * t, rotation: p.rotation, color: color0.lerp(color1, t) }
        })
    }
}

pub(crate) fn simulate3d(
    mut commands: Commands,
    time: Res<Time>,
    mut billboards: ResMut<Billboards>,
    mut emitters: Query<(Entity, &ParticleEmitter3d, &mut Particles3d, Option<&Transform3d>)>,
) {
    for (entity, emitter, mut particles, transform) in &mut emitters {
        let transform = transform.copied().unwrap_or_default();
        particles.step(emitter, transform.translation, transform.rotation, time.delta);
        if emitter.despawn_when_done && particles.finished(emitter) {
            commands.entity(entity).despawn();
            continue;
        }
        billboards.extend(emitter.texture, emitter.blend, particles.billboards(emitter));
    }
}

#[cfg(test)]
mod tests {
    use nge_core::Viewport;
    use nge_ecs::App;
    use nge_render3d::Render3dPlugin;

    use super::*;
    use crate::Particles3dPlugin;

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
        particles.step(&emitter, Vec3::new(1.0, 1.0, 1.0), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), 0.0);
        let start = particles.iter().next().unwrap().rotation;
        particles.step(&emitter, Vec3::ZERO, Quat::IDENTITY, 1.0);
        let p = particles.iter().next().unwrap();
        assert!((p.position - Vec3::new(1.0, -3.0, -1.0)).length() < 1e-4, "{}", p.position);
        assert!((p.rotation - start - 1.0).abs() < 1e-5);
        let b = particles.billboards(&emitter).next().unwrap();
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
    #[should_panic(expected = "Render3dPlugin")]
    fn plugin_needs_render3d() {
        App::new().add_plugins(Particles3dPlugin);
    }
}
