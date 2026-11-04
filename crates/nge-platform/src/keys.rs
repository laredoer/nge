//! macOS 虚拟键码（`kVK_*`，按键盘上的位置）→ Web `KeyboardEvent.code` 的键名。

/// 不认识的键（小键盘、媒体键……）返回 `None`，不转发。
pub fn code(key_code: u16) -> Option<&'static str> {
    Some(match key_code {
        0 => "KeyA",
        1 => "KeyS",
        2 => "KeyD",
        3 => "KeyF",
        4 => "KeyH",
        5 => "KeyG",
        6 => "KeyZ",
        7 => "KeyX",
        8 => "KeyC",
        9 => "KeyV",
        11 => "KeyB",
        12 => "KeyQ",
        13 => "KeyW",
        14 => "KeyE",
        15 => "KeyR",
        16 => "KeyY",
        17 => "KeyT",
        18 => "Digit1",
        19 => "Digit2",
        20 => "Digit3",
        21 => "Digit4",
        22 => "Digit6",
        23 => "Digit5",
        24 => "Equal",
        25 => "Digit9",
        26 => "Digit7",
        27 => "Minus",
        28 => "Digit8",
        29 => "Digit0",
        30 => "BracketRight",
        31 => "KeyO",
        32 => "KeyU",
        33 => "BracketLeft",
        34 => "KeyI",
        35 => "KeyP",
        36 => "Enter",
        37 => "KeyL",
        38 => "KeyJ",
        39 => "Quote",
        40 => "KeyK",
        41 => "Semicolon",
        42 => "Backslash",
        43 => "Comma",
        44 => "Slash",
        45 => "KeyN",
        46 => "KeyM",
        47 => "Period",
        48 => "Tab",
        49 => "Space",
        50 => "Backquote",
        51 => "Backspace",
        53 => "Escape",
        54 => "MetaRight",
        55 => "MetaLeft",
        56 => "ShiftLeft",
        58 => "AltLeft",
        59 => "ControlLeft",
        60 => "ShiftRight",
        61 => "AltRight",
        62 => "ControlRight",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        99 => "F3",
        100 => "F8",
        101 => "F9",
        103 => "F11",
        109 => "F10",
        111 => "F12",
        115 => "Home",
        116 => "PageUp",
        117 => "Delete",
        118 => "F4",
        119 => "End",
        120 => "F2",
        121 => "PageDown",
        122 => "F1",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        _ => return None,
    })
}

/// 修饰键（走 flagsChanged）现在按着没有。`flags` 是 `modifierFlags` 的原始值：
/// 有区分左右的位（IOKit 的 `NX_DEVICE*KEYMASK`）时看那一位；没有时（远程桌面、合成事件）看这一族的 Shift / Control / Option / Command 位。
pub fn modifier_down(code: &str, flags: usize) -> bool {
    const DEVICE_BITS: usize = 0x207f;
    let Some(mask) = modifier_mask(code) else { return false };
    if flags & DEVICE_BITS != 0 {
        return flags & mask != 0;
    }
    let coarse = match code {
        "ShiftLeft" | "ShiftRight" => 1 << 17,
        "ControlLeft" | "ControlRight" => 1 << 18,
        "AltLeft" | "AltRight" => 1 << 19,
        _ => 1 << 20,
    };
    flags & coarse != 0
}

/// 游戏没用 `captureKeys` 指定时吞掉的键：字母、数字、方向键、空格、回车。
/// Tab、Esc、Home / End 等留给 NSH（焦点切换、返回、滚动）。
pub fn default_capture(code: &str) -> bool {
    code.starts_with("Key") || code.starts_with("Digit") || code.starts_with("Arrow") || matches!(code, "Space" | "Enter")
}

/// 修饰键在 `modifierFlags` 里区分左右的那一位。
pub fn modifier_mask(code: &str) -> Option<usize> {
    Some(match code {
        "ControlLeft" => 0x1,
        "ShiftLeft" => 0x2,
        "ShiftRight" => 0x4,
        "MetaLeft" => 0x8,
        "MetaRight" => 0x10,
        "AltLeft" => 0x20,
        "AltRight" => 0x40,
        "ControlRight" => 0x2000,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Down,
    Up,
    /// flagsChanged（修饰键）。
    Flags,
}

/// 从一个按键事件里取出来的、做决定要用的事实。
#[derive(Debug, Clone, Copy)]
pub struct Facts {
    pub code: &'static str,
    pub kind: Kind,
    /// 按着 ⌘。
    pub cmd: bool,
    pub repeat: bool,
    /// `Flags` 时：这个修饰键现在按着（看 `modifierFlags` 里它那一位）。
    pub modifier_down: bool,
    /// 游戏视图可以收：事件是它所在的 key window 的、视图看得见、没在输入框里打字。
    pub eligible: bool,
    /// 游戏要这个键：转发的按下不再往下传（NSH 不处理时 AppKit 会「咚」一声）。
    pub capture: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    /// 转发给游戏：(down, repeat)。
    pub forward: Option<(bool, bool)>,
    /// 不再往下传。
    pub consume: bool,
}

impl Decision {
    pub const PASS: Self = Self { forward: None, consume: false };
}

/// 一个按键事件该怎么处理。`held` 是转发过按下、还没松开的键。
/// 松开只要按下时转发过就转发；按下只在 `eligible` 且没按着 ⌘ 时转发，`capture` 的不再往下传。
pub fn decide(held: &mut std::collections::BTreeSet<&'static str>, f: Facts) -> Decision {
    let pressed = match f.kind {
        Kind::Up => false,
        Kind::Flags => f.modifier_down,
        Kind::Down => true,
    };
    if !pressed {
        let forward = held.remove(f.code).then_some((false, false));
        return Decision { forward, consume: false };
    }
    if !f.eligible || (f.kind == Kind::Down && f.cmd) {
        return Decision::PASS;
    }
    let fresh = held.insert(f.code);
    match f.kind {
        // 修饰键别的位变了（另一个修饰键）时这一位还是按着，不重复转发
        Kind::Flags => Decision { forward: fresh.then_some((true, false)), consume: false },
        _ => Decision { forward: Some((true, f.repeat && !fresh)), consume: f.capture },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashSet};

    use super::*;

    #[test]
    fn common_game_keys() {
        assert_eq!([0, 13, 1, 2].map(code), [Some("KeyA"), Some("KeyW"), Some("KeyS"), Some("KeyD")]);
        assert_eq!([123, 124, 125, 126].map(code), [Some("ArrowLeft"), Some("ArrowRight"), Some("ArrowDown"), Some("ArrowUp")]);
        assert_eq!([49, 36, 53, 56, 60].map(code), [Some("Space"), Some("Enter"), Some("Escape"), Some("ShiftLeft"), Some("ShiftRight")]);
        assert_eq!([82, 10, 200].map(code), [None, None, None], "小键盘、ISO 多出来的键、不存在的键码");
    }

    #[test]
    fn every_letter_digit_and_f_key_once() {
        let names: Vec<_> = (0..=u16::MAX).filter_map(code).collect();
        let unique: HashSet<_> = names.iter().collect();
        assert_eq!(names.len(), unique.len(), "没有两个键码对到同一个名字");
        for c in 'A'..='Z' {
            assert!(unique.contains(&format!("Key{c}").as_str()), "Key{c}");
        }
        for d in 0..=9 {
            assert!(unique.contains(&format!("Digit{d}").as_str()), "Digit{d}");
        }
        for f in 1..=12 {
            assert!(unique.contains(&format!("F{f}").as_str()), "F{f}");
        }
    }

    #[test]
    fn modifiers() {
        assert!(modifier_mask("ShiftLeft").is_some() && modifier_mask("MetaRight").is_some());
        assert!(modifier_mask("KeyA").is_none() && modifier_mask("Space").is_none());
        assert_eq!(code(57), None, "CapsLock 是锁定状态，不当成按着的键");
    }

    fn facts(code: &'static str, kind: Kind) -> Facts {
        Facts { code, kind, cmd: false, repeat: false, modifier_down: false, eligible: true, capture: default_capture(code) }
    }

    #[test]
    fn default_capture_covers_game_keys_but_not_navigation() {
        for c in ["KeyA", "Digit1", "ArrowLeft", "ArrowUp", "Space", "Enter"] {
            assert!(default_capture(c), "{c}");
        }
        for c in ["Tab", "Escape", "Home", "PageDown", "Backspace", "F1"] {
            assert!(!default_capture(c), "{c}");
        }
    }

    #[test]
    fn modifier_down_falls_back_to_the_coarse_bits() {
        const SHIFT: usize = 1 << 17;
        assert!(modifier_down("ShiftLeft", SHIFT | 0x2));
        assert!(!modifier_down("ShiftLeft", SHIFT | 0x4), "按着的是右 Shift");
        assert!(modifier_down("ShiftLeft", SHIFT) && modifier_down("ShiftRight", SHIFT), "没有区分左右的位时只看 Shift 位");
        assert!(!modifier_down("ShiftLeft", 0));
        assert!(modifier_down("MetaRight", 1 << 20) && modifier_down("AltLeft", 1 << 19) && modifier_down("ControlRight", 1 << 18));
        assert!(!modifier_down("KeyA", SHIFT));
    }

    #[test]
    fn down_up_and_what_gets_swallowed() {
        let mut held = BTreeSet::new();
        assert_eq!(decide(&mut held, facts("KeyA", Kind::Down)), Decision { forward: Some((true, false)), consume: true });
        assert_eq!(decide(&mut held, Facts { repeat: true, ..facts("KeyA", Kind::Down) }), Decision { forward: Some((true, true)), consume: true });
        assert_eq!(decide(&mut held, facts("KeyA", Kind::Up)), Decision { forward: Some((false, false)), consume: false });
        assert_eq!(decide(&mut held, facts("KeyA", Kind::Up)), Decision::PASS, "没按下过的松开不转发");
        let tab = decide(&mut held, facts("Tab", Kind::Down));
        assert_eq!(tab, Decision { forward: Some((true, false)), consume: false }, "不在吞键名单里的照常给 NSH");
        let arrow = decide(&mut held, Facts { capture: false, ..facts("ArrowLeft", Kind::Down) });
        assert_eq!(arrow.consume, false, "游戏没要的键不吞");
        assert!(held.contains("ArrowLeft"));
    }

    #[test]
    fn not_eligible_or_command_is_passed_through_but_release_still_forwarded() {
        let mut held = BTreeSet::new();
        assert_eq!(decide(&mut held, Facts { eligible: false, ..facts("KeyW", Kind::Down) }), Decision::PASS);
        assert_eq!(decide(&mut held, Facts { cmd: true, ..facts("KeyQ", Kind::Down) }), Decision::PASS);
        decide(&mut held, facts("KeyW", Kind::Down));
        let up = decide(&mut held, Facts { eligible: false, ..facts("KeyW", Kind::Up) });
        assert_eq!(up.forward, Some((false, false)), "按下时转发过，松开不管焦点都转发");
    }

    #[test]
    fn repeat_without_a_forwarded_press_counts_as_a_press() {
        // ⌘A 按下（不转发），松开 ⌘ 后 A 的自动重复不带 ⌘
        let mut held = BTreeSet::new();
        let d = decide(&mut held, Facts { repeat: true, ..facts("KeyA", Kind::Down) });
        assert_eq!(d.forward, Some((true, false)));
    }

    #[test]
    fn modifiers_follow_the_flag_bits_not_a_toggle() {
        let mut held = BTreeSet::new();
        let shift = |down, eligible| Facts { modifier_down: down, eligible, ..facts("ShiftLeft", Kind::Flags) };
        // 窗口还不是 key window 时按下的 Shift：不转发；激活后松开也不会被当成按下
        assert_eq!(decide(&mut held, shift(true, false)), Decision::PASS);
        assert_eq!(decide(&mut held, shift(false, true)), Decision::PASS);
        assert_eq!(decide(&mut held, shift(true, true)).forward, Some((true, false)));
        assert_eq!(decide(&mut held, shift(true, true)), Decision::PASS, "另一个修饰键变了，Shift 还按着");
        assert_eq!(decide(&mut held, shift(false, true)).forward, Some((false, false)));
        assert!(held.is_empty());
    }
}
