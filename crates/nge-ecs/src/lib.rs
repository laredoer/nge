//! ECS 层：把 `nge_core::Game` 的回调接进 bevy_ecs。
//!
//! 每帧顺序：NSH 发来的 command / touch → `PreUpdate` → `Update` → `PostUpdate`
//! → 推送变化了的 State → Outbox 统一 emit → 清掉输入的边沿状态。
//!
//! 和 NSH 的类型化通道见 `nge_core::bridge`：[`App::add_prop`]、[`App::add_command`]、
//! [`App::add_state`]、[`Outbox::send`]。渲染由插件注册的 [`Renderer`] 完成。

mod app;
mod bridge;
mod input;
mod render;
mod resources;

pub use app::{App, Plugin, PostUpdate, PreUpdate, Renderer, Startup, Update, factory};
pub use input::{Input, Pointer};
pub use render::{ClearColor, RenderFrame};
pub use resources::{NshCommand, Outbox, Props, Screen, Time};

pub use nge_core::bridge::{CommandKey, EventKey, PropKey, StateKey};

pub use bevy_ecs;
pub use glam;

pub mod prelude {
    pub use crate::{
        App, ClearColor, CommandKey, EventKey, Input, NshCommand, PropKey, StateKey, Outbox, Plugin, Pointer, PostUpdate, PreUpdate, Props, Screen,
        Startup, Time, Update,
    };
    pub use bevy_ecs::prelude::*;
    pub use glam::{Vec2, Vec4, vec2};
    pub use serde_json::json;
}
