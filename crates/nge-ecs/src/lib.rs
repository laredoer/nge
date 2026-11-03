//! ECS 层：把 `nge_core::Game` 的回调接进 bevy_ecs。
//!
//! 每帧顺序：NSH 发来的 command / touch → `PreUpdate` → `Update` → `PostUpdate`
//! → Outbox 统一 emit → 清掉输入的边沿状态。渲染由插件注册的 [`Renderer`] 完成。

mod app;
mod input;
mod resources;

pub use app::{App, Plugin, PostUpdate, PreUpdate, Renderer, Startup, Update, factory};
pub use input::{Input, Pointer};
pub use resources::{NshCommand, Outbox, Props, Screen, Time};

pub use bevy_ecs;
pub use glam;

pub mod prelude {
    pub use crate::{
        App, Input, NshCommand, Outbox, Plugin, Pointer, PostUpdate, PreUpdate, Props, Screen,
        Startup, Time, Update,
    };
    pub use bevy_ecs::prelude::*;
    pub use glam::{Vec2, Vec4, vec2};
    pub use serde_json::json;
}
