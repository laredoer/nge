//! NSH 游戏引擎核心：游戏接口、帧循环（原生是渲染线程，浏览器是主线程的 rAF）、帧统计。

pub mod bridge;
mod frame;
#[cfg(any(target_arch = "wasm32", test))]
mod phase;
pub mod game;
pub mod pipeline_cache;
#[cfg(not(target_arch = "wasm32"))]
pub mod runtime;
#[cfg(not(target_arch = "wasm32"))]
mod startup;
pub mod stats;
#[cfg(target_arch = "wasm32")]
pub mod web;

pub use bridge::{CommandKey, EventKey, GAME_COMPONENT, PropKey, StateKey};
pub use frame::{Msg, compression_features};
pub use game::{Ctx, Emit, FrameTiming, Game, GameFactory, Gfx, Viewport};
#[cfg(not(target_arch = "wasm32"))]
pub use runtime::{Engine, create_surface};
pub use stats::FrameReport;
#[cfg(target_arch = "wasm32")]
pub use web::Engine;
pub use wgpu;
