//! 手柄：一帧的读数 → 按着的键名、去掉死区的摇杆和扳机（见 docs/gamepad.md）；几个手柄合起来的一份走原来的路，
//! 每个手柄各自的一份按设备发（见 docs/input-actions.md）。纯函数，不依赖 GameController。

use std::collections::{BTreeMap, BTreeSet};

use nge_core::bridge::{GamepadInput, PadInput};

/// 摇杆方向、扳机「按着」的门槛。
const PRESS: f32 = 0.5;
/// 按着的扳机掉到它以下才算松开：停在 0.5 附近时不来回按下松开。
const TRIGGER_RELEASE: f32 = 0.4;
/// 摇杆长度小于它时当 0。
const DEADZONE: f32 = 0.15;

/// 一个手柄这一帧的读数。没有的按钮是 false，轴的 y 向上。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Reading {
    pub a: bool,
    pub b: bool,
    pub x: bool,
    pub y: bool,
    pub l1: bool,
    pub r1: bool,
    pub l3: bool,
    pub r3: bool,
    pub menu: bool,
    pub options: bool,
    pub l2: f32,
    pub r2: f32,
    pub dpad: [f32; 2],
    pub left: [f32; 2],
    pub right: [f32; 2],
}

/// 方向的键名：上、下、左、右。
type Directions = [&'static str; 4];
const DPAD: Directions = ["GamepadUp", "GamepadDown", "GamepadLeft", "GamepadRight"];
const LEFT_STICK: Directions = ["GamepadLeftStickUp", "GamepadLeftStickDown", "GamepadLeftStickLeft", "GamepadLeftStickRight"];
const RIGHT_STICK: Directions = ["GamepadRightStickUp", "GamepadRightStickDown", "GamepadRightStickLeft", "GamepadRightStickRight"];

impl Reading {
    /// 这一帧按着的键名。`held` 是这个手柄上一帧按着的（扳机的回差要看它）。
    pub fn keys(&self, held: &BTreeSet<&'static str>, out: &mut BTreeSet<&'static str>) {
        let squeezed = |value: f32, code: &str| value > if held.contains(code) { TRIGGER_RELEASE } else { PRESS };
        let buttons = [
            (self.a, "GamepadA"),
            (self.b, "GamepadB"),
            (self.x, "GamepadX"),
            (self.y, "GamepadY"),
            (self.l1, "GamepadL1"),
            (self.r1, "GamepadR1"),
            (self.l3, "GamepadL3"),
            (self.r3, "GamepadR3"),
            (self.menu, "GamepadMenu"),
            (self.options, "GamepadOptions"),
            (squeezed(self.l2, "GamepadL2"), "GamepadL2"),
            (squeezed(self.r2, "GamepadR2"), "GamepadR2"),
        ];
        out.extend(buttons.into_iter().filter(|(down, _)| *down).map(|(_, code)| code));
        for (axis, names) in [(self.dpad, DPAD), (self.left, LEFT_STICK), (self.right, RIGHT_STICK)] {
            let [up, down, left, right] = names;
            let [x, y] = axis;
            let pressed = [(y > PRESS, up), (y < -PRESS, down), (x < -PRESS, left), (x > PRESS, right)];
            out.extend(pressed.into_iter().filter(|(down, _)| *down).map(|(_, code)| code));
        }
    }
}

/// 模拟量：两根摇杆（去掉死区）和两个扳机（0 … 1）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Analog {
    pub sticks: [[f32; 2]; 2],
    pub triggers: [f32; 2],
}

impl Analog {
    /// `gamepad` command 的样子。
    pub fn input(&self) -> GamepadInput {
        let [left, right] = self.sticks;
        GamepadInput { left, right, l2: self.triggers[0], r2: self.triggers[1] }
    }
}

/// 一个手柄的状态：按着的键名和模拟量。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub keys: BTreeSet<&'static str>,
    pub analog: Analog,
}

impl State {
    /// `pad` command 的样子。
    pub fn input(&self, id: u32) -> PadInput {
        let [left, right] = self.analog.sticks;
        let [l2, r2] = self.analog.triggers;
        PadInput { id, connected: true, keys: self.keys.iter().map(|k| (*k).to_owned()).collect(), left, right, l2, r2 }
    }
}

impl Reading {
    /// 去掉死区的摇杆、限制在 0 … 1 的扳机。
    pub fn analog(&self) -> Analog {
        Analog { sticks: [deadzone(self.left), deadzone(self.right)], triggers: [trigger(self.l2), trigger(self.r2)] }
    }

    /// 这个手柄自己的状态；`held` 是它上一帧按着的键。
    pub fn state(&self, held: &BTreeSet<&'static str>) -> State {
        let mut keys = BTreeSet::new();
        self.keys(held, &mut keys);
        State { keys, analog: self.analog() }
    }
}

/// 几个手柄合起来：任何一个按着就算按着；每根摇杆取推得最远的那个，扳机取按得最深的。`held` 是上一帧合起来按着的。
pub fn merge(readings: &[Reading], held: &BTreeSet<&'static str>) -> (BTreeSet<&'static str>, Analog) {
    let mut keys = BTreeSet::new();
    let mut merged = Analog::default();
    for r in readings {
        r.keys(held, &mut keys);
        let analog = r.analog();
        for (best, v) in merged.sticks.iter_mut().zip(analog.sticks) {
            if length(v) > length(*best) {
                *best = v;
            }
        }
        for (best, v) in merged.triggers.iter_mut().zip(analog.triggers) {
            *best = best.max(v);
        }
    }
    (keys, merged)
}

/// 一帧的变化：先松开、再按下；摇杆、扳机变了时给新值。
#[derive(Debug, Default, PartialEq)]
pub struct Changes {
    pub released: Vec<&'static str>,
    pub pressed: Vec<&'static str>,
    pub analog: Option<Analog>,
}

pub fn diff(before: &BTreeSet<&'static str>, now: &BTreeSet<&'static str>, analog_before: Analog, analog_now: Analog) -> Changes {
    Changes {
        released: before.difference(now).copied().collect(),
        pressed: now.difference(before).copied().collect(),
        analog: (analog_before != analog_now).then_some(analog_now),
    }
}

/// 每个手柄上次发出去的状态：变了才发 `pad` command，第一次见到就是接入。
#[derive(Debug, Default)]
pub struct Devices(BTreeMap<u32, State>);

impl Devices {
    /// 手柄 `id` 现在的状态；新接入的、变了的返回要发的 command。
    pub fn set(&mut self, id: u32, state: State) -> Option<PadInput> {
        if self.0.get(&id) == Some(&state) {
            return None;
        }
        let input = state.input(id);
        self.0.insert(id, state);
        Some(input)
    }

    /// 手柄 `id` 拔掉了。
    pub fn remove(&mut self, id: u32) -> Option<PadInput> {
        self.0.remove(&id).map(|_| PadInput { id, connected: false, ..PadInput::default() })
    }

    /// 这一帧读到的所有手柄：读不到了的算拔出，其余的照 [`Devices::set`]。
    pub fn update(&mut self, now: Vec<(u32, State)>) -> Vec<PadInput> {
        let gone: Vec<u32> = self.0.keys().copied().filter(|id| !now.iter().any(|(n, _)| n == id)).collect();
        let mut out: Vec<PadInput> = gone.into_iter().filter_map(|id| self.remove(id)).collect();
        out.extend(now.into_iter().filter_map(|(id, state)| self.set(id, state)));
        out
    }

    /// 这一帧读到的所有手柄的读数：每个按它自己上一帧的状态算按着的键，再照 [`Devices::update`]。
    pub fn read(&mut self, readings: &[(u32, Reading)]) -> Vec<PadInput> {
        let states = readings.iter().map(|(id, r)| (*id, r.state(&self.keys(*id)))).collect();
        self.update(states)
    }

    /// 手柄 `id` 上次按着的键；没见过的是空的。
    pub fn keys(&self, id: u32) -> BTreeSet<&'static str> {
        self.0.get(&id).map(|state| state.keys.clone()).unwrap_or_default()
    }

    /// 全部松开（窗口失焦、进后台）：手柄还连着，按着的发一份空状态。
    pub fn release(&mut self) -> Vec<PadInput> {
        let ids = self.ids();
        ids.into_iter().filter_map(|id| self.set(id, State::default())).collect()
    }

    /// 连着的手柄号。
    pub fn ids(&self) -> Vec<u32> {
        self.0.keys().copied().collect()
    }
}

/// 给手柄编号（平台自己没有号时）：同一个手柄一直是同一个号，新来的用最小的空号。
#[derive(Debug)]
pub struct Ids<K>(Vec<Option<K>>);

impl<K> Default for Ids<K> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<K: PartialEq + Copy> Ids<K> {
    /// `present` 是现在连着的手柄；返回它们各自的号。
    pub fn assign(&mut self, present: &[K]) -> Vec<u32> {
        for slot in &mut self.0 {
            if slot.is_some_and(|k| !present.contains(&k)) {
                *slot = None;
            }
        }
        let mut ids = Vec::with_capacity(present.len());
        for key in present {
            let known = self.0.iter().position(|slot| slot.as_ref() == Some(key));
            let index = known.or_else(|| self.0.iter().position(Option::is_none)).unwrap_or_else(|| {
                self.0.push(None);
                self.0.len() - 1
            });
            self.0[index] = Some(*key);
            ids.push(index as u32);
        }
        ids
    }
}

/// 每个手柄号现在是哪个手柄的（平台自己编号时）：号换了主人、主人不见了，按号缓存的东西（震动引擎）就是旧手柄的，要丢掉。
#[derive(Debug)]
pub struct Owners<K>(BTreeMap<u32, K>);

impl<K> Default for Owners<K> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}

impl<K: PartialEq + Copy> Owners<K> {
    /// `now` 是现在每个号的手柄；返回原来有主人、现在换了或者没了的号。
    pub fn changed(&mut self, now: &[(u32, K)]) -> Vec<u32> {
        let stale = self.0.iter().filter(|(id, owner)| !now.contains(&(**id, **owner))).map(|(id, _)| *id).collect();
        self.0 = now.iter().copied().collect();
        stale
    }
}

fn deadzone(v: [f32; 2]) -> [f32; 2] {
    let v = v.map(|c| if c.is_finite() { c.clamp(-1.0, 1.0) } else { 0.0 });
    if length(v) < DEADZONE { [0.0; 2] } else { v }
}

fn trigger(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

fn length([x, y]: [f32; 2]) -> f32 {
    x.hypot(y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 上一帧什么都没按着。
    const NONE: BTreeSet<&'static str> = BTreeSet::new();

    fn keys(r: Reading) -> Vec<&'static str> {
        merge(&[r], &NONE).0.into_iter().collect()
    }

    #[test]
    fn buttons_and_triggers() {
        assert!(keys(Reading::default()).is_empty());
        let r = Reading { a: true, menu: true, l2: 0.6, r2: 0.4, ..Default::default() };
        assert_eq!(keys(r), ["GamepadA", "GamepadL2", "GamepadMenu"], "扳机过一半才算");
    }

    #[test]
    fn directions_from_dpad_and_sticks() {
        let r = Reading { dpad: [-1.0, 0.0], left: [0.7, 0.7], right: [0.0, -0.4], ..Default::default() };
        assert_eq!(keys(r), ["GamepadLeft", "GamepadLeftStickRight", "GamepadLeftStickUp"], "斜推两个方向都算，没过一半不算");
    }

    #[test]
    fn sticks_have_a_deadzone() {
        let r = Reading { left: [0.1, 0.1], right: [0.3, -0.2], ..Default::default() };
        assert_eq!(merge(&[r], &NONE).1.sticks, [[0.0, 0.0], [0.3, -0.2]]);
    }

    #[test]
    fn several_pads_are_merged() {
        let one = Reading { a: true, left: [0.2, 0.0], ..Default::default() };
        let two = Reading { b: true, left: [0.0, -0.9], ..Default::default() };
        let (keys, analog) = merge(&[one, two], &NONE);
        assert_eq!(keys.into_iter().collect::<Vec<_>>(), ["GamepadA", "GamepadB", "GamepadLeftStickDown"]);
        assert_eq!(analog.sticks[0], [0.0, -0.9], "摇杆取推得最远的");
    }

    #[test]
    fn diff_releases_then_presses_and_reports_moved_sticks() {
        let before = BTreeSet::from(["GamepadA", "GamepadB"]);
        let now = BTreeSet::from(["GamepadB", "GamepadX"]);
        let still = Analog { sticks: [[0.5, 0.0], [0.0; 2]], triggers: [0.0; 2] };
        let c = diff(&before, &now, still, still);
        assert_eq!(c, Changes { released: vec!["GamepadA"], pressed: vec!["GamepadX"], analog: None });
        let moved = Analog::default();
        assert_eq!(diff(&now, &now, still, moved).analog, Some(moved));
        let squeezed = Analog { triggers: [0.25, 0.0], ..still };
        assert_eq!(diff(&now, &now, still, squeezed).analog, Some(squeezed), "扳机变了也发");
        assert_eq!(diff(&now, &BTreeSet::new(), still, still).released, ["GamepadB", "GamepadX"], "全部松开");
    }

    #[test]
    fn bad_axis_values_are_zero() {
        let r = Reading { left: [f32::NAN, 2.0], ..Default::default() };
        assert_eq!(merge(&[r], &NONE).1.sticks[0], [0.0, 1.0]);
    }

    #[test]
    fn triggers_are_analog_and_merged_by_the_deepest() {
        let one = Reading { l2: 0.25, r2: 2.0, ..Default::default() };
        let two = Reading { l2: 0.75, r2: f32::NAN, ..Default::default() };
        assert_eq!(one.analog().triggers, [0.25, 1.0], "限制在 0 … 1");
        assert_eq!(two.analog().triggers, [0.75, 0.0], "坏的值当 0");
        assert_eq!(Reading { l2: -1.0, ..Default::default() }.analog().triggers, [0.0, 0.0]);
        assert_eq!(merge(&[one, two], &NONE).1.triggers, [0.75, 1.0], "几个手柄取按得最深的");
        assert_eq!(merge(&[one], &NONE).1.input(), GamepadInput { left: [0.0; 2], right: [0.0; 2], l2: 0.25, r2: 1.0 });
    }

    #[test]
    fn a_reading_becomes_the_state_of_one_pad() {
        let r = Reading { a: true, l2: 0.75, left: [0.1, 0.0], right: [0.0, -1.0], ..Default::default() };
        let state = r.state(&NONE);
        assert_eq!(state.keys.iter().copied().collect::<Vec<_>>(), ["GamepadA", "GamepadL2", "GamepadRightStickDown"]);
        assert_eq!(state.analog, Analog { sticks: [[0.0; 2], [0.0, -1.0]], triggers: [0.75, 0.0] }, "摇杆去掉死区");
        let wire = state.input(4);
        assert_eq!(wire, PadInput { id: 4, connected: true, keys: vec!["GamepadA".into(), "GamepadL2".into(), "GamepadRightStickDown".into()], left: [0.0; 2], right: [0.0, -1.0], l2: 0.75, r2: 0.0 });
    }

    fn state(keys: &[&'static str]) -> State {
        State { keys: keys.iter().copied().collect(), analog: Analog::default() }
    }

    #[test]
    fn devices_report_new_changed_and_gone_pads() {
        let mut devices = Devices::default();
        let first = devices.update(vec![(0, state(&[])), (1, state(&["GamepadA"]))]);
        assert_eq!(first.iter().map(|p| (p.id, p.connected, p.keys.len())).collect::<Vec<_>>(), [(0, true, 0), (1, true, 1)], "第一次见到就发：接入");
        assert!(devices.update(vec![(0, state(&[])), (1, state(&["GamepadA"]))]).is_empty(), "没变不发");
        let changed = devices.update(vec![(0, state(&[])), (1, state(&[]))]);
        assert_eq!(changed.iter().map(|p| p.id).collect::<Vec<_>>(), [1], "只发变了的");
        let gone = devices.update(vec![(1, state(&[]))]);
        assert_eq!(gone, [PadInput { id: 0, connected: false, ..Default::default() }], "读不到了：拔出");
        assert_eq!(devices.ids(), [1]);
        assert!(devices.remove(0).is_none(), "已经不在了");
        assert_eq!(devices.remove(1).map(|p| p.connected), Some(false));
    }

    #[test]
    fn releasing_keeps_the_pads_connected() {
        let mut devices = Devices::default();
        devices.update(vec![(0, state(&["GamepadA"])), (1, state(&[]))]);
        let released = devices.release();
        assert_eq!(released, [PadInput { id: 0, connected: true, ..Default::default() }], "按着的发一份空状态，没按的不用发");
        assert_eq!(devices.ids(), [0, 1], "失焦不算拔出");
        assert!(devices.release().is_empty());
        assert_eq!(devices.set(0, state(&["GamepadA"])).map(|p| p.keys), Some(vec!["GamepadA".to_owned()]), "回来接着读");
        assert!(devices.set(0, state(&["GamepadA"])).is_none());
    }

    #[test]
    fn ids_stay_with_their_pad_and_reuse_the_smallest_free_one() {
        let mut ids = Ids::default();
        assert_eq!(ids.assign(&['a', 'b']), [0, 1]);
        assert_eq!(ids.assign(&['b', 'a']), [1, 0], "顺序变了号不变");
        assert_eq!(ids.assign(&['b']), [1], "a 拔掉了，b 还是 1");
        assert_eq!(ids.assign(&['c', 'b', 'd']), [0, 1, 2], "新来的用最小的空号");
        assert_eq!(ids.assign(&[]), [] as [u32; 0]);
        assert_eq!(ids.assign(&['d']), [0]);
    }

    #[test]
    fn a_trigger_lets_go_lower_than_it_presses() {
        let at = |l2: f32| Reading { l2, r2: l2, ..Default::default() };
        let held = BTreeSet::from(["GamepadL2"]);
        let pressed = |r: Reading, held: &BTreeSet<&'static str>| r.state(held).keys.into_iter().collect::<Vec<_>>();
        assert!(pressed(at(0.45), &NONE).is_empty(), "没按着：过 0.5 才算");
        assert_eq!(pressed(at(0.45), &held), ["GamepadL2"], "按着的：掉到 0.4 以下才松，R2 没按着不算");
        assert!(pressed(at(0.35), &held).is_empty());
        assert_eq!(pressed(at(0.55), &NONE), ["GamepadL2", "GamepadR2"]);
        assert_eq!(merge(&[at(0.45)], &held).0.into_iter().collect::<Vec<_>>(), ["GamepadL2"], "合并的那条路也一样");
        // 抖在门槛附近：每个手柄按自己上次的状态算，不来回按下松开
        let mut devices = Devices::default();
        assert!(devices.keys(9).is_empty());
        let downs: Vec<bool> = [0.49, 0.51, 0.49, 0.45, 0.39, 0.45, 0.51].iter().map(|&l2| {
            devices.read(&[(0, Reading { l2, ..Default::default() })]);
            devices.keys(0).contains("GamepadL2")
        }).collect();
        assert_eq!(downs, [false, true, true, true, false, false, true]);
    }

    #[test]
    fn a_number_that_changes_hands_is_noticed() {
        let mut owners = Owners::default();
        assert!(owners.changed(&[(0, 'a'), (1, 'b')]).is_empty(), "第一次见：没有旧主人");
        assert!(owners.changed(&[(1, 'b'), (0, 'a')]).is_empty());
        assert_eq!(owners.changed(&[(0, 'c'), (1, 'b')]), [0], "0 号换了手柄：它的震动引擎是旧手柄的，要丢掉");
        assert_eq!(owners.changed(&[(0, 'c')]), [1], "拔掉的也算");
        assert!(owners.changed(&[(0, 'c'), (1, 'd')]).is_empty(), "空出来的号给新手柄：没有旧的要丢");
        assert_eq!(owners.changed(&[]), [0, 1]);
    }
}
