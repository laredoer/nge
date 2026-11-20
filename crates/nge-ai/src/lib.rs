//! 寻路和 AI：格子 A*、小状态机、追击（见 docs/ai.md）。

mod bt;
mod chase;
mod fsm;
mod grid;
mod ground;
mod navmesh3d;

use nge_ecs::{App, Plugin, Update};

pub use bt::{Blackboard, Node, Status, Tree, Value};
pub use chase::{AGENT_RADIUS, ARRIVE, ChaseChanged, ChaseState, Chaser, LOSE, NavGrid, REPLAN, chase};
pub use fsm::Fsm;
pub use ground::{Ground, GroundGraph, GroundMap, Hop, Link, Move, Reach, Spot, Terrain, Walker};
pub use grid::{Cell, Grid, MAX_EXPANDED, clear, find_path, middle, smooth};
pub use navmesh3d::{MAX_NAV3D_EXPANDED, NavConfig3d, NavMesh3d, NavObstacle3d, NavTri};

/// 注册 [`ChaseChanged`] 消息、[`NavMesh3d`] 资源和追击系统（`Update`）。
pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NavMesh3d>().add_message::<ChaseChanged>().add_systems(Update, chase);
    }
}
