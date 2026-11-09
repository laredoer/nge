//! 2D 粒子：发射器跟着实体的 `Transform2d`，粒子在 CPU 上模拟（存在发射器自己的数组里），
//! 每帧打成 [`SpriteInstances`] 交给 2D 批量绘制。
//! 3D 粒子和拖尾（见 docs/particles3d.md）：跟着 `Transform3d`，每帧打成 `Billboards` 交给 3D 渲染器。

mod emitter;
mod emitter3d;
mod rng;
mod trail;

pub use emitter::{DEFAULT_MAX_PARTICLES, Particle, ParticleEmitter, Particles};
pub use emitter3d::{Particle3d, ParticleEmitter3d, Particles3d};
pub use trail::{Trail3d, TrailPoints};

use bevy_ecs::prelude::*;
use nge_ecs::{App, Plugin, PostUpdate};
use nge_render2d::SpriteInstances;

/// 注册粒子模拟（PostUpdate，排在 [`ParticlesSimulated`] 里）。要放在 `Render2dPlugin` 之后。
pub struct ParticlePlugin;

impl Plugin for ParticlePlugin {
    fn build(&self, app: &mut App) {
        // SpriteInstances 每帧由 Render2dPlugin 清空，没有它会越攒越多
        assert!(app.world().contains_resource::<SpriteInstances>(), "ParticlePlugin 要加在 Render2dPlugin 之后");
        app.add_systems(PostUpdate, emitter::simulate.in_set(ParticlesSimulated));
    }
}

/// 粒子模拟系统。要在同一帧里读粒子数的系统排在它后面。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ParticlesSimulated;

/// 注册 3D 粒子和拖尾（PostUpdate，排在 [`Particles3dSimulated`] 里；改 `Transform3d` 的系统要排在它前面）。要放在 `Render3dPlugin` 之后。
pub struct Particles3dPlugin;

impl Plugin for Particles3dPlugin {
    fn build(&self, app: &mut App) {
        // Billboards 每帧由 Render3dPlugin 清空，没有它会越攒越多
        assert!(app.world().contains_resource::<nge_render3d::Billboards>(), "Particles3dPlugin 要加在 Render3dPlugin 之后");
        app.add_systems(PostUpdate, (emitter3d::simulate3d, trail::trails).in_set(Particles3dSimulated));
    }
}

/// 3D 粒子和拖尾的系统：读实体这一帧最后的 `Transform3d`，物理、跟随这些改位置的系统要排在它前面。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Particles3dSimulated;

pub mod prelude {
    pub use crate::{ParticleEmitter, ParticleEmitter3d, ParticlePlugin, Particles, Particles3dPlugin, Trail3d};
}
