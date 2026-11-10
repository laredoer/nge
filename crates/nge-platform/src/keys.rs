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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

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
}
