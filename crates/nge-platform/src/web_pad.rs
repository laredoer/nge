//! 浏览器的手柄：`Gamepad` 的按钮和轴（`mapping` 是 `"standard"` 的）换成和 Mac 一样的读数（见 docs/input-actions.md）。
//! 纯函数，不依赖浏览器，在 Mac 上也能测。

use nge_core::bridge::RumbleOut;

use crate::pad::Reading;
use crate::rumble::{millis, unit};

/// 按钮按到这个值算按着。
const PRESS: f32 = 0.5;

/// 这个手柄读不读：只认标准布局（Xbox 那样排的 17 个按钮、4 根轴）、还连着的。
pub fn usable(mapping: &str, connected: bool) -> bool {
    connected && mapping == "standard"
}

/// 标准布局的按钮值（0 … 1）和轴（y 朝下）→ 读数。Home（16）留给系统。
pub fn reading(buttons: &[f32], axes: &[f32]) -> Reading {
    let value = |i: usize| buttons.get(i).copied().unwrap_or(0.0);
    let pressed = |i: usize| value(i) > PRESS;
    let axis = |i: usize| axes.get(i).copied().unwrap_or(0.0);
    let pair = |negative: usize, positive: usize| f32::from(i8::from(pressed(positive)) - i8::from(pressed(negative)));
    Reading {
        a: pressed(0),
        b: pressed(1),
        x: pressed(2),
        y: pressed(3),
        l1: pressed(4),
        r1: pressed(5),
        l2: value(6),
        r2: value(7),
        options: pressed(8),
        menu: pressed(9),
        l3: pressed(10),
        r3: pressed(11),
        dpad: [pair(14, 15), pair(13, 12)],
        left: [axis(0), -axis(1)],
        right: [axis(2), -axis(3)],
    }
}

/// `vibrationActuator.playEffect("dual-rumble", …)` 的参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Effect {
    pub duration_ms: f64,
    /// 低频（重）马达。
    pub strong: f64,
    /// 高频（轻）马达。
    pub weak: f64,
}

pub fn effect(rumble: &RumbleOut) -> Effect {
    Effect { duration_ms: f64::from(millis(rumble.duration)), strong: f64::from(unit(rumble.low)), weak: f64::from(unit(rumble.high)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 标准布局的 17 个按钮，只有 `pressed` 里的按着。
    fn buttons(pressed: &[(usize, f32)]) -> [f32; 17] {
        let mut b = [0.0; 17];
        for &(i, v) in pressed {
            b[i] = v;
        }
        b
    }

    #[test]
    fn standard_buttons_become_the_same_reading_as_on_mac() {
        let r = reading(&buttons(&[(0, 1.0), (5, 1.0), (9, 1.0), (10, 1.0)]), &[0.0; 4]);
        assert!(r.a && r.r1 && r.menu && r.l3);
        assert!(!r.b && !r.x && !r.y && !r.l1 && !r.options && !r.r3);
        let r = reading(&buttons(&[(1, 1.0), (2, 1.0), (3, 1.0), (4, 1.0), (8, 1.0), (11, 1.0), (16, 1.0)]), &[0.0; 4]);
        assert!(r.b && r.x && r.y && r.l1 && r.options && r.r3 && !r.a && !r.menu, "Home（16）不认");
        assert_eq!(reading(&buttons(&[(0, 0.4)]), &[]), Reading::default(), "没按过一半不算；缺的轴当 0");
    }

    #[test]
    fn triggers_dpad_and_sticks() {
        let r = reading(&buttons(&[(6, 0.25), (7, 1.0), (12, 1.0), (15, 1.0)]), &[0.5, -1.0, -0.25, 0.75]);
        assert_eq!((r.l2, r.r2), (0.25, 1.0), "扳机是模拟量");
        assert_eq!(r.dpad, [1.0, 1.0], "十字键：上、右");
        assert_eq!(reading(&buttons(&[(13, 1.0), (14, 1.0)]), &[]).dpad, [-1.0, -1.0], "下、左");
        assert_eq!((r.left, r.right), ([0.5, 1.0], [-0.25, -0.75]), "浏览器的 y 朝下，换成朝上");
        assert_eq!(reading(&[1.0], &[0.5]).left, [0.5, 0.0], "按钮、轴不够长的不崩");
    }

    #[test]
    fn only_standard_pads_are_read() {
        assert!(usable("standard", true));
        assert!(!usable("", true), "布局不标准的不认：按钮顺序没有准");
        assert!(!usable("standard", false), "断开了的不读");
    }

    #[test]
    fn rumble_in_browser_units() {
        let e = effect(&RumbleOut { pad: None, low: 1.0, high: 0.25, duration: 0.5 });
        assert_eq!(e, Effect { duration_ms: 500.0, strong: 1.0, weak: 0.25 }, "低频是 strongMagnitude，高频是 weakMagnitude");
        let e = effect(&RumbleOut { pad: None, low: 7.0, high: f32::NAN, duration: 60.0 });
        assert_eq!(e, Effect { duration_ms: 5000.0, strong: 1.0, weak: 0.0 });
    }
}
