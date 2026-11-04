//! 手柄：一帧的读数 → 按着的键名和去掉死区的摇杆（见 docs/gamepad.md）。纯函数，不依赖 GameController。

use std::collections::BTreeSet;

/// 摇杆方向、扳机「按着」的门槛。
const PRESS: f32 = 0.5;
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
    /// 这一帧按着的键名。
    pub fn keys(&self, out: &mut BTreeSet<&'static str>) {
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
            (self.l2 > PRESS, "GamepadL2"),
            (self.r2 > PRESS, "GamepadR2"),
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

/// 几个手柄合起来：任何一个按着就算按着；每根摇杆取推得最远的那个，去掉死区。
pub fn merge(readings: &[Reading]) -> (BTreeSet<&'static str>, [[f32; 2]; 2]) {
    let mut keys = BTreeSet::new();
    let mut sticks = [[0.0; 2]; 2];
    for r in readings {
        r.keys(&mut keys);
        for (best, v) in sticks.iter_mut().zip([r.left, r.right]) {
            let v = deadzone(v);
            if length(v) > length(*best) {
                *best = v;
            }
        }
    }
    (keys, sticks)
}

/// 一帧的变化：先松开、再按下；摇杆变了时给新值。
#[derive(Debug, Default, PartialEq)]
pub struct Changes {
    pub released: Vec<&'static str>,
    pub pressed: Vec<&'static str>,
    pub sticks: Option<[[f32; 2]; 2]>,
}

pub fn diff(before: &BTreeSet<&'static str>, now: &BTreeSet<&'static str>, sticks_before: [[f32; 2]; 2], sticks_now: [[f32; 2]; 2]) -> Changes {
    Changes {
        released: before.difference(now).copied().collect(),
        pressed: now.difference(before).copied().collect(),
        sticks: (sticks_before != sticks_now).then_some(sticks_now),
    }
}

fn deadzone(v: [f32; 2]) -> [f32; 2] {
    let v = v.map(|c| if c.is_finite() { c.clamp(-1.0, 1.0) } else { 0.0 });
    if length(v) < DEADZONE { [0.0; 2] } else { v }
}

fn length([x, y]: [f32; 2]) -> f32 {
    x.hypot(y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(r: Reading) -> Vec<&'static str> {
        merge(&[r]).0.into_iter().collect()
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
        assert_eq!(merge(&[r]).1, [[0.0, 0.0], [0.3, -0.2]]);
    }

    #[test]
    fn several_pads_are_merged() {
        let one = Reading { a: true, left: [0.2, 0.0], ..Default::default() };
        let two = Reading { b: true, left: [0.0, -0.9], ..Default::default() };
        let (keys, sticks) = merge(&[one, two]);
        assert_eq!(keys.into_iter().collect::<Vec<_>>(), ["GamepadA", "GamepadB", "GamepadLeftStickDown"]);
        assert_eq!(sticks[0], [0.0, -0.9], "摇杆取推得最远的");
    }

    #[test]
    fn diff_releases_then_presses_and_reports_moved_sticks() {
        let before = BTreeSet::from(["GamepadA", "GamepadB"]);
        let now = BTreeSet::from(["GamepadB", "GamepadX"]);
        let still = [[0.5, 0.0], [0.0; 2]];
        let c = diff(&before, &now, still, still);
        assert_eq!(c, Changes { released: vec!["GamepadA"], pressed: vec!["GamepadX"], sticks: None });
        let moved = [[0.0; 2]; 2];
        assert_eq!(diff(&now, &now, still, moved).sticks, Some(moved));
        assert_eq!(diff(&now, &BTreeSet::new(), still, still).released, ["GamepadB", "GamepadX"], "全部松开");
    }

    #[test]
    fn bad_axis_values_are_zero() {
        let r = Reading { left: [f32::NAN, 2.0], ..Default::default() };
        assert_eq!(merge(&[r]).1[0], [0.0, 1.0]);
    }
}
