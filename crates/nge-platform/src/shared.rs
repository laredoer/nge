//! 两端共用：主线程侧的引擎代理（视口去重、消息转发、卡顿压测）。

use std::cell::Cell;
use std::time::{Duration, Instant};

use nge_core::{Engine, Msg, Viewport};
use serde_json::Value as JsonValue;

/// 环境变量：每隔 [`STALL_EVERY`] 让主线程睡这么多毫秒，用来验证渲染线程不受主线程卡顿影响。
pub const STALL_ENV: &str = "NGE_DEBUG_STALL_MS";
const STALL_EVERY: Duration = Duration::from_secs(2);
/// command：让主线程睡 `args.ms` 毫秒（压测按钮用），不转发给游戏。
const STALL_COMMAND: &str = "nge.stall_main";

pub(crate) struct Proxy {
    engine: Engine,
    viewport: Cell<Viewport>,
    stall: Option<(Duration, Cell<Instant>)>,
}

impl Proxy {
    pub fn new(engine: Engine) -> Self {
        let stall = std::env::var(STALL_ENV)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&ms| ms > 0)
            .map(|ms| {
                log::warn!("压测：主线程每 {}s 卡 {ms}ms", STALL_EVERY.as_secs());
                (Duration::from_millis(ms), Cell::new(Instant::now()))
            });
        Self { engine, viewport: Cell::new(Viewport::default()), stall }
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
