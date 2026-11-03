//! NSH 界面和游戏之间的协议：两边共用这些类型和 key。

use bevy_ecs::prelude::{Message, Resource};
use nge_core::{CommandKey, EventKey, PropKey, StateKey};
use serde::{Deserialize, Serialize};

#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Difficulty {
    #[default]
    Normal,
    Hard,
}

#[derive(Message, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Control {
    Pause,
    Resume,
    Restart,
}

/// HUD 显示的全部状态。
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hud {
    pub score: u32,
    pub lives: u32,
    pub best: u32,
    pub paused: bool,
    pub over: bool,
}

impl Default for Hud {
    fn default() -> Self {
        Self { score: 0, lives: START_LIVES, best: 0, paused: false, over: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameOver {
    pub score: u32,
    pub best: u32,
    pub new_best: bool,
}

pub const START_LIVES: u32 = 3;

pub const DIFFICULTY: PropKey<Difficulty> = PropKey::new("difficulty");
pub const CONTROL: CommandKey<Control> = CommandKey::new("control");
pub const HUD: StateKey<Hud> = StateKey::new("hud");
pub const GAME_OVER: EventKey<GameOver> = EventKey::new("game_over");
