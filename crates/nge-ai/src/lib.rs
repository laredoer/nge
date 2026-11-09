//! 寻路和 AI：格子 A*、小状态机、追击（见 docs/ai.md）。

mod chase;
mod fsm;
mod grid;

use nge_ecs::{App, Plugin, Update};

pub use chase::{AGENT_RADIUS, ARRIVE, ChaseChanged, ChaseState, Chaser, LOSE, NavGrid, REPLAN, chase};
pub use fsm::Fsm;
pub use grid::{Cell, Grid, MAX_EXPANDED, clear, find_path, middle, smooth};

/// 注册 [`ChaseChanged`] 消息和追击系统（`Update`）。
pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ChaseChanged>().add_systems(Update, chase);
    }
}
