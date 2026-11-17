//! 动画状态机：状态、转换、混合树、分层、动画事件。和骨骼无关：只输出「每层在播哪些片段、各自的权重和时间」，
//! 3D（`nge-render3d`）按它混姿势，2D（`nge-scene`）按它选精灵帧（见 docs/anim-graph.md）。

mod blend;
mod def;
mod events;
mod graph;

use bevy_ecs::prelude::*;

pub use blend::{blend1d, blend2d};
pub use def::{
    AtDef, BlendClipDef, BlendDef, Clip, ClipDef, EventDef, Graph, GraphDef, Layer, LayerDef, LayerMode, OneOrMany, ParamDef, ParamKind, ParamValue, StateDef, TransitionDef, WeightDef,
};
pub use events::{MAX_LOOPS, crossed};
pub use graph::{AnimGraph, ClipSample, Fired, LayerOutput, MAX_INSTANCES};

/// 动画事件（ECS 消息）：`entity` 上第 `layer` 层的 `state` 状态里，片段 `clip` 播到了事件 `name`。
#[derive(Message, Debug, Clone, PartialEq)]
pub struct AnimEvent {
    pub entity: Entity,
    pub layer: String,
    pub state: String,
    pub clip: String,
    pub name: String,
}

impl AnimEvent {
    pub fn new(entity: Entity, fired: Fired) -> Self {
        Self { entity, layer: fired.layer, state: fired.state, clip: fired.clip, name: fired.name }
    }
}

/// 推进状态机的系统（3D、2D）都在这个集里；要读这一帧的 [`AnimEvent`] 的系统排在它后面。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Animated;

#[cfg(test)]
mod tests;
