//! 动作：游戏问「跳按下了没有」，不直接读键名（见 docs/input-actions.md）。
//! 绑定表（[`ActionMap`]）每个玩家一张，[`Actions`] 每帧从 [`Input`] 算出每个动作的状态；
//! 手柄按设备分给玩家，状态变了发 `action` / `pad` 事件给 TSX；[`Rumble`] 让玩家的手柄震动。

use std::collections::{BTreeMap, BTreeSet};

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_core::bridge::{ACTION_EVENT, ActionEvent, PAD_EVENT, PadEvent, RUMBLE_OUT, RumbleOut, RumbleRequest};
use serde::{Deserialize, Serialize};

use crate::input::{Input, Pad, Stick, Trigger};
use crate::resources::Outbox;

/// 一次震动最长这么久（秒）。
pub const MAX_RUMBLE_SECONDS: f32 = 5.0;
/// 轴推过它，发给 TSX 的方向才算数。
const EVENT_THRESHOLD: f32 = 0.5;

/// 一维的模拟量来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalogAxis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    L2,
    R2,
}

/// 二维的模拟量来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalogStick {
    LeftStick,
    RightStick,
}

/// 一个动作绑了什么。键名和 [`Input::key_down`] 的一样（键盘的 `Space`，手柄的 `GamepadA`、`GamepadLeftStickLeft`、`GamepadL2`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Binding {
    /// 有一个键按着就是按着。
    Button {
        #[serde(default)]
        keys: Vec<String>,
    },
    /// −1 … 1：键给 ±1，模拟量给连续的值，取绝对值大的。
    Axis {
        #[serde(default)]
        negative: Vec<String>,
        #[serde(default)]
        positive: Vec<String>,
        #[serde(default)]
        analog: Vec<AnalogAxis>,
    },
    /// 两个方向各 −1 … 1（x 向右、y 向上）：键和摇杆里取长度大的。
    Axis2d {
        #[serde(default)]
        left: Vec<String>,
        #[serde(default)]
        right: Vec<String>,
        #[serde(default)]
        down: Vec<String>,
        #[serde(default)]
        up: Vec<String>,
        #[serde(default)]
        analog: Vec<AnalogStick>,
    },
}

/// 绑定表：每个玩家一张，动作名 → 绑定。TSX 的 prop `actions` 就是它。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActionMap {
    #[serde(default)]
    pub players: Vec<BTreeMap<String, Binding>>,
    /// 正在等玩家按键来绑：所有动作都算没按。
    #[serde(default)]
    pub capturing: bool,
}

impl ActionMap {
    /// 从 JSON 文本读（示例把默认绑定放在一个 Rust 和 TSX 共用的 JSON 文件里）。
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("绑定表读不出来: {e}"))
    }
}

/// 一个动作这一帧的状态。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ActionState {
    pub down: bool,
    pub pressed: bool,
    pub released: bool,
    /// 按钮：x 是 0 … 1；一维的轴：x；二维的轴：x、y。
    pub value: Vec2,
}

/// 发给 TSX 的样子：按着没有、方向。
type Sent = (bool, i8, i8);

/// 每个玩家每个动作的状态，`PreUpdate` 里从 [`Input`] 算。没声明的动作、没有的玩家：没按，值是 0。
#[derive(Resource, Debug, Default)]
pub struct Actions {
    map: ActionMap,
    /// 每个玩家的手柄号。
    slots: Vec<Option<u32>>,
    states: Vec<BTreeMap<String, ActionState>>,
    /// 告诉过 TSX 的动作状态、手柄归谁。
    sent: Vec<BTreeMap<String, Sent>>,
    reported: BTreeMap<u32, Option<usize>>,
    /// 换了表：按着的动作再告诉 TSX 一遍。
    resend: bool,
    /// 上一帧在等玩家按键来绑。
    was_capturing: bool,
    /// 绑键结束时还按着的键（键盘和合并的手柄键、每个手柄自己的）：松开之前不算数。
    blocked: BTreeSet<String>,
    blocked_pad: BTreeSet<(u32, String)>,
    /// 直接驱动的动作（[`Actions::drive`]）：(玩家, 动作) → 每个来源的方向；这一帧新按下、松开的。
    driven: BTreeMap<(usize, String), BTreeMap<String, Vec2>>,
    driven_pressed: BTreeSet<(usize, String)>,
    driven_released: BTreeSet<(usize, String)>,
    /// 上一帧结束时直接驱动按着的：这个来源按着时一帧里松开又按下才算重按。
    driven_was: BTreeSet<(usize, String)>,
}

/// 一个玩家的动作。
#[derive(Clone, Copy)]
pub struct PlayerActions<'a>(Option<&'a BTreeMap<String, ActionState>>);

impl PlayerActions<'_> {
    pub fn state(&self, action: &str) -> ActionState {
        self.0.and_then(|states| states.get(action)).copied().unwrap_or_default()
    }

    /// 现在按着（轴：不是 0）。
    pub fn down(&self, action: &str) -> bool {
        self.state(action).down
    }

    /// 这一帧刚按下（按着的键这一帧松开过又按下也算，和 [`Input::key_pressed`] 一样）。
    pub fn pressed(&self, action: &str) -> bool {
        self.state(action).pressed
    }

    /// 这一帧刚松开（同一帧里按下又松开时 `pressed` 也是 true）。
    pub fn released(&self, action: &str) -> bool {
        self.state(action).released
    }

    /// 一维的值：按钮 0 … 1（绑了扳机时是它的模拟量），轴 −1 … 1。
    pub fn value(&self, action: &str) -> f32 {
        self.state(action).value.x
    }

    /// 二维的值，长度不超过 1。
    pub fn vec(&self, action: &str) -> Vec2 {
        self.state(action).value
    }
}

impl Actions {
    /// 换绑定表（改键、TSX 的 prop）。按着的动作接着算按着、座位不动；连着的手柄和按着的动作会再报给 TSX 一遍。
    pub fn set_map(&mut self, map: ActionMap) {
        self.slots.resize(map.players.len(), None);
        self.map = map;
        self.reported.clear();
        self.resend = true;
    }

    pub fn map(&self) -> &ActionMap {
        &self.map
    }

    /// 第 `player` 个玩家（从 0 数）的动作。
    pub fn player(&self, player: usize) -> PlayerActions<'_> {
        PlayerActions(self.states.get(player))
    }

    /// 第一个玩家的。
    pub fn down(&self, action: &str) -> bool {
        self.player(0).down(action)
    }

    pub fn pressed(&self, action: &str) -> bool {
        self.player(0).pressed(action)
    }

    pub fn released(&self, action: &str) -> bool {
        self.player(0).released(action)
    }

    pub fn value(&self, action: &str) -> f32 {
        self.player(0).value(action)
    }

    pub fn vec(&self, action: &str) -> Vec2 {
        self.player(0).vec(action)
    }

    /// 这个玩家的手柄号。只有一个玩家时所有手柄都是他的，这里给的是最先接入的那个。
    pub fn pad_of(&self, player: usize) -> Option<u32> {
        self.slots.get(player).copied().flatten()
    }

    /// 把手柄 `pad` 给第 `player` 个玩家；他原来的手柄换给 `pad` 原来的主人，别人的不动。
    pub fn assign(&mut self, player: usize, pad: u32) {
        if player >= self.slots.len() {
            return;
        }
        let before = self.slots[player];
        if let Some(other) = self.slots.iter().position(|s| *s == Some(pad)) {
            self.slots[other] = before;
        }
        self.slots[player] = Some(pad);
    }

    /// 直接驱动一个动作（触屏按钮、虚拟摇杆）：不经过键和绑定表，改键管不到。`value` 是方向（按钮给 `Vec2::X`），`None` 是松开。
    /// 同一个动作可以有几个来源（`source`），各自按下松开，方向加起来（左右都按着是 0）。动作要在表里声明过才有状态。
    pub fn drive(&mut self, player: usize, action: &str, source: &str, value: Option<Vec2>) {
        let key = (player, action.to_owned());
        let clean = |v: Vec2| Vec2::new(unit(v.x), unit(v.y));
        match value {
            Some(v) => {
                let sources = self.driven.entry(key.clone()).or_default();
                if sources.is_empty() {
                    self.driven_pressed.insert(key);
                }
                sources.insert(source.to_owned(), clean(v));
            }
            None => {
                let Some(sources) = self.driven.get_mut(&key) else { return };
                if sources.remove(source).is_some() && sources.is_empty() {
                    self.driven.remove(&key);
                    self.driven_released.insert(key);
                }
            }
        }
    }

    /// 松开所有直接驱动的动作（切后台：手指的结束事件可能不来了）。下一帧算刚松开、告诉 TSX。
    pub fn release_driven(&mut self) {
        let held = std::mem::take(&mut self.driven);
        self.driven_pressed.clear();
        self.driven_released.extend(held.into_keys());
    }

    /// 只有一个玩家：不分手柄，读合并后的。
    fn merged(&self) -> bool {
        self.map.players.len() <= 1
    }

    /// 手柄归第几个玩家；没有绑定表时不归谁。
    fn player_of(&self, pad: u32) -> Option<usize> {
        match self.map.players.len() {
            0 => None,
            1 => Some(0),
            _ => self.slots.iter().position(|s| *s == Some(pad)),
        }
    }

    /// 拔掉的手柄让出位置；没有位置的手柄给最靠前的、还没手柄的玩家。
    fn seat(&mut self, connected: &BTreeSet<u32>) {
        for slot in &mut self.slots {
            if slot.is_some_and(|pad| !connected.contains(&pad)) {
                *slot = None;
            }
        }
        for &pad in connected {
            if !self.slots.contains(&Some(pad))
                && let Some(free) = self.slots.iter_mut().find(|s| s.is_none())
            {
                *free = Some(pad);
            }
        }
    }

    /// 手柄接入、拔出、换了玩家时告诉 TSX。
    fn report_pads(&mut self, connected: &BTreeSet<u32>, outbox: &mut Outbox) {
        let gone: Vec<u32> = self.reported.keys().copied().filter(|pad| !connected.contains(pad)).collect();
        for id in gone {
            self.reported.remove(&id);
            outbox.send(PAD_EVENT, &PadEvent { id, connected: false, player: None });
        }
        for &id in connected {
            let player = self.player_of(id);
            if self.reported.insert(id, player) != Some(player) {
                outbox.send(PAD_EVENT, &PadEvent { id, connected: true, player });
            }
        }
    }

    /// 绑键刚结束：这时还按着的键记下来，松开之前不算数（不然绑上去的那个键、取消用的 Esc 马上触发动作）。松开了的放行。
    fn block_held_keys(&mut self, input: &Input) {
        if self.was_capturing && !self.map.capturing {
            self.blocked.extend(input.keys_down().map(str::to_owned));
            for id in input.pads() {
                let keys = input.pad(id).into_iter().flat_map(Pad::keys_down);
                self.blocked_pad.extend(keys.map(|k| (id, k.to_owned())));
            }
        }
        self.was_capturing = self.map.capturing;
        self.blocked.retain(|code| input.key_down(code));
        self.blocked_pad.retain(|(id, code)| input.pad(*id).is_some_and(|pad| pad.key_down(code)));
    }

    /// 从这一帧的输入算每个动作的状态，变了的告诉 TSX。
    fn update(&mut self, input: &Input, outbox: &mut Outbox) {
        let connected: BTreeSet<u32> = input.pads().collect();
        // 换表前拔掉的手柄也要报一次
        for &id in input.pads_disconnected() {
            if !self.reported.contains_key(&id) {
                outbox.send(PAD_EVENT, &PadEvent { id, connected: false, player: None });
            }
        }
        self.seat(&connected);
        self.report_pads(&connected, outbox);
        self.block_held_keys(input);

        let players = self.map.players.len();
        // 表里去掉了的玩家：按着的告诉 TSX 松开
        for (player, sent) in self.sent.iter().enumerate().skip(players) {
            for (name, _) in sent.iter().filter(|(_, sent)| sent.0) {
                outbox.send(ACTION_EVENT, &ActionEvent { action: name.clone(), player, down: false, x: 0, y: 0 });
            }
        }
        self.states.resize_with(players, BTreeMap::new);
        self.sent.resize_with(players, BTreeMap::new);
        let merged = self.merged();
        let resend = std::mem::take(&mut self.resend);
        let Self { map, slots, states, sent, blocked, blocked_pad, driven, driven_pressed, driven_released, driven_was, .. } = self;
        for (player, bindings) in map.players.iter().enumerate() {
            let pad = if merged { None } else { Some(slots[player].and_then(|id| input.pad(id).map(|pad| (id, pad)))) };
            let reader = Reader { input, pad, blocked, blocked_pad };
            let (states, sent) = (&mut states[player], &mut sent[player]);
            // 表里去掉了的动作：按着的告诉 TSX 松开
            states.retain(|name, _| bindings.contains_key(name));
            sent.retain(|name, sent| {
                if !bindings.contains_key(name) && sent.0 {
                    outbox.send(ACTION_EVENT, &ActionEvent { action: name.clone(), player, down: false, x: 0, y: 0 });
                }
                bindings.contains_key(name)
            });
            for (name, binding) in bindings {
                if !states.contains_key(name) {
                    states.insert(name.clone(), ActionState::default());
                    sent.insert(name.clone(), Sent::default());
                }
                let (Some(state), Some(sent)) = (states.get_mut(name), sent.get_mut(name)) else { continue };
                let key = (player, name.clone());
                let direct = Driven {
                    value: driven.get(&key).map(|sources| sources.values().fold(Vec2::ZERO, |sum, v| sum + *v)),
                    pressed: driven_pressed.contains(&key),
                    released: driven_released.contains(&key),
                    was: driven_was.contains(&key),
                };
                *state = if map.capturing { ActionState { released: state.down, ..Default::default() } } else { evaluate(binding, &reader, direct, *state) };
                let mut told = false;
                let told_down = sent.0;
                let mut tell = |now: Sent, again: bool| {
                    if *sent != now || (again && now.0 && !told) {
                        *sent = now;
                        told = true;
                        outbox.send(ACTION_EVENT, &ActionEvent { action: name.clone(), player, down: now.0, x: now.1, y: now.2 });
                    }
                };
                // 一帧里按下又松开的按钮：两个都告诉；TSX 以为还按着时（一帧里松开又按下）先告诉松开
                if state.pressed && matches!(binding, Binding::Button { .. }) {
                    if told_down {
                        tell((false, 0, 0), false);
                    }
                    tell((true, 1, 0), false);
                }
                // 换了表：按着的再报一遍
                tell(quantize(binding, state), resend);
            }
        }
        driven_pressed.clear();
        driven_released.clear();
        *driven_was = driven.keys().cloned().collect();
    }
}

/// −1 … 1，不是有限数的当 0。
fn unit(v: f32) -> f32 {
    if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 }
}

/// 直接驱动的动作这一帧的样子：现在的方向（没有来源按着是 `None`）、这一帧新按下 / 全松开过。
#[derive(Clone, Copy)]
struct Driven {
    value: Option<Vec2>,
    pressed: bool,
    released: bool,
    /// 上一帧结束时按着。
    was: bool,
}

/// 一个玩家读输入：键盘大家共用；手柄的键和模拟量，只有一个玩家时读合并后的（`pad` 是 `None`），不然读他自己的手柄（没有就都是 0）。
/// 绑键结束时还按着的键（`blocked`）当没按。
struct Reader<'a> {
    input: &'a Input,
    pad: Option<Option<(u32, &'a Pad)>>,
    blocked: &'a BTreeSet<String>,
    blocked_pad: &'a BTreeSet<(u32, String)>,
}

#[derive(Clone, Copy)]
enum Edge {
    Down,
    Pressed,
    Released,
}

impl Reader<'_> {
    fn key(&self, code: &str, edge: Edge) -> bool {
        match (self.pad, code.starts_with("Gamepad")) {
            (Some(pad), true) => pad.is_some_and(|(id, pad)| {
                // 借来的键名只用来查，不分配
                !self.blocked_pad.iter().any(|(i, c)| *i == id && c == code)
                    && match edge {
                        Edge::Down => pad.key_down(code),
                        Edge::Pressed => pad.key_pressed(code),
                        Edge::Released => pad.key_released(code),
                    }
            }),
            _ => {
                !self.blocked.contains(code)
                    && match edge {
                        Edge::Down => self.input.key_down(code),
                        Edge::Pressed => self.input.key_pressed(code),
                        Edge::Released => self.input.key_released(code),
                    }
            }
        }
    }

    fn any<'k>(&self, codes: impl IntoIterator<Item = &'k String>, edge: Edge) -> bool {
        codes.into_iter().any(|code| self.key(code, edge))
    }

    /// 有一个键这一帧松开过又按下了。
    fn repressed<'k>(&self, codes: impl IntoIterator<Item = &'k String>) -> bool {
        codes.into_iter().any(|code| self.key(code, Edge::Pressed) && self.key(code, Edge::Released))
    }

    fn stick(&self, stick: Stick) -> Vec2 {
        match self.pad {
            None => self.input.stick(stick),
            Some(pad) => pad.map_or(Vec2::ZERO, |(_, pad)| pad.stick(stick)),
        }
    }

    fn trigger(&self, trigger: Trigger) -> f32 {
        match self.pad {
            None => self.input.trigger(trigger),
            Some(pad) => pad.map_or(0.0, |(_, pad)| pad.trigger(trigger)),
        }
    }

    fn axis(&self, axis: AnalogAxis) -> f32 {
        match axis {
            AnalogAxis::LeftStickX => self.stick(Stick::Left).x,
            AnalogAxis::LeftStickY => self.stick(Stick::Left).y,
            AnalogAxis::RightStickX => self.stick(Stick::Right).x,
            AnalogAxis::RightStickY => self.stick(Stick::Right).y,
            AnalogAxis::L2 => self.trigger(Trigger::L2),
            AnalogAxis::R2 => self.trigger(Trigger::R2),
        }
    }

    /// 一个键当按钮时的值：扳机是它的模拟量（平台层没给模拟量时按着就是 1），别的按着是 1。
    fn key_value(&self, code: &str) -> f32 {
        let down = f32::from(u8::from(self.key(code, Edge::Down)));
        let analog = match code {
            "GamepadL2" => self.trigger(Trigger::L2),
            "GamepadR2" => self.trigger(Trigger::R2),
            _ => 0.0,
        };
        if analog > 0.0 { analog } else { down }
    }

    /// 两组键：正的按着 1，负的按着 −1，都按或都没按是 0。
    fn digital<'k>(&self, negative: impl IntoIterator<Item = &'k String>, positive: impl IntoIterator<Item = &'k String>) -> f32 {
        f32::from(i8::from(self.any(positive, Edge::Down)) - i8::from(self.any(negative, Edge::Down)))
    }
}

/// 这个动作绑了哪根摇杆的模拟量：[左, 右]。
fn analog_sticks(binding: &Binding) -> [bool; 2] {
    match binding {
        Binding::Button { .. } => [false; 2],
        Binding::Axis { analog, .. } => [
            analog.iter().any(|a| matches!(a, AnalogAxis::LeftStickX | AnalogAxis::LeftStickY)),
            analog.iter().any(|a| matches!(a, AnalogAxis::RightStickX | AnalogAxis::RightStickY)),
        ],
        Binding::Axis2d { analog, .. } => [analog.contains(&AnalogStick::LeftStick), analog.contains(&AnalogStick::RightStick)],
    }
}

/// 绑了某根摇杆的模拟量时，这根摇杆的方向键（推过一半）不算：不然推过一半值就从 0.5 跳到 1。
fn counted<'k>(keys: &'k [String], [left, right]: [bool; 2]) -> impl Iterator<Item = &'k String> + Clone {
    keys.iter().filter(move |code| !(left && code.starts_with("GamepadLeftStick")) && !(right && code.starts_with("GamepadRightStick")))
}

fn larger(a: f32, b: f32) -> f32 {
    if b.abs() > a.abs() { b } else { a }
}

/// 刚按下、刚松开。`prs` / `rel`：有键这一帧按下 / 松开；`repress`：有键这一帧松开过又按下；
/// `level`：没有按键的边沿（模拟量）时，从不按到按着也算刚按下。
fn edges(prev: bool, down: bool, prs: bool, rel: bool, repress: bool, level: bool) -> (bool, bool) {
    // 按钮只认这一帧真按下的：改完键、换了手柄时还按着的不算刚按下
    let pressed = (prs && (!prev || repress) && (down || rel)) || (level && down && !prev);
    let released = (!down && (prev || (prs && rel))) || (prev && down && repress);
    (pressed, released)
}

fn evaluate(binding: &Binding, r: &Reader, direct: Driven, prev: ActionState) -> ActionState {
    let sticks = analog_sticks(binding);
    let driven = direct.value.unwrap_or(Vec2::ZERO);
    match binding {
        Binding::Button { keys } => {
            let down = r.any(keys, Edge::Down) || direct.value.is_some();
            let (prs, rel) = (r.any(keys, Edge::Pressed) || direct.pressed, r.any(keys, Edge::Released) || direct.released);
            // 直接驱动的按钮（按住连发）上一帧按着、这一帧松开过又按下，和键一样算重按；别的来源按着时触屏点一下不算
            let repress = r.repressed(keys) || (direct.was && direct.pressed && direct.released);
            let (pressed, released) = edges(prev.down, down, prs, rel, repress, false);
            let held = f32::from(u8::from(direct.value.is_some()));
            ActionState { down, pressed, released, value: Vec2::new(keys.iter().map(|k| r.key_value(k)).fold(held, f32::max), 0.0) }
        }
        Binding::Axis { negative, positive, analog } => {
            let (negative, positive) = (counted(negative, sticks), counted(positive, sticks));
            let digital = r.digital(negative.clone(), positive.clone());
            let x = analog.iter().map(|a| r.axis(*a)).fold(larger(digital, driven.x), larger).clamp(-1.0, 1.0);
            let down = x != 0.0;
            let keys = negative.chain(positive);
            let (prs, rel) = (r.any(keys.clone(), Edge::Pressed) || direct.pressed, r.any(keys.clone(), Edge::Released) || direct.released);
            let (pressed, released) = edges(prev.down, down, prs, rel, r.repressed(keys), true);
            ActionState { down, pressed, released, value: Vec2::new(x, 0.0) }
        }
        Binding::Axis2d { left, right, down, up, analog } => {
            let [left, right, down, up] = [left, right, down, up].map(|keys| counted(keys, sticks));
            let digital = Vec2::new(r.digital(left.clone(), right.clone()), r.digital(down.clone(), up.clone()));
            let pick = |best: Vec2, v: Vec2| if v.length_squared() > best.length_squared() { v } else { best };
            let stick = |s: &AnalogStick| r.stick(if *s == AnalogStick::LeftStick { Stick::Left } else { Stick::Right });
            // 键盘斜着按、摇杆推到角上：长度不超过 1
            let value = analog.iter().map(stick).fold(pick(digital, driven), pick).clamp_length_max(1.0);
            let held = value != Vec2::ZERO;
            let keys = left.chain(right).chain(down).chain(up);
            let (prs, rel) = (r.any(keys.clone(), Edge::Pressed) || direct.pressed, r.any(keys.clone(), Edge::Released) || direct.released);
            let (pressed, released) = edges(prev.down, held, prs, rel, r.repressed(keys), true);
            ActionState { down: held, pressed, released, value }
        }
    }
}

/// 发给 TSX 的样子：按钮是按着没有；轴是推过一半的方向。
fn quantize(binding: &Binding, state: &ActionState) -> Sent {
    if matches!(binding, Binding::Button { .. }) {
        return (state.down, i8::from(state.down), 0);
    }
    let sign = |v: f32| if v > EVENT_THRESHOLD { 1 } else if v < -EVENT_THRESHOLD { -1 } else { 0 };
    let (x, y) = (sign(state.value.x), sign(state.value.y));
    (x != 0 || y != 0, x, y)
}

pub(crate) fn update_actions(input: Res<Input>, mut actions: ResMut<Actions>, mut outbox: ResMut<Outbox>) {
    actions.update(&input, &mut outbox);
}

/// 手柄震动：这一帧要的，帧末按玩家找到手柄发给平台层。同一个手柄上新的一次顶掉正在震的；游戏暂停（时间缩放是 0）时自动停。
#[derive(Resource, Debug, Default)]
pub struct Rumble(Vec<RumbleRequest>);

impl Rumble {
    /// 让第 `player` 个玩家的手柄震 `seconds` 秒：`low` 是低频（重）马达、`high` 是高频（轻）马达，0 … 1。
    pub fn play(&mut self, player: usize, low: f32, high: f32, seconds: f32) {
        self.0.push(RumbleRequest { player, low, high, duration: seconds, stop: false });
    }

    /// 停掉第 `player` 个玩家的手柄上正在震的。
    pub fn stop(&mut self, player: usize) {
        self.0.push(RumbleRequest { player, low: 0.0, high: 0.0, duration: 0.0, stop: true });
    }

    pub(crate) fn request(&mut self, request: RumbleRequest) {
        self.0.push(request);
    }
}

/// 强度限制在 0 … 1、时长最多 [`MAX_RUMBLE_SECONDS`]；没东西可震（强度都是 0、时长不是正数）时是 `None`。
fn clean_rumble(request: &RumbleRequest) -> Option<(f32, f32, f32)> {
    let unit = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    let (low, high) = (unit(request.low), unit(request.high));
    let duration = if request.duration.is_finite() { request.duration.clamp(0.0, MAX_RUMBLE_SECONDS) } else { 0.0 };
    (duration > 0.0 && (low > 0.0 || high > 0.0)).then_some((low, high, duration))
}

/// 帧末：要震的、要停的按玩家找到手柄发出去；游戏刚暂停时把正在震的都停掉。`playing`：发过震动、还没停过。
pub(crate) fn flush_rumble(
    time: Res<crate::Time>,
    actions: Res<Actions>,
    mut rumble: ResMut<Rumble>,
    mut outbox: ResMut<Outbox>,
    mut playing: Local<bool>,
    mut paused: Local<bool>,
) {
    for request in rumble.0.drain(..) {
        // 只有一个玩家时所有手柄一起震
        let pad = if actions.merged() { None } else { actions.pad_of(request.player) };
        if request.player >= actions.map.players.len() || !(actions.merged() || pad.is_some()) {
            continue;
        }
        if request.stop {
            outbox.send(RUMBLE_OUT, &RumbleOut::stop(pad));
        } else if let Some((low, high, duration)) = clean_rumble(&request) {
            outbox.send(RUMBLE_OUT, &RumbleOut { pad, low, high, duration });
            *playing = true;
        }
    }
    let now_paused = time.delta <= 0.0;
    if now_paused && !*paused && *playing {
        outbox.send(RUMBLE_OUT, &RumbleOut::stop(None));
        *playing = false;
    }
    *paused = now_paused;
}

#[cfg(test)]
mod tests {
    use nge_core::Viewport;
    use serde_json::{Value, json};

    use super::*;
    use crate::App;

    const VIEWPORT: Viewport = Viewport { size: [400.0, 300.0], pixels: [800, 600] };

    fn map() -> Value {
        json!({ "players": [{
            "jump": { "kind": "button", "keys": ["Space", "GamepadA"] },
            "fire": { "kind": "button", "keys": ["GamepadR2"] },
            "move": { "kind": "axis", "negative": ["ArrowLeft", "GamepadLeft"], "positive": ["ArrowRight", "GamepadRight"], "analog": ["LeftStickX"] },
            "gas": { "kind": "axis", "analog": ["R2"] },
            "aim": { "kind": "axis2d", "left": ["KeyJ"], "right": ["KeyL"], "down": ["KeyK"], "up": ["KeyI"], "analog": ["RightStick"] },
        }] })
    }

    fn two_players() -> Value {
        json!({ "players": [
            { "jump": { "kind": "button", "keys": ["KeyW", "GamepadA"] }, "move": { "kind": "axis", "negative": ["KeyA"], "positive": ["KeyD"], "analog": ["LeftStickX"] } },
            { "jump": { "kind": "button", "keys": ["KeyI", "GamepadA"] }, "move": { "kind": "axis", "negative": ["KeyJ"], "positive": ["KeyL"], "analog": ["LeftStickX"] } },
        ] })
    }

    fn app(map: Value) -> App {
        let mut app = App::new();
        app.prop("actions", Some(&map));
        app
    }

    fn key(app: &mut App, code: &str, down: bool) {
        app.command("key", &json!({ "code": code, "down": down }));
    }

    fn pad(app: &mut App, state: Value) {
        app.command("pad", &state);
    }

    fn tick(app: &mut App) -> Vec<(String, Value)> {
        app.tick(VIEWPORT, 1.0 / 60.0)
    }

    fn actions(app: &App) -> &Actions {
        app.world().resource::<Actions>()
    }

    #[test]
    fn a_button_is_down_while_any_of_its_keys_is() {
        let mut app = app(map());
        tick(&mut app);
        assert!(!actions(&app).down("jump") && !actions(&app).pressed("jump"));
        key(&mut app, "Space", true);
        tick(&mut app);
        let a = actions(&app);
        assert!(a.down("jump") && a.pressed("jump") && !a.released("jump"));
        assert_eq!(a.value("jump"), 1.0);
        key(&mut app, "GamepadA", true);
        tick(&mut app);
        assert!(actions(&app).down("jump") && !actions(&app).pressed("jump"), "已经按着，再按一个键不算刚按下");
        key(&mut app, "Space", false);
        tick(&mut app);
        assert!(actions(&app).down("jump") && !actions(&app).released("jump"), "还有一个键按着");
        key(&mut app, "GamepadA", false);
        tick(&mut app);
        assert!(!actions(&app).down("jump") && actions(&app).released("jump"));
        tick(&mut app);
        assert!(!actions(&app).released("jump"), "刚松开只有一帧");
    }

    #[test]
    fn a_tap_within_one_frame_is_pressed_and_released() {
        let mut app = app(map());
        key(&mut app, "Space", true);
        key(&mut app, "Space", false);
        tick(&mut app);
        let a = actions(&app);
        assert!(a.pressed("jump") && a.released("jump") && !a.down("jump"));
    }

    #[test]
    fn an_axis_takes_the_larger_of_its_keys_and_the_stick() {
        let mut app = app(map());
        key(&mut app, "ArrowLeft", true);
        tick(&mut app);
        assert_eq!(actions(&app).value("move"), -1.0);
        assert!(actions(&app).down("move") && actions(&app).pressed("move"), "轴不是 0 就算按着");
        key(&mut app, "ArrowRight", true);
        tick(&mut app);
        assert_eq!(actions(&app).value("move"), 0.0, "两边都按着");
        assert!(actions(&app).released("move"));
        key(&mut app, "ArrowLeft", false);
        key(&mut app, "ArrowRight", false);
        app.command("gamepad", &json!({ "left": [0.25, 0.9] }));
        tick(&mut app);
        assert_eq!(actions(&app).value("move"), 0.25, "摇杆的模拟量");
        key(&mut app, "GamepadLeft", true);
        tick(&mut app);
        assert_eq!(actions(&app).value("move"), -1.0, "键的值更大");
    }

    #[test]
    fn a_2d_axis_has_both_components() {
        let mut app = app(map());
        key(&mut app, "KeyJ", true);
        key(&mut app, "KeyI", true);
        tick(&mut app);
        let diagonal = actions(&app).vec("aim");
        assert!((diagonal.length() - 1.0).abs() < 1e-6 && diagonal.x < 0.0 && diagonal.y > 0.0, "斜着按长度也是 1：{diagonal}");
        key(&mut app, "KeyJ", false);
        key(&mut app, "KeyI", false);
        app.command("gamepad", &json!({ "right": [0.5, -0.25] }));
        tick(&mut app);
        assert_eq!(actions(&app).vec("aim"), Vec2::new(0.5, -0.25));
        assert_eq!(actions(&app).value("aim"), 0.5, "value 是 x");
    }

    #[test]
    fn a_trigger_is_analog_and_still_a_button() {
        let mut app = app(map());
        app.command("gamepad", &json!({ "r2": 0.25 }));
        tick(&mut app);
        let a = actions(&app);
        assert!(!a.down("fire"), "没过一半不算按下");
        assert_eq!((a.value("fire"), a.value("gas")), (0.25, 0.25), "模拟量照样给");
        key(&mut app, "GamepadR2", true);
        app.command("gamepad", &json!({ "r2": 0.75 }));
        tick(&mut app);
        let a = actions(&app);
        assert!(a.down("fire") && a.pressed("fire"));
        assert_eq!(a.value("fire"), 0.75);
        // 只有键、没有模拟量（触屏按钮发的）：按着就是 1
        app.command("gamepad", &json!({}));
        tick(&mut app);
        assert_eq!(actions(&app).value("fire"), 1.0);
    }

    #[test]
    fn unknown_actions_and_players_are_idle() {
        let mut app = app(map());
        key(&mut app, "Space", true);
        tick(&mut app);
        let a = actions(&app);
        assert!(!a.down("nope") && !a.pressed("nope") && a.value("nope") == 0.0 && a.vec("nope") == Vec2::ZERO);
        assert!(!a.player(3).down("jump"));
        assert!(a.player(0).down("jump"));
    }

    #[test]
    fn with_one_player_every_pad_is_theirs() {
        let mut app = app(map());
        // 合并后的那条路：任何手柄的 A
        key(&mut app, "GamepadA", true);
        pad(&mut app, json!({ "id": 4, "keys": ["GamepadA"] }));
        pad(&mut app, json!({ "id": 9, "keys": [] }));
        let events = tick(&mut app);
        assert!(actions(&app).down("jump"));
        let pads: Vec<_> = events.iter().filter(|(n, _)| n == "pad").map(|(_, e)| e.clone()).collect();
        assert_eq!(pads, [json!({ "id": 4, "connected": true, "player": 0 }), json!({ "id": 9, "connected": true, "player": 0 })]);
    }

    #[test]
    fn without_a_map_pads_belong_to_nobody() {
        let mut app = App::new();
        pad(&mut app, json!({ "id": 4 }));
        let events = tick(&mut app);
        assert_eq!(events, [("pad".to_owned(), json!({ "id": 4, "connected": true, "player": null }))], "没有绑定表就没有玩家");
    }

    #[test]
    fn a_stick_with_an_analog_binding_ignores_its_own_direction_keys() {
        let mut bindings = map();
        bindings["players"][0]["move"]["negative"] = json!(["ArrowLeft", "GamepadLeft", "GamepadLeftStickLeft"]);
        bindings["players"][0]["move"]["positive"] = json!(["ArrowRight", "GamepadRight", "GamepadLeftStickRight", "GamepadRightStickRight"]);
        bindings["players"][0]["aim"]["right"] = json!(["KeyL", "GamepadRightStickRight"]);
        let mut app = app(bindings);
        for push in [0.55, 0.9] {
            // 平台层这时也把「推过一半」当成键发过来
            key(&mut app, "GamepadLeftStickRight", true);
            app.command("gamepad", &json!({ "left": [push, 0.0] }));
            tick(&mut app);
            assert_eq!(actions(&app).value("move"), push, "不从 0.5 跳到 1");
        }
        key(&mut app, "GamepadLeftStickRight", false);
        app.command("gamepad", &json!({ "left": [0.0, 0.0], "right": [0.75, 0.0] }));
        key(&mut app, "GamepadRightStickRight", true);
        tick(&mut app);
        assert_eq!(actions(&app).value("move"), 1.0, "别的摇杆的方向键照常算");
        assert_eq!(actions(&app).vec("aim"), Vec2::new(0.75, 0.0), "二维的轴也一样");
    }

    #[test]
    fn a_2d_axis_is_never_longer_than_one() {
        let mut app = app(map());
        app.command("gamepad", &json!({ "right": [1.0, 1.0] }));
        tick(&mut app);
        let corner = actions(&app).vec("aim");
        assert!((corner.length() - 1.0).abs() < 1e-6 && (corner.x - corner.y).abs() < 1e-6, "摇杆的角：{corner}");
        app.command("gamepad", &json!({ "right": [0.25, 0.0] }));
        tick(&mut app);
        assert_eq!(actions(&app).vec("aim"), Vec2::new(0.25, 0.0), "短的不拉长");
    }

    #[test]
    fn a_2d_axis_has_pressed_and_released_edges() {
        let mut app = app(map());
        key(&mut app, "KeyL", true);
        tick(&mut app);
        let a = actions(&app);
        assert!(a.down("aim") && a.pressed("aim") && !a.released("aim"));
        tick(&mut app);
        assert!(actions(&app).down("aim") && !actions(&app).pressed("aim"));
        key(&mut app, "KeyL", false);
        tick(&mut app);
        let a = actions(&app);
        assert!(!a.down("aim") && a.released("aim") && !a.pressed("aim"));
        // 只推摇杆（没有键的边沿）也有刚按下
        app.command("gamepad", &json!({ "right": [0.3, 0.0] }));
        tick(&mut app);
        assert!(actions(&app).pressed("aim"));
    }

    #[test]
    fn released_and_pressed_again_within_one_frame_is_a_new_press() {
        let mut app = app(map());
        key(&mut app, "Space", true);
        key(&mut app, "ArrowLeft", true);
        tick(&mut app);
        key(&mut app, "Space", false);
        key(&mut app, "Space", true);
        key(&mut app, "ArrowLeft", false);
        key(&mut app, "ArrowLeft", true);
        tick(&mut app);
        let a = actions(&app);
        assert!(a.down("jump") && a.pressed("jump") && a.released("jump"), "和直接读键一样：这一帧松开过又按下");
        assert!(a.down("move") && a.pressed("move") && a.released("move"), "轴的键也一样");
        tick(&mut app);
        assert!(!actions(&app).pressed("jump") && !actions(&app).pressed("move"));
    }

    #[test]
    fn an_axis_key_tapped_within_one_frame_has_both_edges() {
        let mut app = app(map());
        key(&mut app, "ArrowLeft", true);
        key(&mut app, "ArrowLeft", false);
        tick(&mut app);
        let a = actions(&app);
        assert!(a.pressed("move") && a.released("move") && !a.down("move"));
        // 两边同时按下：值是 0，不算按下
        key(&mut app, "ArrowLeft", true);
        key(&mut app, "ArrowRight", true);
        tick(&mut app);
        assert!(!actions(&app).pressed("move") && !actions(&app).down("move"));
    }

    fn act(app: &mut App, args: Value) {
        app.command("action", &args);
    }

    #[test]
    fn actions_can_be_driven_directly_whatever_the_bindings() {
        let mut bindings = map();
        // 跳改成了别的键，空格现在是别的动作
        bindings["players"][0]["jump"]["keys"] = json!(["KeyR"]);
        let mut app = app(bindings);
        act(&mut app, json!({ "action": "jump", "down": true }));
        let events = tick(&mut app);
        let a = actions(&app);
        assert!(a.down("jump") && a.pressed("jump") && a.value("jump") == 1.0, "触屏按钮按动作发，不看键");
        assert_eq!(action_events(&events), [json!({ "action": "jump", "player": 0, "down": true, "x": 1, "y": 0 })]);
        tick(&mut app);
        assert!(actions(&app).down("jump") && !actions(&app).pressed("jump"));
        act(&mut app, json!({ "action": "jump", "down": false }));
        tick(&mut app);
        assert!(!actions(&app).down("jump") && actions(&app).released("jump"));
        // 一帧里点一下
        act(&mut app, json!({ "action": "jump", "down": true }));
        act(&mut app, json!({ "action": "jump", "down": false }));
        tick(&mut app);
        assert!(actions(&app).pressed("jump") && actions(&app).released("jump") && !actions(&app).down("jump"));
        // 没按着时的松开、坏的命令不算
        act(&mut app, json!({ "action": "jump", "down": false }));
        act(&mut app, json!({ "down": true }));
        tick(&mut app);
        assert!(!actions(&app).released("jump") && !actions(&app).down("jump"));
        // 刚按下只算一帧
        act(&mut app, json!({ "action": "jump", "down": true }));
        tick(&mut app);
        tick(&mut app);
        assert!(actions(&app).down("jump") && !actions(&app).pressed("jump"));
    }

    #[test]
    fn a_touch_tap_while_the_key_is_held_is_not_a_new_press() {
        let mut app = app(map());
        key(&mut app, "Space", true);
        tick(&mut app);
        // 键按着，触屏在一帧里点一下：触屏这个来源上一帧没按着，不算重按
        act(&mut app, json!({ "action": "jump", "down": true }));
        act(&mut app, json!({ "action": "jump", "down": false }));
        let events = tick(&mut app);
        let a = actions(&app);
        assert!(a.down("jump") && !a.pressed("jump") && !a.released("jump"));
        assert!(action_events(&events).is_empty(), "TSX 也不收到松开、按下");
    }

    #[test]
    fn suspending_releases_what_touch_was_holding() {
        let mut app = app(map());
        act(&mut app, json!({ "action": "jump", "down": true }));
        act(&mut app, json!({ "action": "move", "down": true, "x": -1, "source": "stick" }));
        tick(&mut app);
        // 切后台：手指的结束事件可能永远不来
        app.command("nge.suspend", &json!({ "suspended": true }));
        app.command("nge.suspend", &json!({ "suspended": false }));
        let events = tick(&mut app);
        let a = actions(&app);
        assert!(!a.down("jump") && a.released("jump") && !a.down("move") && a.value("move") == 0.0);
        assert_eq!(
            action_events(&events),
            [json!({ "action": "jump", "player": 0, "down": false, "x": 0, "y": 0 }), json!({ "action": "move", "player": 0, "down": false, "x": 0, "y": 0 })]
        );
    }

    #[test]
    fn a_driven_button_released_and_pressed_again_within_one_frame_is_a_new_press() {
        let mut app = app(map());
        act(&mut app, json!({ "action": "jump", "down": true }));
        tick(&mut app);
        // 按住连发：同一帧里松开又按下
        act(&mut app, json!({ "action": "jump", "down": false }));
        act(&mut app, json!({ "action": "jump", "down": true }));
        let events = tick(&mut app);
        let a = actions(&app);
        assert!(a.down("jump") && a.pressed("jump") && a.released("jump"), "和键一样：这一帧松开过又按下");
        assert_eq!(
            action_events(&events),
            [json!({ "action": "jump", "player": 0, "down": false, "x": 0, "y": 0 }), json!({ "action": "jump", "player": 0, "down": true, "x": 1, "y": 0 })],
            "TSX 也收到一对松开、按下"
        );
        tick(&mut app);
        assert!(actions(&app).down("jump") && !actions(&app).pressed("jump"));
        // 键也一样补发给 TSX
        key(&mut app, "Space", true);
        tick(&mut app);
        act(&mut app, json!({ "action": "jump", "down": false }));
        tick(&mut app);
        key(&mut app, "Space", false);
        key(&mut app, "Space", true);
        let events = tick(&mut app);
        assert_eq!(action_events(&events).len(), 2, "键在一帧里松开又按下也补发");
    }

    #[test]
    fn directions_driven_directly_add_up_per_source() {
        let mut app = app(two_players());
        act(&mut app, json!({ "action": "move", "down": true, "x": -1, "source": "left" }));
        tick(&mut app);
        assert_eq!(actions(&app).value("move"), -1.0);
        assert_eq!(actions(&app).player(1).value("move"), 0.0, "只管指定的玩家");
        act(&mut app, json!({ "action": "move", "down": true, "x": 1, "source": "right" }));
        tick(&mut app);
        assert_eq!(actions(&app).value("move"), 0.0, "左右都按着：抵消");
        act(&mut app, json!({ "action": "move", "down": false, "source": "left" }));
        tick(&mut app);
        assert_eq!(actions(&app).value("move"), 1.0, "松开一个，另一个还在");
        // 同一个来源再发一次是换值（虚拟摇杆）；数值限制在 −1 … 1，坏的当 0
        act(&mut app, json!({ "action": "move", "down": true, "x": 0.25, "source": "right" }));
        tick(&mut app);
        assert_eq!(actions(&app).value("move"), 0.25);
        act(&mut app, json!({ "action": "move", "down": true, "x": 9, "source": "right" }));
        act(&mut app, json!({ "action": "move", "player": 1, "down": true, "x": -0.5 }));
        tick(&mut app);
        assert_eq!((actions(&app).value("move"), actions(&app).player(1).value("move")), (1.0, -0.5));
        // 每个来源先限幅再加：9 和 −1 抵消成 0，不是 8
        act(&mut app, json!({ "action": "move", "down": true, "x": -1, "source": "left" }));
        tick(&mut app);
        assert_eq!(actions(&app).value("move"), 0.0);
        act(&mut app, json!({ "action": "move", "down": false, "source": "left" }));
        // 键盘的键和它取大的
        key(&mut app, "KeyA", true);
        act(&mut app, json!({ "action": "move", "down": true, "x": 0.25, "source": "right" }));
        tick(&mut app);
        assert_eq!(actions(&app).value("move"), -1.0);
    }

    #[test]
    fn switching_between_one_and_two_players_keeps_a_held_pad_key_down() {
        let one = json!({ "players": [two_players()["players"][0].clone()] });
        let mut app = app(one.clone());
        // 平台层两条路都发：合并的键和这个手柄自己的状态
        key(&mut app, "GamepadA", true);
        pad(&mut app, json!({ "id": 2, "keys": ["GamepadA"] }));
        tick(&mut app);
        assert!(actions(&app).down("jump") && actions(&app).pressed("jump"));
        app.prop("actions", Some(&two_players()));
        tick(&mut app);
        let a = actions(&app);
        assert!(a.down("jump") && !a.pressed("jump") && !a.released("jump"), "换成两个人：还是按着，不算新按下");
        assert!(!a.player(1).down("jump"));
        app.prop("actions", Some(&one));
        tick(&mut app);
        let a = actions(&app);
        assert!(a.down("jump") && !a.pressed("jump") && !a.released("jump"), "换回一个人也一样");
    }

    #[test]
    fn a_new_map_keeps_the_seats_and_reports_held_actions_again() {
        let mut app = app(two_players());
        pad(&mut app, json!({ "id": 5 }));
        pad(&mut app, json!({ "id": 7 }));
        key(&mut app, "KeyW", true);
        tick(&mut app);
        app.world_mut().resource_mut::<Actions>().assign(0, 7);
        tick(&mut app);
        app.prop("actions", Some(&two_players()));
        assert_eq!((actions(&app).pad_of(0), actions(&app).pad_of(1)), (Some(7), Some(5)), "换表不动座位");
        let events = tick(&mut app);
        assert_eq!((actions(&app).pad_of(0), actions(&app).pad_of(1)), (Some(7), Some(5)));
        assert_eq!(action_events(&events), [json!({ "action": "jump", "player": 0, "down": true, "x": 1, "y": 0 })], "按着的再报一遍，JS 重载后对得上");
        assert!(!actions(&app).pressed("jump"), "Rust 这边不算新按下");
    }

    #[test]
    fn a_pad_unplugged_right_before_a_new_map_is_still_reported_gone() {
        let mut app = app(map());
        pad(&mut app, json!({ "id": 3 }));
        tick(&mut app);
        pad(&mut app, json!({ "id": 3, "connected": false }));
        app.prop("actions", Some(&map()));
        let events = tick(&mut app);
        assert!(events.contains(&("pad".to_owned(), json!({ "id": 3, "connected": false, "player": null }))), "{events:?}");
    }

    #[test]
    fn players_dropped_from_the_map_let_go_of_what_they_held() {
        let mut app = app(two_players());
        key(&mut app, "KeyI", true);
        key(&mut app, "KeyL", true);
        tick(&mut app);
        assert!(actions(&app).player(1).down("jump"));
        app.prop("actions", Some(&json!({ "players": [two_players()["players"][0].clone()] })));
        let events = action_events(&tick(&mut app));
        assert!(events.contains(&json!({ "action": "jump", "player": 1, "down": false, "x": 0, "y": 0 })), "{events:?}");
        assert!(events.contains(&json!({ "action": "move", "player": 1, "down": false, "x": 0, "y": 0 })), "{events:?}");
        assert!(!actions(&app).player(1).down("jump"));
    }

    #[test]
    fn assigning_swaps_only_the_two_players_involved() {
        let three = json!({ "players": [{}, {}, {}] });
        let mut app = app(three);
        for id in [10, 11, 12] {
            pad(&mut app, json!({ "id": id }));
        }
        tick(&mut app);
        app.world_mut().resource_mut::<Actions>().assign(0, 12);
        let a = actions(&app);
        assert_eq!([a.pad_of(0), a.pad_of(1), a.pad_of(2)], [Some(12), Some(11), Some(10)], "第二个人的不动");
        app.world_mut().resource_mut::<Actions>().assign(7, 10);
        assert_eq!(actions(&app).pad_of(2), Some(10), "没有的玩家：不变");
    }

    #[test]
    fn with_two_players_each_gets_a_pad() {
        let mut app = app(two_players());
        pad(&mut app, json!({ "id": 5, "keys": ["GamepadA"] }));
        let events = tick(&mut app);
        assert!(events.contains(&("pad".to_owned(), json!({ "id": 5, "connected": true, "player": 0 }))), "{events:?}");
        let a = actions(&app);
        assert!(a.player(0).down("jump") && !a.player(1).down("jump"), "第一个手柄是第一个玩家的");
        assert_eq!((a.pad_of(0), a.pad_of(1)), (Some(5), None));

        pad(&mut app, json!({ "id": 7, "left": [-0.5, 0.0] }));
        let events = tick(&mut app);
        assert!(events.contains(&("pad".to_owned(), json!({ "id": 7, "connected": true, "player": 1 }))), "{events:?}");
        let a = actions(&app);
        assert_eq!((a.player(0).value("move"), a.player(1).value("move")), (0.0, -0.5), "各读各的摇杆");
        assert!(!a.player(1).down("jump"), "第一个手柄的 A 不是第二个玩家的");

        // 合并的那条路上的手柄键不算（分不清是谁的）
        key(&mut app, "GamepadA", true);
        pad(&mut app, json!({ "id": 5, "keys": [] }));
        tick(&mut app);
        assert!(!actions(&app).player(0).down("jump") && !actions(&app).player(1).down("jump"));

        // 第三个手柄没有位置；第一个拔掉后位置空出来，第二个不动，下一帧第三个补上
        pad(&mut app, json!({ "id": 8 }));
        let events = tick(&mut app);
        assert!(events.contains(&("pad".to_owned(), json!({ "id": 8, "connected": true, "player": null }))), "{events:?}");
        pad(&mut app, json!({ "id": 5, "connected": false }));
        let events = tick(&mut app);
        assert!(events.contains(&("pad".to_owned(), json!({ "id": 5, "connected": false, "player": null }))), "{events:?}");
        assert!(events.contains(&("pad".to_owned(), json!({ "id": 8, "connected": true, "player": 0 }))), "{events:?}");
        assert_eq!((actions(&app).pad_of(0), actions(&app).pad_of(1)), (Some(8), Some(7)));
    }

    #[test]
    fn a_player_without_a_pad_has_no_pad_keys() {
        let mut app = app(two_players());
        pad(&mut app, json!({ "id": 1, "keys": ["GamepadA"], "left": [1.0, 0.0] }));
        tick(&mut app);
        assert!(!actions(&app).player(1).down("jump") && actions(&app).player(1).value("move") == 0.0);
    }

    #[test]
    fn one_keyboard_can_be_split_between_two_players() {
        let mut app = app(two_players());
        key(&mut app, "KeyD", true);
        key(&mut app, "KeyI", true);
        tick(&mut app);
        let a = actions(&app);
        assert_eq!((a.player(0).value("move"), a.player(1).value("move")), (1.0, 0.0));
        assert!(!a.player(0).down("jump") && a.player(1).down("jump"));
    }

    #[test]
    fn pads_can_be_assigned_by_hand() {
        let mut app = app(two_players());
        pad(&mut app, json!({ "id": 1 }));
        pad(&mut app, json!({ "id": 2, "keys": ["GamepadA"] }));
        tick(&mut app);
        app.world_mut().resource_mut::<Actions>().assign(0, 2);
        let events = tick(&mut app);
        let a = actions(&app);
        assert_eq!((a.pad_of(0), a.pad_of(1)), (Some(2), Some(1)), "两个人对调");
        assert!(a.player(0).down("jump"));
        assert!(events.contains(&("pad".to_owned(), json!({ "id": 1, "connected": true, "player": 1 }))), "{events:?}");
    }

    fn action_events(events: &[(String, Value)]) -> Vec<Value> {
        events.iter().filter(|(n, _)| n == "action").map(|(_, e)| e.clone()).collect()
    }

    #[test]
    fn changes_are_sent_to_tsx() {
        let mut app = app(map());
        assert!(action_events(&tick(&mut app)).is_empty(), "没按的不发");
        key(&mut app, "Space", true);
        assert_eq!(action_events(&tick(&mut app)), [json!({ "action": "jump", "player": 0, "down": true, "x": 1, "y": 0 })]);
        assert!(action_events(&tick(&mut app)).is_empty(), "按着不重复发");
        key(&mut app, "Space", false);
        assert_eq!(action_events(&tick(&mut app)), [json!({ "action": "jump", "player": 0, "down": false, "x": 0, "y": 0 })]);
        // 一帧里按下又松开：两个都发
        key(&mut app, "Space", true);
        key(&mut app, "Space", false);
        let taps = action_events(&tick(&mut app));
        assert_eq!(taps.iter().map(|e| e["down"].as_bool().unwrap()).collect::<Vec<_>>(), [true, false]);
        // 轴：过一半才算，方向变了才发
        app.command("gamepad", &json!({ "left": [0.25, 0.0] }));
        assert!(action_events(&tick(&mut app)).is_empty());
        app.command("gamepad", &json!({ "left": [0.75, 0.0] }));
        assert_eq!(action_events(&tick(&mut app)), [json!({ "action": "move", "player": 0, "down": true, "x": 1, "y": 0 })]);
        app.command("gamepad", &json!({ "left": [1.0, 0.0] }));
        assert!(action_events(&tick(&mut app)).is_empty(), "模拟量在变，方向没变");
        key(&mut app, "KeyI", true);
        assert_eq!(action_events(&tick(&mut app)), [json!({ "action": "aim", "player": 0, "down": true, "x": 0, "y": 1 })]);
    }

    #[test]
    fn nothing_is_down_while_capturing_a_key() {
        let mut app = app(map());
        key(&mut app, "Space", true);
        tick(&mut app);
        let mut capturing = map();
        capturing["capturing"] = json!(true);
        app.prop("actions", Some(&capturing));
        let events = tick(&mut app);
        assert!(!actions(&app).down("jump") && actions(&app).released("jump"));
        assert_eq!(action_events(&events).len(), 1, "松开告诉 TSX");
        // 绑完了键还按着：等它松开一次才重新算数，Rust 和 TSX 都一样
        app.prop("actions", Some(&map()));
        let events = tick(&mut app);
        assert!(!actions(&app).down("jump") && !actions(&app).pressed("jump"), "还按着的键不算");
        assert!(action_events(&events).is_empty(), "{events:?}");
        tick(&mut app);
        assert!(!actions(&app).down("jump"));
        key(&mut app, "Space", false);
        let events = tick(&mut app);
        assert!(!actions(&app).released("jump") && action_events(&events).is_empty(), "松开的这一下也不算");
        key(&mut app, "Space", true);
        let events = tick(&mut app);
        assert!(actions(&app).down("jump") && actions(&app).pressed("jump"), "松开过再按才算");
        assert_eq!(action_events(&events).len(), 1);
    }

    #[test]
    fn keys_held_when_capturing_ends_wait_for_a_release_on_pads_too() {
        let mut app = app(two_players());
        pad(&mut app, json!({ "id": 5, "keys": ["GamepadA"] }));
        key(&mut app, "Escape", true);
        let mut capturing = two_players();
        capturing["players"][0]["menu"] = json!({ "kind": "button", "keys": ["Escape"] });
        let done = capturing.clone();
        capturing["capturing"] = json!(true);
        app.prop("actions", Some(&capturing));
        tick(&mut app);
        // Esc 取消了捕获：绑着 Escape 的动作不触发；手柄上按着的 A 也不算
        app.prop("actions", Some(&done));
        let events = tick(&mut app);
        assert!(action_events(&events).is_empty(), "{events:?}");
        assert!(!actions(&app).down("menu") && !actions(&app).down("jump"));
        pad(&mut app, json!({ "id": 5, "keys": [] }));
        tick(&mut app);
        pad(&mut app, json!({ "id": 5, "keys": ["GamepadA"] }));
        tick(&mut app);
        assert!(actions(&app).down("jump") && !actions(&app).down("menu"), "手柄的键松开再按就算；Esc 还按着");
    }

    #[test]
    fn a_new_map_takes_over_and_reports_the_pads_again() {
        let mut app = app(map());
        pad(&mut app, json!({ "id": 3 }));
        key(&mut app, "KeyX", true);
        tick(&mut app);
        assert!(!actions(&app).down("jump"));
        let mut rebound = map();
        rebound["players"][0]["jump"]["keys"] = json!(["KeyX"]);
        app.prop("actions", Some(&rebound));
        let events = tick(&mut app);
        assert!(actions(&app).down("jump"), "改了键马上生效");
        assert!(events.contains(&("pad".to_owned(), json!({ "id": 3, "connected": true, "player": 0 }))), "JS 重载后也知道连着哪些手柄：{events:?}");
        // 坏的表不要，原来的留着；去掉 prop 也留着
        app.prop("actions", Some(&json!({ "players": "nope" })));
        app.prop("actions", None);
        tick(&mut app);
        assert!(actions(&app).down("jump"));
        assert_eq!(actions(&app).map().players.len(), 1);
    }

    #[test]
    fn an_action_dropped_from_the_map_is_released() {
        let mut app = app(map());
        key(&mut app, "Space", true);
        tick(&mut app);
        let mut without = map();
        without["players"][0].as_object_mut().unwrap().remove("jump");
        app.prop("actions", Some(&without));
        assert_eq!(action_events(&tick(&mut app)), [json!({ "action": "jump", "player": 0, "down": false, "x": 0, "y": 0 })]);
        assert!(!actions(&app).down("jump"));
    }

    fn rumbles(events: &[(String, Value)]) -> Vec<Value> {
        events.iter().filter(|(n, _)| n == "nge.rumble").map(|(_, e)| e.clone()).collect()
    }

    #[test]
    fn rumble_goes_to_the_players_pad() {
        // 一个玩家：所有手柄
        let mut one = app(map());
        pad(&mut one, json!({ "id": 4 }));
        one.world_mut().resource_mut::<Rumble>().play(0, 0.5, 2.0, 0.25);
        assert_eq!(rumbles(&tick(&mut one)), [json!({ "pad": null, "low": 0.5, "high": 1.0, "duration": 0.25 })], "强度限制在 0 … 1");
        assert!(rumbles(&tick(&mut one)).is_empty(), "只发一次");
        one.world_mut().resource_mut::<Rumble>().play(1, 1.0, 1.0, 0.25);
        assert!(rumbles(&tick(&mut one)).is_empty(), "没有这个玩家");

        let mut two = app(two_players());
        pad(&mut two, json!({ "id": 6 }));
        pad(&mut two, json!({ "id": 9 }));
        tick(&mut two);
        two.command("rumble", &json!({ "player": 1, "low": 1.0, "high": 0.25, "duration": 0.5 }));
        assert_eq!(rumbles(&tick(&mut two)), [json!({ "pad": 9, "low": 1.0, "high": 0.25, "duration": 0.5 })], "TSX 的 rumble command 走同一条路");
        pad(&mut two, json!({ "id": 9, "connected": false }));
        tick(&mut two);
        two.world_mut().resource_mut::<Rumble>().play(1, 1.0, 1.0, 0.25);
        assert!(rumbles(&tick(&mut two)).is_empty(), "这个玩家没有手柄");
    }

    const STOP: f32 = 0.0;

    #[test]
    fn rumble_can_be_stopped() {
        let mut two = app(two_players());
        pad(&mut two, json!({ "id": 6 }));
        tick(&mut two);
        two.world_mut().resource_mut::<Rumble>().stop(0);
        assert_eq!(rumbles(&tick(&mut two)), [json!({ "pad": 6, "low": STOP, "high": STOP, "duration": STOP })], "强度和时长都是 0 就是停");
        two.command("rumble", &json!({ "player": 0, "stop": true }));
        assert_eq!(rumbles(&tick(&mut two)).len(), 1, "TSX 也能停");
        two.world_mut().resource_mut::<Rumble>().stop(1);
        assert!(rumbles(&tick(&mut two)).is_empty(), "没手柄的玩家不用停");
        let mut one = app(map());
        one.world_mut().resource_mut::<Rumble>().stop(0);
        assert_eq!(rumbles(&tick(&mut one)), [json!({ "pad": null, "low": STOP, "high": STOP, "duration": STOP })]);
    }

    #[test]
    fn pausing_stops_a_rumble_that_is_playing() {
        let mut app = app(map());
        app.world_mut().resource_mut::<crate::Time>().scale = 0.0;
        assert!(rumbles(&tick(&mut app)).is_empty(), "没在震，暂停时不用停");
        app.world_mut().resource_mut::<crate::Time>().scale = 1.0;
        tick(&mut app);
        app.world_mut().resource_mut::<Rumble>().play(0, 1.0, 1.0, 2.0);
        assert_eq!(rumbles(&tick(&mut app)).len(), 1);
        app.world_mut().resource_mut::<crate::Time>().scale = 0.0;
        assert_eq!(rumbles(&tick(&mut app)), [json!({ "pad": null, "low": STOP, "high": STOP, "duration": STOP })], "暂停了：所有手柄停");
        assert!(rumbles(&tick(&mut app)).is_empty(), "只停一次");
        // 继续了又暂停：中间没有新的震动，不用再停
        app.world_mut().resource_mut::<crate::Time>().scale = 1.0;
        tick(&mut app);
        app.world_mut().resource_mut::<crate::Time>().scale = 0.0;
        assert!(rumbles(&tick(&mut app)).is_empty(), "已经停了");
    }

    #[test]
    fn rumble_drops_nothing_to_play_and_caps_the_duration() {
        let mut app = app(map());
        let mut rumble = app.world_mut().resource_mut::<Rumble>();
        rumble.play(0, 0.0, 0.0, 1.0);
        rumble.play(0, 1.0, 1.0, 0.0);
        rumble.play(0, f32::NAN, -1.0, 1.0);
        rumble.play(0, 1.0, f32::NAN, f32::NAN);
        rumble.play(0, 0.5, 0.0, 60.0);
        assert_eq!(rumbles(&tick(&mut app)), [json!({ "pad": null, "low": 0.5, "high": 0.0, "duration": MAX_RUMBLE_SECONDS })]);
    }

    #[test]
    fn the_map_reads_from_json_text() {
        let map = ActionMap::from_json(&map().to_string()).unwrap();
        assert_eq!(map.players[0]["jump"], Binding::Button { keys: vec!["Space".into(), "GamepadA".into()] });
        assert!(matches!(&map.players[0]["aim"], Binding::Axis2d { analog, .. } if analog == &[AnalogStick::RightStick]));
        assert!(ActionMap::from_json("{").is_err());
        assert!(ActionMap::from_json(r#"{"players":[{"jump":{"kind":"lever"}}]}"#).is_err(), "不认识的种类");
    }
}
