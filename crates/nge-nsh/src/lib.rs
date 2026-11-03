//! NSH 侧（主线程）：游戏视图元素 [`game_view`] 和类型化通道绑定 [`GameViewExt`]。
//!
//! ```ignore
//! let game = game_ref();
//! game_view(game)
//!     .game_prop(DIFFICULTY, move || difficulty.get())
//!     .sync_state(HUD, hud)
//!     .on_game_event(GAME_OVER, move |e| over.set(Some(e)))
//!     .flex_1();
//! game.send(CONTROL, &Control::Restart);
//! ```

use nge_core::bridge::{TOUCH, Touch, TouchPhase};
use nsh_core::prelude::*;
use nsh_core::{TouchEvent, TouchPhase as NshTouchPhase};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use nge_core::FrameReport;
pub use nge_core::bridge::{CommandKey, ENGINE_ERROR, EngineError, EventKey, FRAME_STATS, GAME_COMPONENT, PropKey, StateKey};

/// 游戏视图的句柄：放到 [`game_view`] 上，用它给游戏发 command。`Copy`，和 signal 一样随所属组件释放；
/// 视图未挂载或已释放时发的 command 会被丢弃。
#[derive(Clone, Copy)]
pub struct GameRef(ElementRef);

pub fn game_ref() -> GameRef {
    GameRef(element_ref())
}

impl GameRef {
    /// 发一个类型化 command（游戏侧 `App::add_command` 注册后用 `MessageReader<T>` 读取）。
    pub fn send<T: Serialize>(&self, key: CommandKey<T>, value: &T) {
        self.0.command(key.name(), key.encode(value));
    }

    /// 发一个未类型化 command（游戏侧收到 `NshCommand`；`nge.stall_main` 之类的平台命令也走这里）。
    pub fn command(&self, name: &str, args: serde_json::Value) {
        self.0.command(name, args);
    }
}

/// 游戏视图元素：平台层以 [`GAME_COMPONENT`] 注册的原生组件，触摸自动转发给游戏，引擎错误记日志。
/// 返回的是 NSH 的 `NativeView`，布局样式照常链式调用；不要再调 `.element_ref`（会顶掉 `game`）。
pub fn game_view(game: GameRef) -> NativeView {
    native(GAME_COMPONENT)
        .element_ref(&game.0)
        .on_touch(move |e: &TouchEvent| {
            for touch in touches(e) {
                game.send(TOUCH, &touch);
            }
        })
        .on_game_event(ENGINE_ERROR, |e| log::error!("游戏引擎错误: {}", e.message))
}

/// 只取这次变化了的手指。
fn touches(e: &TouchEvent) -> impl Iterator<Item = Touch> + '_ {
    let phase = match e.phase {
        NshTouchPhase::Start => TouchPhase::Start,
        NshTouchPhase::Move => TouchPhase::Move,
        NshTouchPhase::End => TouchPhase::End,
        NshTouchPhase::Cancel => TouchPhase::Cancel,
    };
    e.points
        .iter()
        .filter(|p| p.changed)
        .map(move |p| Touch { phase, id: p.id, x: p.location.x.0, y: p.location.y.0, t: e.timestamp })
}

/// 在游戏视图上绑定类型化通道。
pub trait GameViewExt: Sized {
    /// 把一个值绑定到游戏的 prop：值（通常读 signal）变了就推给游戏，游戏侧成为资源 `T`。
    fn game_prop<T: Serialize + 'static>(self, key: PropKey<T>, value: impl Fn() -> T + 'static) -> Self;

    /// 游戏侧资源 `T` 变化时写进 `signal`（用 `signal()` 创建的会对相等的值去重）。
    fn sync_state<T: DeserializeOwned + PartialEq + 'static>(self, key: StateKey<T>, signal: Signal<T>) -> Self;

    /// 游戏发出事件 `T` 时调用 `handler`。
    fn on_game_event<T: DeserializeOwned + 'static>(self, key: EventKey<T>, handler: impl Fn(T) + 'static) -> Self;
}

impl GameViewExt for NativeView {
    fn game_prop<T: Serialize + 'static>(self, key: PropKey<T>, value: impl Fn() -> T + 'static) -> Self {
        self.prop(key.name(), move || key.encode(&value()))
    }

    fn sync_state<T: DeserializeOwned + PartialEq + 'static>(self, key: StateKey<T>, signal: Signal<T>) -> Self {
        self.on_native_event(move |e: &NativeEvent| {
            if *e.name == *key.name() {
                log::trace!("state {} = {}", key.name(), e.data);
                match key.decode(&e.data) {
                    Ok(v) => {
                        // signal 的所属组件已释放时 set 会 panic
                        if !signal.try_set(v) {
                            log::debug!("state {} 的 signal 已释放", key.name());
                        }
                    }
                    Err(err) => log::warn!("{err}"),
                }
            }
        })
    }

    fn on_game_event<T: DeserializeOwned + 'static>(self, key: EventKey<T>, handler: impl Fn(T) + 'static) -> Self {
        self.on_native_event(move |e: &NativeEvent| {
            if *e.name == *key.name() {
                log::trace!("event {} = {}", key.name(), e.data);
                match key.decode(&e.data) {
                    Ok(v) => handler(v),
                    Err(err) => log::warn!("{err}"),
                }
            }
        })
    }
}
