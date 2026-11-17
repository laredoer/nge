//! Android 的硬件键盘和手柄：键码、手柄的轴换成和 Mac 一样的键名和 `pad::Reading`（见 docs/android.md）。
//! 纯函数，不依赖 JNI，在 Mac 上也能测。

use std::collections::{BTreeMap, BTreeSet};

use crate::pad::{self, Reading};

/// 键盘的键码（`KeyEvent.KEYCODE_*`）→ 键名（Web `KeyboardEvent.code`），和 Mac 的键名表一样多。
const KEYBOARD: &[(i32, &str)] = &[
    (29, "KeyA"), (30, "KeyB"), (31, "KeyC"), (32, "KeyD"), (33, "KeyE"), (34, "KeyF"), (35, "KeyG"), (36, "KeyH"), (37, "KeyI"),
    (38, "KeyJ"), (39, "KeyK"), (40, "KeyL"), (41, "KeyM"), (42, "KeyN"), (43, "KeyO"), (44, "KeyP"), (45, "KeyQ"), (46, "KeyR"),
    (47, "KeyS"), (48, "KeyT"), (49, "KeyU"), (50, "KeyV"), (51, "KeyW"), (52, "KeyX"), (53, "KeyY"), (54, "KeyZ"),
    (7, "Digit0"), (8, "Digit1"), (9, "Digit2"), (10, "Digit3"), (11, "Digit4"), (12, "Digit5"), (13, "Digit6"), (14, "Digit7"),
    (15, "Digit8"), (16, "Digit9"),
    (131, "F1"), (132, "F2"), (133, "F3"), (134, "F4"), (135, "F5"), (136, "F6"), (137, "F7"), (138, "F8"), (139, "F9"), (140, "F10"),
    (141, "F11"), (142, "F12"),
    (62, "Space"), (66, "Enter"), (61, "Tab"), (111, "Escape"), (67, "Backspace"), (112, "Delete"),
    (122, "Home"), (123, "End"), (92, "PageUp"), (93, "PageDown"),
    (69, "Minus"), (70, "Equal"), (71, "BracketLeft"), (72, "BracketRight"), (73, "Backslash"), (74, "Semicolon"), (75, "Quote"),
    (68, "Backquote"), (55, "Comma"), (56, "Period"), (76, "Slash"),
    (59, "ShiftLeft"), (60, "ShiftRight"), (113, "ControlLeft"), (114, "ControlRight"), (57, "AltLeft"), (58, "AltRight"),
    (117, "MetaLeft"), (118, "MetaRight"),
];

/// 十字键的键码（上、下、左、右）：键盘上是方向键，手柄上是十字键。
const DPAD: [(i32, &str, &str); 4] = [(19, "ArrowUp", "GamepadUp"), (20, "ArrowDown", "GamepadDown"), (21, "ArrowLeft", "GamepadLeft"), (22, "ArrowRight", "GamepadRight")];

/// 手柄按钮的键码（`KEYCODE_BUTTON_*`）。Home（`BUTTON_MODE`）留给系统。
const BUTTONS: &[(i32, &str)] = &[
    (96, "GamepadA"), (97, "GamepadB"), (99, "GamepadX"), (100, "GamepadY"), (102, "GamepadL1"), (103, "GamepadR1"),
    (104, "GamepadL2"), (105, "GamepadR2"), (106, "GamepadL3"), (107, "GamepadR3"), (108, "GamepadMenu"), (109, "GamepadOptions"),
];

/// 键码对应的键名；`gamepad`：事件来自手柄（十字键算手柄的）。
pub fn key_name(code: i32, gamepad: bool) -> Option<&'static str> {
    if let Some((_, arrow, pad)) = DPAD.iter().find(|(c, ..)| *c == code) {
        return Some(if gamepad { pad } else { arrow });
    }
    KEYBOARD.iter().chain(BUTTONS).find(|(c, _)| *c == code).map(|(_, name)| *name)
}

/// `InputDevice.SOURCE_GAMEPAD`、`SOURCE_JOYSTICK`（都带着类别位，要整个对上）。
const SOURCE_GAMEPAD: i32 = 0x401;
const SOURCE_JOYSTICK: i32 = 0x0100_0010;

/// 事件来自手柄。键盘的方向键有时带 `SOURCE_DPAD`，不算。
pub fn from_gamepad(source: i32) -> bool {
    source & SOURCE_GAMEPAD == SOURCE_GAMEPAD || source & SOURCE_JOYSTICK == SOURCE_JOYSTICK
}

/// 按着 Ctrl 或 Meta（`META_CTRL_ON`、`META_META_ON`）：快捷键，留给 NSH。
pub fn shortcut(meta: i32) -> bool {
    meta & (0x1000 | 0x10000) != 0
}

/// 一个手柄的轴（`nsh::AXES` 的顺序：X、Y、Z、RZ、HAT_X、HAT_Y、LTRIGGER、RTRIGGER、BRAKE、GAS、RX、RY；
/// 设备没有的轴是 NaN）→ 读数；按钮走按键事件，这里都是 false。
///
/// 右摇杆：常见的手柄是 Z / RZ；没有扳机轴、却有 RX / RY 的（通用 HID 手柄）是 RX / RY，这时 Z / RZ 是扳机。
pub fn reading(axes: &[f32]) -> Reading {
    let get = |i: usize| axes.get(i).copied().filter(|v| !v.is_nan());
    let at = |i: usize| get(i).unwrap_or(0.0);
    let has = |i: usize| get(i).is_some();
    let triggers = [6, 7, 8, 9].into_iter().any(has);
    let rx_ry = has(10) && has(11) && !(has(2) && has(3) && triggers);
    let (right, z_triggers) = if rx_ry { ([at(10), -at(11)], [at(2), at(3)]) } else { ([at(2), -at(3)], [0.0; 2]) };
    Reading {
        left: [at(0), -at(1)],
        right,
        dpad: [at(4), -at(5)],
        l2: at(6).max(at(8)).max(z_triggers[0]),
        r2: at(7).max(at(9)).max(z_triggers[1]),
        ..Reading::default()
    }
}

/// 每个手柄最近一次的轴。
#[derive(Default)]
pub struct Pads(BTreeMap<i32, Reading>);

impl Pads {
    pub fn update(&mut self, device: i32, axes: &[f32]) {
        self.0.insert(device, reading(axes));
    }

    /// 全部松开（画面没了时）。
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn remove(&mut self, device: i32) {
        self.0.remove(&device);
    }

    pub fn readings(&self) -> Vec<Reading> {
        self.0.values().copied().collect()
    }
}

/// 手柄的按钮（按键事件）和轴合起来：两路来的同一个键（扳机的 L2 / R2、十字键）只算一次。
#[derive(Default)]
pub struct PadInput {
    buttons: BTreeMap<i32, BTreeSet<&'static str>>,
    pads: Pads,
    /// 转发过的：按着的键名和摇杆。
    held: BTreeSet<&'static str>,
    sticks: [[f32; 2]; 2],
}

impl PadInput {
    /// 设备 `device` 的按钮按下、松开。
    pub fn button(&mut self, device: i32, code: &'static str, down: bool) -> pad::Changes {
        let held = self.buttons.entry(device).or_default();
        if down {
            held.insert(code);
        } else {
            held.remove(code);
        }
        self.changes()
    }

    /// 设备 `device` 的轴。
    pub fn motion(&mut self, device: i32, axes: &[f32]) -> pad::Changes {
        self.pads.update(device, axes);
        self.changes()
    }

    /// 手柄拔掉了：它按着的都松开。
    pub fn remove(&mut self, device: i32) -> pad::Changes {
        self.buttons.remove(&device);
        self.pads.remove(device);
        self.changes()
    }

    /// 全部松开（画面没了、窗口失焦）。
    pub fn clear(&mut self) -> pad::Changes {
        self.buttons.clear();
        self.pads.clear();
        self.changes()
    }

    fn changes(&mut self) -> pad::Changes {
        let (mut now, sticks) = pad::merge(&self.pads.readings());
        now.extend(self.buttons.values().flatten().copied());
        let changes = pad::diff(&self.held, &now, self.sticks, sticks);
        self.held = now;
        self.sticks = sticks;
        changes
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const KEYBOARD: i32 = 0x101;
    const GAMEPAD: i32 = 0x401;
    const JOYSTICK: i32 = 0x0100_0010;
    const DPAD: i32 = 0x201;

    #[test]
    fn keyboard_keys_are_the_ones_mac_knows() {
        assert_eq!(key_name(29, false), Some("KeyA"));
        assert_eq!(key_name(54, false), Some("KeyZ"));
        assert_eq!(key_name(7, false), Some("Digit0"));
        assert_eq!(key_name(16, false), Some("Digit9"));
        assert_eq!(key_name(62, false), Some("Space"));
        assert_eq!(key_name(67, false), Some("Backspace"));
        assert_eq!(key_name(112, false), Some("Delete"));
        assert_eq!(key_name(4, false), None, "返回键留给系统");
        let android: BTreeSet<_> = (0..400).filter_map(|code| key_name(code, false)).filter(|n| !n.starts_with("Gamepad")).collect();
        let mac: BTreeSet<_> = crate::hid::USAGES.iter().map(|(_, name)| *name).collect();
        assert_eq!(android, mac, "和 Mac、iPad 转发同一组键");
    }

    #[test]
    fn gamepad_buttons_and_its_dpad() {
        assert_eq!(key_name(96, true), Some("GamepadA"));
        assert_eq!(key_name(97, true), Some("GamepadB"));
        assert_eq!(key_name(99, true), Some("GamepadX"));
        assert_eq!(key_name(100, true), Some("GamepadY"));
        assert_eq!(key_name(108, true), Some("GamepadMenu"));
        assert_eq!(key_name(109, true), Some("GamepadOptions"));
        assert_eq!(key_name(106, true), Some("GamepadL3"));
        assert_eq!(key_name(110, true), None, "Home 键留给系统");
        assert_eq!(key_name(21, true), Some("GamepadLeft"), "手柄的十字键");
        assert_eq!(key_name(21, false), Some("ArrowLeft"), "键盘的方向键");
        assert_eq!(key_name(96, false), Some("GamepadA"), "按钮的键码只有手柄有，不看来源");
    }

    #[test]
    fn gamepad_sources() {
        assert!(from_gamepad(GAMEPAD) && from_gamepad(JOYSTICK) && from_gamepad(GAMEPAD | KEYBOARD));
        assert!(!from_gamepad(KEYBOARD) && !from_gamepad(DPAD) && !from_gamepad(KEYBOARD | DPAD), "键盘的方向键带 DPAD 也不算手柄");
    }

    #[test]
    fn control_and_meta_are_shortcuts() {
        assert!(shortcut(0x1000) && shortcut(0x10000));
        assert!(!shortcut(0) && !shortcut(0x1) && !shortcut(0x2), "Shift、Alt 不算");
    }

    #[test]
    fn axes_become_a_reading_with_y_up() {
        // X、Y、Z、RZ、HAT_X、HAT_Y、LTRIGGER、RTRIGGER、BRAKE、GAS
        let r = reading(&[0.5, -1.0, -0.25, 0.75, 1.0, 1.0, 0.2, 0.0, 0.6, 0.9]);
        assert_eq!(r.left, [0.5, 1.0], "Android 的 y 朝下，换成朝上");
        assert_eq!(r.right, [-0.25, -0.75]);
        assert_eq!(r.dpad, [1.0, -1.0]);
        assert_eq!((r.l2, r.r2), (0.6, 0.9), "扳机取 TRIGGER 和 BRAKE / GAS 大的");
        assert!(!r.a && !r.menu, "按钮走按键事件");
        assert_eq!(reading(&[0.3]), crate::pad::Reading { left: [0.3, 0.0], ..Default::default() }, "缺的轴当 0");
    }

    /// 轴：X、Y、Z、RZ、HAT_X、HAT_Y、LTRIGGER、RTRIGGER、BRAKE、GAS、RX、RY；设备没有的是 NaN。
    fn axes(set: &[(usize, f32)]) -> [f32; 12] {
        let mut a = [f32::NAN; 12];
        for i in [0, 1, 2, 3, 4, 5, 6, 7] {
            a[i] = 0.0;
        }
        for &(i, v) in set {
            a[i] = v;
        }
        a
    }

    #[test]
    fn the_right_stick_is_z_rz_or_rx_ry_by_what_the_device_has() {
        let xbox = axes(&[(2, 0.5), (3, -0.5)]);
        assert_eq!(reading(&xbox).right, [0.5, 0.5], "常见的手柄：Z / RZ");
        let mut generic = axes(&[(10, -0.5), (11, 1.0)]);
        generic[2] = 0.8;
        generic[3] = -1.0;
        generic[6] = f32::NAN;
        generic[7] = f32::NAN;
        let r = reading(&generic);
        assert_eq!(r.right, [-0.5, -1.0], "没有扳机轴、有 RX / RY：右摇杆是 RX / RY");
        assert_eq!((r.l2, r.r2), (0.8, 0.0), "这时 Z / RZ 是扳机（没按着是 -1）");
        let mut both = axes(&[(2, 0.25), (3, 0.0), (10, 1.0), (11, 1.0)]);
        both[6] = 0.0;
        assert_eq!(reading(&both).right, [0.25, 0.0], "有扳机轴时 Z / RZ 是摇杆");
    }

    #[test]
    fn a_trigger_reported_as_a_key_and_an_axis_is_pressed_once() {
        let mut pads = PadInput::default();
        let c = pads.button(1, "GamepadL2", true);
        assert_eq!(c.pressed, ["GamepadL2"]);
        let c = pads.motion(1, &axes(&[(6, 1.0)]));
        assert!(c.pressed.is_empty() && c.released.is_empty(), "轴也按着：不再发一次 {:?}", c.pressed);
        let c = pads.button(1, "GamepadL2", false);
        assert!(c.released.is_empty(), "轴还按着");
        let c = pads.motion(1, &axes(&[(6, 0.0)]));
        assert_eq!(c.released, ["GamepadL2"]);
    }

    #[test]
    fn a_removed_pad_lets_go_of_its_keys_and_sticks() {
        let mut pads = PadInput::default();
        pads.button(1, "GamepadA", true);
        pads.button(2, "GamepadB", true);
        let c = pads.motion(1, &axes(&[(0, -1.0)]));
        assert_eq!(c.pressed, ["GamepadLeftStickLeft"]);
        assert_eq!(c.sticks, Some([[-1.0, 0.0], [0.0, 0.0]]));
        let c = pads.remove(1);
        assert_eq!(c.released, ["GamepadA", "GamepadLeftStickLeft"], "拔掉的手柄松开");
        assert_eq!(c.sticks, Some([[0.0; 2]; 2]), "摇杆归零");
        let c = pads.clear();
        assert_eq!(c.released, ["GamepadB"], "失焦：全部松开");
        assert!(pads.clear().released.is_empty());
    }

    #[test]
    fn pads_keep_each_device_and_forget_all_on_release() {
        let mut pads = Pads::default();
        pads.update(1, &[1.0, 0.0]);
        pads.update(2, &[0.0, 1.0]);
        assert_eq!(pads.readings().len(), 2, "每个手柄一份");
        pads.update(1, &[0.0, 0.0]);
        assert_eq!(pads.readings().len(), 2);
        pads.clear();
        assert!(pads.readings().is_empty());
    }
}
