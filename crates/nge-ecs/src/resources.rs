//! 内置资源：时间、屏幕、NSH 的 prop / command 和发回 NSH 的事件。

use std::collections::HashMap;

use bevy_ecs::prelude::{Message, Resource};
use glam::Vec2;
use serde_json::Value as JsonValue;

/// 帧时间。`delta` 已被引擎截断到 50ms 以内。
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct Time {
    pub delta: f32,
    pub elapsed: f64,
    pub frame: u64,
}

/// 游戏视图的大小（point）和像素密度。
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    pub size: Vec2,
    pub scale: f32,
}

impl Default for Screen {
    fn default() -> Self {
        Self { size: Vec2::ZERO, scale: 1.0 }
    }
}

/// NSH 设置的 prop（`native(..).prop(name, value)`）。
#[derive(Resource, Debug, Default)]
pub struct Props(pub HashMap<String, JsonValue>);

impl Props {
    pub fn get(&self, name: &str) -> Option<&JsonValue> {
        self.0.get(name)
    }
}

/// NSH 发来的 command（`ElementRef::command`），触摸除外（进 [`crate::Input`]）。
#[derive(Message, Debug, Clone)]
pub struct NshCommand {
    pub name: String,
    pub args: JsonValue,
}

/// 发回 NSH 的事件，每帧末尾统一 emit（NSH 侧 `on_native_event` 收到）。
#[derive(Resource, Debug, Default)]
pub struct Outbox(Vec<(String, JsonValue)>);

impl Outbox {
    pub fn emit(&mut self, name: impl Into<String>, data: JsonValue) {
        self.0.push((name.into(), data));
    }

    pub(crate) fn drain(&mut self) -> std::vec::Drain<'_, (String, JsonValue)> {
        self.0.drain(..)
    }
}
