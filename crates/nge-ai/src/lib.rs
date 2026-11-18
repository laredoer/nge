//! 寻路和 AI：格子 A*、小状态机、追击（见 docs/ai.md）。

mod bt;
mod chase;
mod fsm;
mod grid;
mod ground;

use nge_ecs::{App, Plugin, Update};

pub use bt::{Blackboard, Node, Status, Tree, Value};
pub use chase::{AGENT_RADIUS, ARRIVE, ChaseChanged, ChaseState, Chaser, LOSE, NavGrid, REPLAN, chase};
pub use fsm::Fsm;
pub use ground::{Ground, GroundGraph, GroundMap, Hop, Link, Move, Reach, Spot, Terrain};
pub use grid::{Cell, Grid, MAX_EXPANDED, clear, find_path, middle, smooth};

/// 注册 [`ChaseChanged`] 消息和追击系统（`Update`）。
pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ChaseChanged>().add_systems(Update, chase);
    }
}
