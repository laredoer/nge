//! NSH 游戏引擎核心：游戏接口、渲染线程帧循环、帧统计。

pub mod bridge;
pub mod game;
pub mod runtime;
pub mod stats;

pub use bridge::{CommandKey, EventKey, GAME_COMPONENT, PropKey, StateKey};
pub use game::{Ctx, Emit, Game, GameFactory, Gfx, Viewport};
pub use runtime::{Engine, Msg, create_surface};
pub use stats::FrameReport;
pub use wgpu;
