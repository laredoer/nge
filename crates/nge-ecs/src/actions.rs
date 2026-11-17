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

    /// 这一帧刚按下。
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

    /// 二维的值。
    pub fn vec(&self, action: &str) -> Vec2 {
        self.state(action).value
    }
}

impl Actions {
    /// 换绑定表（改键、TSX 的 prop）。按着的动作接着算按着；连着的手柄会再报给 TSX 一遍。
    pub fn set_map(&mut self, map: ActionMap) {
        self.slots.resize(map.players.len(), None);
        self.map = map;
        self.reported.clear();
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

    /// 把手柄 `pad` 给第 `player` 个玩家；他原来的手柄换给 `pad` 原来的主人。
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

    /// 只有一个玩家：不分手柄，读合并后的。
    fn merged(&self) -> bool {
        self.map.players.len() <= 1
    }

    /// 手柄归第几个玩家。
    fn player_of(&self, pad: u32) -> Option<usize> {
        if self.merged() { Some(0) } else { self.slots.iter().position(|s| *s == Some(pad)) }
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

        let players = self.map.players.len();
        self.states.resize_with(players, BTreeMap::new);
        self.sent.resize_with(players, BTreeMap::new);
        let merged = self.merged();
        let Self { map, slots, states, sent, .. } = self;
        for (player, bindings) in map.players.iter().enumerate() {
            let pad = if merged { None } else { Some(slots[player].and_then(|id| input.pad(id))) };
            let reader = Reader { input, pad };
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
                *state = if map.capturing { ActionState { released: state.down, ..Default::default() } } else { evaluate(binding, &reader, *state) };
                let mut tell = |now: Sent| {
                    if *sent != now {
                        *sent = now;
                        outbox.send(ACTION_EVENT, &ActionEvent { action: name.clone(), player, down: now.0, x: now.1, y: now.2 });
                    }
                };
                // 一帧里按下又松开的按钮：两个都告诉
                if state.pressed && matches!(binding, Binding::Button { .. }) {
                    tell((true, 1, 0));
                }
                tell(quantize(binding, state));
            }
        }
    }
}

/// 一个玩家读输入：键盘大家共用；手柄的键和模拟量，只有一个玩家时读合并后的（`pad` 是 `None`），不然读他自己的手柄（没有就都是 0）。
struct Reader<'a> {
    input: &'a Input,
    pad: Option<Option<&'a Pad>>,
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
            (Some(pad), true) => pad.is_some_and(|pad| match edge {
                Edge::Down => pad.key_down(code),
                Edge::Pressed => pad.key_pressed(code),
                Edge::Released => pad.key_released(code),
            }),
            _ => match edge {
                Edge::Down => self.input.key_down(code),
                Edge::Pressed => self.input.key_pressed(code),
                Edge::Released => self.input.key_released(code),
            },
        }
    }

    fn any(&self, codes: &[String], edge: Edge) -> bool {
        codes.iter().any(|code| self.key(code, edge))
    }

    fn stick(&self, stick: Stick) -> Vec2 {
        match self.pad {
            None => self.input.stick(stick),
            Some(pad) => pad.map_or(Vec2::ZERO, |pad| pad.stick(stick)),
        }
    }

    fn trigger(&self, trigger: Trigger) -> f32 {
        match self.pad {
            None => self.input.trigger(trigger),
            Some(pad) => pad.map_or(0.0, |pad| pad.trigger(trigger)),
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
    fn digital(&self, negative: &[String], positive: &[String]) -> f32 {
        f32::from(i8::from(self.any(positive, Edge::Down)) - i8::from(self.any(negative, Edge::Down)))
    }
}

fn larger(a: f32, b: f32) -> f32 {
    if b.abs() > a.abs() { b } else { a }
}

fn evaluate(binding: &Binding, r: &Reader, prev: ActionState) -> ActionState {
    match binding {
        Binding::Button { keys } => {
            let down = r.any(keys, Edge::Down);
            ActionState {
                down,
                // 只认这一帧真按下的：改完键、换了手柄时还按着的不算刚按下
                pressed: r.any(keys, Edge::Pressed) && !prev.down,
                released: !down && (prev.down || r.any(keys, Edge::Released)),
                value: Vec2::new(keys.iter().map(|k| r.key_value(k)).fold(0.0, f32::max), 0.0),
            }
        }
        Binding::Axis { negative, positive, analog } => {
            let x = analog.iter().map(|a| r.axis(*a)).fold(r.digital(negative, positive), larger).clamp(-1.0, 1.0);
            let down = x != 0.0;
            ActionState { down, pressed: down && !prev.down, released: !down && prev.down, value: Vec2::new(x, 0.0) }
        }
        Binding::Axis2d { left, right, down, up, analog } => {
            let keys = Vec2::new(r.digital(left, right), r.digital(down, up));
            let pick = |best: Vec2, v: Vec2| if v.length_squared() > best.length_squared() { v } else { best };
            let value = analog.iter().map(|s| r.stick(if *s == AnalogStick::LeftStick { Stick::Left } else { Stick::Right })).fold(keys, pick);
            let down = value != Vec2::ZERO;
            ActionState { down, pressed: down && !prev.down, released: !down && prev.down, value }
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

/// 手柄震动：这一帧要的，帧末按玩家找到手柄发给平台层。
#[derive(Resource, Debug, Default)]
pub struct Rumble(Vec<RumbleRequest>);

impl Rumble {
    /// 让第 `player` 个玩家的手柄震 `seconds` 秒：`low` 是低频（重）马达、`high` 是高频（轻）马达，0 … 1。
    pub fn play(&mut self, player: usize, low: f32, high: f32, seconds: f32) {
        self.0.push(RumbleRequest { player, low, high, duration: seconds });
    }
}

/// 强度限制在 0 … 1、时长最多 [`MAX_RUMBLE_SECONDS`]；没东西可震（强度都是 0、时长不是正数）时是 `None`。
fn clean_rumble(request: &RumbleRequest) -> Option<(f32, f32, f32)> {
    let unit = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    let (low, high) = (unit(request.low), unit(request.high));
    let duration = if request.duration.is_finite() { request.duration.clamp(0.0, MAX_RUMBLE_SECONDS) } else { 0.0 };
    (duration > 0.0 && (low > 0.0 || high > 0.0)).then_some((low, high, duration))
}

pub(crate) fn flush_rumble(actions: Res<Actions>, mut rumble: ResMut<Rumble>, mut outbox: ResMut<Outbox>) {
    for request in rumble.0.drain(..) {
        let Some((low, high, duration)) = clean_rumble(&request) else { continue };
        // 只有一个玩家时所有手柄一起震
        let pad = if actions.merged() { None } else { actions.pad_of(request.player) };
        if request.player < actions.map.players.len() && (actions.merged() || pad.is_some()) {
            outbox.send(RUMBLE_OUT, &RumbleOut { pad, low, high, duration });
        }
    }
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
        assert_eq!(actions(&app).vec("aim"), Vec2::new(-1.0, 1.0));
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
        // 绑完了键还按着：不算刚按下
        app.prop("actions", Some(&map()));
        tick(&mut app);
        assert!(actions(&app).down("jump") && !actions(&app).pressed("jump"));
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
