//! NSH 和游戏之间的类型化通道。key 是带类型的名字，两边共用同一个 Rust 类型，传输格式是 JSON。
//!
//! | 方向 | 持续值 | 一次性 |
//! |---|---|---|
//! | NSH → 游戏 | [`PropKey`] | [`CommandKey`] |
//! | 游戏 → NSH | [`StateKey`] | [`EventKey`] |

use std::fmt;
use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::stats::FrameReport;

/// 游戏视图在平台层注册的组件名。
pub const GAME_COMPONENT: &str = "nge.game";

/// 内置：触摸，`game_view()` 自动转发。
pub const TOUCH: CommandKey<Touch> = CommandKey::new("touch");
/// 内置：键盘（Mac），平台层自己转发给游戏。
pub const KEY: CommandKey<KeyInput> = CommandKey::new("key");
/// 内置：同一次按键也发给 NSH（TSX 的 `onKey`）。
pub const KEY_EVENT: EventKey<KeyInput> = EventKey::new("key");
/// 内置：每秒一次的帧率和最长帧间隔。
pub const FRAME_STATS: EventKey<FrameReport> = EventKey::new("fps");
/// 内置：渲染线程出错（初始化失败、panic、surface 丢失）。`game_view()` 默认记日志。
pub const ENGINE_ERROR: EventKey<EngineError> = EventKey::new("error");
/// 引擎 / 平台自用的 command 前缀（如 `nge.stall_main`）。
pub const RESERVED_COMMAND_PREFIX: &str = "nge.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineError {
    pub message: String,
}

/// 游戏不能用来注册的通道名：`outbound` 为 true 指游戏 → NSH（State / Event），否则指 command。
pub fn is_reserved(name: &str, outbound: bool) -> bool {
    if outbound {
        name == FRAME_STATS.name() || name == ENGINE_ERROR.name() || name == KEY_EVENT.name()
    } else {
        name == TOUCH.name() || name == KEY.name() || name.starts_with(RESERVED_COMMAND_PREFIX)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TouchPhase {
    Start,
    Move,
    End,
    Cancel,
}

/// 一根手指的一次变化，坐标是游戏视图内的 point（左上角原点，y 朝下）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Touch {
    pub phase: TouchPhase,
    pub id: u32,
    pub x: f32,
    pub y: f32,
    /// 事件时间戳（毫秒，起点任意，即 NSH 的 `TouchEvent::timestamp`），算甩动速度用。
    #[serde(default)]
    pub t: f64,
}

/// 一次按键变化。`code` 是 Web `KeyboardEvent.code` 的键名（`KeyA`、`ArrowLeft`、`Space`），按键盘上的位置，和布局无关。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyInput {
    pub code: String,
    pub down: bool,
    /// 按住不放时系统自动重复的按下。
    #[serde(default)]
    pub repeat: bool,
}

macro_rules! key {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        pub struct $name<T> {
            name: &'static str,
            _type: PhantomData<fn() -> T>,
        }

        impl<T> $name<T> {
            pub const fn new(name: &'static str) -> Self {
                Self { name, _type: PhantomData }
            }

            pub const fn name(&self) -> &'static str {
                self.name
            }
        }

        impl<T: Serialize> $name<T> {
            pub fn encode(&self, value: &T) -> JsonValue {
                encode(self.name, value)
            }
        }

        impl<T: DeserializeOwned> $name<T> {
            pub fn decode(&self, value: &JsonValue) -> Result<T, DecodeError> {
                decode(self.name, value)
            }
        }

        impl<T> Clone for $name<T> {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl<T> Copy for $name<T> {}

        impl<T> fmt::Debug for $name<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({:?})", stringify!($name), self.name)
            }
        }
    };
}

key! {
    /// NSH → 游戏的持续值：NSH 侧绑定一个值，变了就推；游戏侧成为资源 `T`。
    PropKey
}
key! {
    /// NSH → 游戏的一次性命令：游戏侧成为消息 `T`。
    CommandKey
}
key! {
    /// 游戏 → NSH 的持续值：游戏侧资源 `T` 变了就在帧末推；NSH 侧写进 signal。
    StateKey
}
key! {
    /// 游戏 → NSH 的一次性事件。
    EventKey
}

/// 收到的数据和协议类型对不上。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    pub key: &'static str,
    pub message: String,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "通道 {} 解析失败: {}", self.key, self.message)
    }
}

impl std::error::Error for DecodeError {}

fn encode<T: Serialize>(key: &'static str, value: &T) -> JsonValue {
    // 协议类型是普通数据结构，序列化成 JSON 只会在 map 键不是字符串之类的定义错误时失败
    serde_json::to_value(value).unwrap_or_else(|e| {
        log::error!("通道 {key} 序列化失败: {e}");
        JsonValue::Null
    })
}

fn decode<T: DeserializeOwned>(key: &'static str, value: &JsonValue) -> Result<T, DecodeError> {
    T::deserialize(value).map_err(|e| DecodeError { key, message: e.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Hud {
        score: u32,
        lives: u8,
    }

    const HUD: StateKey<Hud> = StateKey::new("hud");

    #[test]
    fn roundtrip() {
        let hud = Hud { score: 12, lives: 3 };
        let wire = HUD.encode(&hud);
        assert_eq!(wire, json!({ "score": 12, "lives": 3 }));
        assert_eq!(HUD.decode(&wire), Ok(hud));
    }

    #[test]
    fn mismatch_is_an_error_naming_the_key() {
        let err = HUD.decode(&json!({ "score": "x" })).unwrap_err();
        assert_eq!(err.key, "hud");
        assert!(err.to_string().contains("hud"));
    }

    #[test]
    fn touch_wire_format_matches_nsh_phase_names() {
        let t = TOUCH.decode(&json!({ "phase": "start", "id": 2, "x": 1.5, "y": 3.0 })).unwrap();
        assert_eq!(t, Touch { phase: TouchPhase::Start, id: 2, x: 1.5, y: 3.0, t: 0.0 });
    }

    #[test]
    fn reserved_names() {
        assert!(is_reserved("fps", true) && is_reserved("error", true) && is_reserved("key", true) && !is_reserved("hud", true));
        assert!(is_reserved("touch", false) && is_reserved("key", false) && is_reserved("nge.stall_main", false) && !is_reserved("fps", false));
    }

    #[test]
    fn key_wire_format() {
        let k = KEY.decode(&json!({ "code": "ArrowLeft", "down": true })).unwrap();
        assert_eq!(k, KeyInput { code: "ArrowLeft".into(), down: true, repeat: false });
        assert_eq!(KEY_EVENT.encode(&k), json!({ "code": "ArrowLeft", "down": true, "repeat": false }));
    }

    #[test]
    fn frame_stats_wire_format_is_stable() {
        let wire = FRAME_STATS.encode(&FrameReport { fps: 120, worst_ms: 9 });
        assert_eq!(wire, json!({ "fps": 120, "worst_ms": 9 }));
        assert_eq!(FRAME_STATS.name(), "fps");
    }
}
