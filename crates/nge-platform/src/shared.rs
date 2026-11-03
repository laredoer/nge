//! 两端共用：主线程侧的引擎代理（视口去重、消息转发、暂停控制、卡顿压测）。

use std::cell::Cell;
use std::time::{Duration, Instant};

use nge_core::{Engine, Msg, Viewport};
use serde_json::Value as JsonValue;

/// 环境变量：每隔 [`STALL_EVERY`] 让主线程睡这么多毫秒，用来验证渲染线程不受主线程卡顿影响。
pub const STALL_ENV: &str = "NGE_DEBUG_STALL_MS";
const STALL_EVERY: Duration = Duration::from_secs(2);
/// command：让主线程睡 `args.ms` 毫秒（压测按钮用），不转发给游戏。
const STALL_COMMAND: &str = "nge.stall_main";

/// 渲染线程该不该跑：View 在窗口里、且 App 在前台。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PauseState {
    in_window: bool,
    active: bool,
}

impl PauseState {
    /// 刚创建的 View 还没进窗口。
    pub const INITIAL: Self = Self { in_window: false, active: true };

    pub fn running(self) -> bool {
        self.in_window && self.active
    }

    pub fn with_window(self, in_window: bool) -> Self {
        Self { in_window, ..self }
    }

    pub fn with_active(self, active: bool) -> Self {
        Self { active, ..self }
    }
}

pub(crate) struct Proxy {
    engine: Engine,
    viewport: Cell<Viewport>,
    state: Cell<PauseState>,
    stall: Option<(Duration, Cell<Instant>)>,
}

impl Proxy {
    /// `active`：App 当前是否在前台。
    pub fn new(engine: Engine, active: bool) -> Self {
        let stall = std::env::var(STALL_ENV)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&ms| ms > 0)
            .map(|ms| {
                log::warn!("压测：主线程每 {}s 卡 {ms}ms", STALL_EVERY.as_secs());
                (Duration::from_millis(ms), Cell::new(Instant::now()))
            });
        // 渲染线程启动后先暂停，等 View 进窗口再恢复
        engine.pause();
        Self { engine, viewport: Cell::new(Viewport::default()), state: Cell::new(PauseState::INITIAL.with_active(active)), stall }
    }

    /// View 即将离开 / 已经进入窗口（主线程）。离开时同步暂停，返回后渲染线程不再访问 View。
    pub fn set_in_window(&self, in_window: bool) {
        self.transition(self.state.get().with_window(in_window));
    }

    /// App 进后台 / 回前台（主线程）。
    #[cfg_attr(not(target_os = "ios"), allow(dead_code))]
    pub fn set_active(&self, active: bool) {
        self.transition(self.state.get().with_active(active));
    }

    fn transition(&self, next: PauseState) {
        let prev = self.state.replace(next);
        match (prev.running(), next.running()) {
            (true, false) => {
                self.engine.pause();
            }
            (false, true) => self.engine.resume(),
            _ => {}
        }
    }

    /// display link 每帧调用（主线程）：视口变了才转发。
    pub fn tick(&self, viewport: Viewport) {
        if viewport != self.viewport.get() {
            self.viewport.set(viewport);
            self.engine.send(Msg::Viewport(viewport));
        }
        if let Some((ms, last)) = &self.stall
            && last.get().elapsed() >= STALL_EVERY
        {
            std::thread::sleep(*ms);
            log::info!("压测：主线程刚卡了 {}ms", ms.as_millis());
            last.set(Instant::now());
        }
    }

    pub fn prop(&self, name: &str, value: Option<&JsonValue>) {
        self.engine.send(Msg::Prop(name.to_owned(), value.cloned()));
    }

    pub fn command(&self, name: &str, args: &JsonValue) {
        if name == STALL_COMMAND {
            let ms = args["ms"].as_u64().unwrap_or(200);
            std::thread::sleep(Duration::from_millis(ms));
            return;
        }
        self.engine.send(Msg::Command(name.to_owned(), args.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::PauseState;

    #[test]
    fn initial_is_paused_until_in_window() {
        assert!(!PauseState::INITIAL.running());
        assert!(PauseState::INITIAL.with_window(true).running());
    }

    #[test]
    fn background_pauses_even_in_window() {
        let s = PauseState::INITIAL.with_window(true).with_active(false);
        assert!(!s.running());
        assert!(s.with_active(true).running());
    }

    #[test]
    fn leaving_window_while_background_stays_paused_on_foreground() {
        let s = PauseState::INITIAL.with_window(true).with_active(false).with_window(false);
        assert!(!s.with_active(true).running());
    }
}
