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
/// 内置：手柄摇杆的模拟量（Mac），平台层在值变了时发给游戏；按钮走 [`KEY`]。
pub const GAMEPAD: CommandKey<GamepadInput> = CommandKey::new("gamepad");
/// 内置（平台层发）：一个手柄的整份状态，变了才发；第一次发就是接入（见 docs/input-actions.md）。
pub const PAD: CommandKey<PadInput> = CommandKey::new("pad");
/// 内置：TSX 让手柄震动；引擎按玩家找到手柄，换成 [`RUMBLE_OUT`]。
pub const RUMBLE: CommandKey<RumbleRequest> = CommandKey::new("rumble");
/// 内置（引擎 → 平台层）：让手柄震动。平台层截下来，不交给 NSH。
pub const RUMBLE_OUT: EventKey<RumbleOut> = EventKey::new("nge.rumble");
/// 内置：动作的状态变了（按钮按下 / 松开，轴的方向变了），TSX 的 `onAction`。
pub const ACTION_EVENT: EventKey<ActionEvent> = EventKey::new("action");
/// 内置：手柄接入、拔出、换了玩家，TSX 的 `onPad`。
pub const PAD_EVENT: EventKey<PadEvent> = EventKey::new("pad");
/// 内置 prop：动作的绑定表（`nge-ecs` 的 `ActionMap`）。
pub const ACTIONS_PROP: &str = "actions";
/// 内置（平台层发）：画面没了、App 进了后台（`true`）或者回来了（`false`）。渲染线程停着时也会当场处理，
/// 游戏用 `App::on_suspend` 接（比如停掉声音）。
pub const SUSPEND: CommandKey<Suspend> = CommandKey::new("nge.suspend");
/// 内置：每秒一次的帧率和最长帧间隔。
pub const FRAME_STATS: EventKey<FrameReport> = EventKey::new("fps");
/// 内置：渲染线程出错（初始化失败、panic、surface 丢失）。`game_view()` 默认记日志。
pub const ENGINE_ERROR: EventKey<EngineError> = EventKey::new("error");
/// 内置：第一帧画出来了（web 上就是加载完了）。游戏可以等它再开始走时间。
pub const READY: EventKey<Ready> = EventKey::new("ready");
/// 引擎 / 平台自用的 command 前缀（如 `nge.stall_main`）。
pub const RESERVED_COMMAND_PREFIX: &str = "nge.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineError {
    pub message: String,
}

/// 游戏不能用来注册的通道名：`outbound` 为 true 指游戏 → NSH（State / Event），否则指 command。
pub fn is_reserved(name: &str, outbound: bool) -> bool {
    if outbound {
        [FRAME_STATS.name(), ENGINE_ERROR.name(), KEY_EVENT.name(), READY.name(), ACTION_EVENT.name(), PAD_EVENT.name()].contains(&name)
            || name.starts_with(RESERVED_COMMAND_PREFIX)
    } else {
        [TOUCH.name(), KEY.name(), GAMEPAD.name(), PAD.name(), RUMBLE.name()].contains(&name) || name.starts_with(RESERVED_COMMAND_PREFIX)
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

/// [`SUSPEND`] 的参数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suspend {
    pub suspended: bool,
}

/// 两根摇杆，x 向右、y 向上，各在 −1 … 1，已经去掉死区；两个扳机 0 … 1。几个手柄合起来的。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GamepadInput {
    pub left: [f32; 2],
    pub right: [f32; 2],
    pub l2: f32,
    pub r2: f32,
}

fn yes() -> bool {
    true
}

/// 一个手柄的整份状态：按着的键名（`GamepadA`、`GamepadLeftStickLeft`…）、去掉死区的摇杆、扳机。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PadInput {
    /// 平台层给的手柄号。
    pub id: u32,
    /// false：拔掉了，别的字段不看。
    #[serde(default = "yes")]
    pub connected: bool,
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub left: [f32; 2],
    #[serde(default)]
    pub right: [f32; 2],
    #[serde(default)]
    pub l2: f32,
    #[serde(default)]
    pub r2: f32,
}

fn rumble_duration() -> f32 {
    0.2
}

/// TSX 的 `rumble` command：第 `player` 个玩家的手柄，低频（重）、高频（轻）马达的强度 0 … 1，`duration` 秒。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RumbleRequest {
    #[serde(default)]
    pub player: usize,
    #[serde(default)]
    pub low: f32,
    #[serde(default)]
    pub high: f32,
    #[serde(default = "rumble_duration")]
    pub duration: f32,
}

/// [`RUMBLE_OUT`]：`pad` 是手柄号，`None` 是所有连着的手柄。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RumbleOut {
    pub pad: Option<u32>,
    pub low: f32,
    pub high: f32,
    pub duration: f32,
}

/// [`ACTION_EVENT`]：`down` 是按着（轴是推过一半）；`x`、`y` 是方向（−1 / 0 / 1，按钮按着时 x 是 1）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionEvent {
    pub action: String,
    pub player: usize,
    pub down: bool,
    pub x: i8,
    pub y: i8,
}

/// [`PAD_EVENT`]：`player` 是这个手柄归第几个玩家（从 0 数），没分给谁是 `None`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PadEvent {
    pub id: u32,
    pub connected: bool,
    pub player: Option<usize>,
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
    fn gamepad_wire_format() {
        let g = GAMEPAD.decode(&json!({ "left": [0.5, -1.0], "right": [0.0, 0.25] })).unwrap();
        assert_eq!(g, GamepadInput { left: [0.5, -1.0], right: [0.0, 0.25], ..Default::default() });
        assert_eq!(GAMEPAD.decode(&json!({})).unwrap(), GamepadInput::default(), "缺的摇杆是 0");
        assert!(is_reserved("gamepad", false));
        assert!(is_reserved("ready", true) && !is_reserved("ready", false));
        assert_eq!(READY.encode(&Ready {}), serde_json::json!({}));
    }

    #[test]
    fn pad_and_rumble_wire_formats() {
        let p = PAD.decode(&json!({ "id": 2, "keys": ["GamepadA"], "left": [0.5, 0.0], "l2": 0.25 })).unwrap();
        assert_eq!(p, PadInput { id: 2, connected: true, keys: vec!["GamepadA".into()], left: [0.5, 0.0], right: [0.0; 2], l2: 0.25, r2: 0.0 }, "缺的是默认值，connected 默认连着");
        assert!(!PAD.decode(&json!({ "id": 2, "connected": false })).unwrap().connected);
        assert!(PAD.decode(&json!({ "keys": [] })).is_err(), "手柄号不能缺");
        let g = GAMEPAD.decode(&json!({ "left": [0.0, 0.0], "l2": 0.75 })).unwrap();
        assert_eq!((g.l2, g.r2), (0.75, 0.0), "gamepad 带上合并后的扳机");
        let r = RUMBLE.decode(&json!({ "low": 1.0 })).unwrap();
        assert_eq!(r, RumbleRequest { player: 0, low: 1.0, high: 0.0, duration: 0.2 });
        let out = RumbleOut { pad: None, low: 0.5, high: 0.25, duration: 0.5 };
        assert_eq!(RUMBLE_OUT.encode(&out), json!({ "pad": null, "low": 0.5, "high": 0.25, "duration": 0.5 }));
        assert_eq!(RUMBLE_OUT.decode(&json!({ "pad": 3, "low": 0.5, "high": 0.25, "duration": 0.1 })).unwrap().pad, Some(3));
        let a = ActionEvent { action: "jump".into(), player: 1, down: true, x: 1, y: 0 };
        assert_eq!(ACTION_EVENT.encode(&a), json!({ "action": "jump", "player": 1, "down": true, "x": 1, "y": 0 }));
        assert_eq!(PAD_EVENT.encode(&PadEvent { id: 0, connected: true, player: None }), json!({ "id": 0, "connected": true, "player": null }));
    }

    #[test]
    fn input_channels_are_reserved() {
        assert!(is_reserved("pad", false) && is_reserved("rumble", false));
        assert!(is_reserved("action", true) && is_reserved("pad", true) && is_reserved("nge.rumble", true));
        assert!(!is_reserved("action", false) && !is_reserved("rumble", true));
        assert_eq!(ACTIONS_PROP, "actions");
    }

    #[test]
    fn frame_stats_wire_format_is_stable() {
        let wire = FRAME_STATS.encode(&FrameReport { fps: 120, worst_ms: 9 });
        assert_eq!(wire, json!({ "fps": 120, "worst_ms": 9 }));
        assert_eq!(FRAME_STATS.name(), "fps");
    }
}
