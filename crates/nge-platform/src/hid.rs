//! iOS 的硬件键盘：GameController 的 `GCKeyboard` 按 HID 用法码给每个键的状态，换成和 macOS 一样的键名（见 docs/ios-input.md）。
//! 纯函数，不依赖 GameController，在 Mac 上也能测。


use std::collections::BTreeSet;

/// HID 键盘用法码（`GCKeyCode` 的值）→ 键名（Web `KeyboardEvent.code`）。和 macOS 的键名表一样多。
pub const USAGES: &[(isize, &str)] = &[
    // 字母
    (4, "KeyA"), (5, "KeyB"), (6, "KeyC"), (7, "KeyD"), (8, "KeyE"), (9, "KeyF"), (10, "KeyG"), (11, "KeyH"), (12, "KeyI"), (13, "KeyJ"),
    (14, "KeyK"), (15, "KeyL"), (16, "KeyM"), (17, "KeyN"), (18, "KeyO"), (19, "KeyP"), (20, "KeyQ"), (21, "KeyR"), (22, "KeyS"),
    (23, "KeyT"), (24, "KeyU"), (25, "KeyV"), (26, "KeyW"), (27, "KeyX"), (28, "KeyY"), (29, "KeyZ"),
    // 数字（HID 里 0 排在 9 后面）
    (30, "Digit1"), (31, "Digit2"), (32, "Digit3"), (33, "Digit4"), (34, "Digit5"), (35, "Digit6"), (36, "Digit7"), (37, "Digit8"),
    (38, "Digit9"), (39, "Digit0"),
    // 回车、符号
    (40, "Enter"), (41, "Escape"), (42, "Backspace"), (43, "Tab"), (44, "Space"), (45, "Minus"), (46, "Equal"), (47, "BracketLeft"),
    (48, "BracketRight"), (49, "Backslash"), (51, "Semicolon"), (52, "Quote"), (53, "Backquote"), (54, "Comma"), (55, "Period"),
    (56, "Slash"),
    // F1 … F12
    (58, "F1"), (59, "F2"), (60, "F3"), (61, "F4"), (62, "F5"), (63, "F6"), (64, "F7"), (65, "F8"), (66, "F9"), (67, "F10"), (68, "F11"),
    (69, "F12"),
    // 编辑和方向键
    (74, "Home"), (75, "PageUp"), (76, "Delete"), (77, "End"), (78, "PageDown"), (79, "ArrowRight"), (80, "ArrowLeft"), (81, "ArrowDown"),
    (82, "ArrowUp"),
    // 修饰键
    (224, "ControlLeft"), (225, "ShiftLeft"), (226, "AltLeft"), (227, "MetaLeft"), (228, "ControlRight"), (229, "ShiftRight"),
    (230, "AltRight"), (231, "MetaRight"),
];

/// 用法码对应的键名。
#[cfg(test)]
pub fn code(usage: isize) -> Option<&'static str> {
    USAGES.iter().find(|(u, _)| *u == usage).map(|(_, c)| *c)
}

fn is_modifier(code: &str) -> bool {
    ["Control", "Shift", "Alt", "Meta"].iter().any(|m| code.starts_with(m))
}

/// 转发过的键：和这一帧按着的比，得出要发的按下 / 松开。
#[derive(Debug, Default)]
pub struct Keyboard {
    forwarded: BTreeSet<&'static str>,
    /// 按着 ⌘ 时按下的键（系统快捷键），松开前都不发。
    suppressed: BTreeSet<&'static str>,
}

impl Keyboard {
    /// `now` 是这一帧按着的键；返回要发的 (键名, 是否按下)，先松开后按下。传空集合就是全部松开。
    pub fn update(&mut self, now: &BTreeSet<&'static str>) -> Vec<(&'static str, bool)> {
        let released: Vec<_> = self.forwarded.difference(now).copied().collect();
        self.forwarded.retain(|c| now.contains(c));
        self.suppressed.retain(|c| now.contains(c));
        let command = now.contains("MetaLeft") || now.contains("MetaRight");
        let mut changes: Vec<_> = released.into_iter().map(|c| (c, false)).collect();
        for &c in now {
            if self.forwarded.contains(c) || self.suppressed.contains(c) {
                continue;
            }
            if command && !is_modifier(c) {
                self.suppressed.insert(c);
            } else {
                self.forwarded.insert(c);
                changes.push((c, true));
            }
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{Keyboard, USAGES, code};

    fn set(keys: &[&'static str]) -> BTreeSet<&'static str> {
        keys.iter().copied().collect()
    }

    #[test]
    fn usages_follow_the_hid_table() {
        assert_eq!(code(4), Some("KeyA"));
        assert_eq!(code(29), Some("KeyZ"));
        assert_eq!(code(30), Some("Digit1"));
        assert_eq!(code(39), Some("Digit0"));
        assert_eq!(code(44), Some("Space"));
        assert_eq!([code(79), code(80), code(81), code(82)], [Some("ArrowRight"), Some("ArrowLeft"), Some("ArrowDown"), Some("ArrowUp")]);
        assert_eq!([code(224), code(227), code(231)], [Some("ControlLeft"), Some("MetaLeft"), Some("MetaRight")]);
        assert_eq!(code(0), None);
        let names: BTreeSet<_> = USAGES.iter().map(|(_, c)| *c).collect();
        assert_eq!(names.len(), USAGES.len(), "键名不重复");
    }

    /// 和 macOS 的键名表是同一套名字：游戏按键逻辑不用分平台。
    #[cfg(target_os = "macos")]
    #[test]
    fn names_match_the_mac_keyboard() {
        let mac: BTreeSet<_> = (0..=u16::MAX).filter_map(crate::keys::code).collect();
        let ios: BTreeSet<_> = USAGES.iter().map(|(_, c)| *c).collect();
        assert_eq!(ios, mac);
    }

    #[test]
    fn presses_and_releases_are_sent_once() {
        let mut kb = Keyboard::default();
        assert_eq!(kb.update(&set(&["KeyA"])), [("KeyA", true)]);
        assert_eq!(kb.update(&set(&["KeyA"])), [], "按住不重复");
        assert_eq!(kb.update(&set(&["KeyB"])), [("KeyA", false), ("KeyB", true)], "先松开后按下");
        assert_eq!(kb.update(&set(&[])), [("KeyB", false)]);
    }

    #[test]
    fn keys_pressed_with_command_are_left_to_the_system() {
        let mut kb = Keyboard::default();
        assert_eq!(kb.update(&set(&["MetaLeft"])), [("MetaLeft", true)]);
        assert_eq!(kb.update(&set(&["MetaLeft", "KeyQ"])), [], "⌘Q 不发");
        assert_eq!(kb.update(&set(&["KeyQ"])), [("MetaLeft", false)], "松开 ⌘ 以后 Q 还按着：也不发");
        assert_eq!(kb.update(&set(&[])), []);
        assert_eq!(kb.update(&set(&["KeyQ"])), [("KeyQ", true)], "重新按下才发");
        // 先按着的键再按 ⌘：照常
        assert_eq!(kb.update(&set(&["KeyQ", "MetaRight", "ShiftLeft"])), [("MetaRight", true), ("ShiftLeft", true)], "修饰键照样发");
    }
}
