//! 内置资源：时间、屏幕、NSH 的 prop / command 和发回 NSH 的事件。

use std::collections::HashMap;

use bevy_ecs::prelude::{Message, Resource};
use glam::Vec2;
use nge_core::EventKey;
use serde::Serialize;
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

/// NSH 设置的所有 prop 的原始值；注册过的 prop 另外会成为类型化资源。
#[derive(Resource, Debug, Default)]
pub struct Props(pub HashMap<String, JsonValue>);

impl Props {
    pub fn get(&self, name: &str) -> Option<&JsonValue> {
        self.0.get(name)
    }
}

/// 没有用 `App::add_command` 注册过的 command，原样透传；注册过的只会变成类型化消息，不会再出现在这里（触摸进 [`crate::Input`]）。
#[derive(Message, Debug, Clone)]
pub struct NshCommand {
    pub name: String,
    pub args: JsonValue,
}

/// 发回 NSH 的事件，每帧末尾统一 emit（NSH 侧 `on_native_event` 收到）。
#[derive(Resource, Debug, Default)]
pub struct Outbox(Vec<(String, JsonValue)>);

impl Outbox {
    /// 发一个类型化事件（NSH 侧用 `.on_game_event(key, ..)` 接收）。
    pub fn send<T: Serialize>(&mut self, key: EventKey<T>, value: &T) {
        self.0.push((key.name().to_owned(), key.encode(value)));
    }

    /// 发一个未类型化的事件。
    pub fn emit(&mut self, name: impl Into<String>, data: JsonValue) {
        self.0.push((name.into(), data));
    }

    pub(crate) fn drain(&mut self) -> std::vec::Drain<'_, (String, JsonValue)> {
        self.0.drain(..)
    }
}
