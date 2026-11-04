//! 2D 粒子：发射器跟着实体的 `Transform2d`，粒子在 CPU 上模拟（存在发射器自己的数组里），
//! 每帧打成 [`SpriteInstances`] 交给 2D 批量绘制。

mod emitter;
mod rng;

pub use emitter::{DEFAULT_MAX_PARTICLES, Particle, ParticleEmitter, Particles};

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

pub mod prelude {
    pub use crate::{ParticleEmitter, ParticlePlugin, Particles};
}
