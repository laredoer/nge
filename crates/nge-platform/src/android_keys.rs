//! Android 的硬件键盘和手柄：键码、手柄的轴换成和 Mac 一样的键名和 `pad::Reading`（见 docs/android.md）。
//! 纯函数，不依赖 JNI，在 Mac 上也能测。

use std::collections::BTreeMap;

use crate::pad::Reading;

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

/// 一个手柄的轴（`nsh_platform_android::AXES` 的顺序）→ 读数；按钮走按键事件，这里都是 false。
pub fn reading(axes: &[f32]) -> Reading {
    let at = |i: usize| axes.get(i).copied().unwrap_or(0.0);
    Reading {
        left: [at(0), -at(1)],
        right: [at(2), -at(3)],
        dpad: [at(4), -at(5)],
        l2: at(6).max(at(8)),
        r2: at(7).max(at(9)),
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

    pub fn readings(&self) -> Vec<Reading> {
        self.0.values().copied().collect()
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
