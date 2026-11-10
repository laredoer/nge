//! 键盘的平台无关部分：哪些键转给游戏、哪些吞掉（macOS 和浏览器共用，见 docs/keyboard.md）。
//! 键名用 Web 的 `KeyboardEvent.code`。

use serde_json::Value as JsonValue;

/// 转给游戏的键（和 macOS 键码表认识的一样）；浏览器上别的键（小键盘、媒体键……）不转发。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const NAMES: &[&str] = &[
    "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK", "KeyL", "KeyM",
    "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT", "KeyU", "KeyV", "KeyW", "KeyX", "KeyY", "KeyZ",
    "Digit0", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8", "Digit9",
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
    "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Space", "Enter", "Tab", "Escape", "Backspace", "Delete",
    "Home", "End", "PageUp", "PageDown", "Minus", "Equal", "BracketLeft", "BracketRight", "Backslash",
    "Semicolon", "Quote", "Backquote", "Comma", "Period", "Slash",
    "ShiftLeft", "ShiftRight", "ControlLeft", "ControlRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight",
];

/// `KeyboardEvent.code` → 转给游戏的键名；不认识的返回 `None`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn name(code: &str) -> Option<&'static str> {
    NAMES.iter().copied().find(|n| *n == code)
}

/// 平台层自己用的 prop：游戏视图要吞的键（`code` 名的数组），不转给游戏。
pub const CAPTURE_KEYS: &str = "captureKeys";

/// 解析 `captureKeys`；不是字符串数组时记 warn，用默认的（`None`）。
pub fn capture_keys(value: Option<&JsonValue>) -> Option<Vec<String>> {
    let value = value.filter(|v| !v.is_null())?;
    let list = value.as_array().and_then(|a| a.iter().map(|v| v.as_str().map(str::to_owned)).collect::<Option<Vec<_>>>());
    if list.is_none() {
        log::warn!("captureKeys 要是键名的数组，收到 {value}，用默认的");
    }
    list
}

/// 这个键要不要吞：游戏用 `captureKeys` 指定过就看名单，没有就用 [`default_capture`]。
pub fn captured(list: Option<&[String]>, code: &str) -> bool {
    list.map_or_else(|| default_capture(code), |list| list.iter().any(|c| c == code))
}

/// 游戏没用 `captureKeys` 指定时吞掉的键：字母、数字、方向键、空格、回车。
/// Tab、Esc、Home / End 等留给 NSH（焦点切换、返回、滚动）。
pub fn default_capture(code: &str) -> bool {
    code.starts_with("Key") || code.starts_with("Digit") || code.starts_with("Arrow") || matches!(code, "Space" | "Enter")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Down,
    Up,
    /// flagsChanged（修饰键，macOS）；浏览器上修饰键也是 keydown / keyup。
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Flags,
}

/// 从一个按键事件里取出来的、做决定要用的事实。
#[derive(Debug, Clone, Copy)]
pub struct Facts {
    pub code: &'static str,
    pub kind: Kind,
    /// 按着 ⌘（浏览器上 ⌘ 或 Ctrl：留给浏览器和 NSH 的快捷键）。
    pub cmd: bool,
    pub repeat: bool,
    /// `Flags` 时：这个修饰键现在按着（macOS 看 `modifierFlags` 里它那一位）。
    pub modifier_down: bool,
    /// 游戏视图可以收：窗口在前台（macOS 是 key window）、视图看得见、没在输入框里打字。
    pub eligible: bool,
    /// 游戏要这个键：转发的按下不再往下传（macOS 上 NSH 不处理时 AppKit 会「咚」一声，浏览器上会滚页面）。
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

/// 修饰键：Shift、Control、Alt（Option）、Meta（⌘）。
fn is_modifier(code: &str) -> bool {
    ["Shift", "Control", "Alt", "Meta"].iter().any(|m| code.starts_with(m))
}

/// 松开 ⌘：浏览器（macOS）上按着 ⌘ 时别的键收不到 keyup，这时把按着的全部补发松开。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn releases_all(f: &Facts) -> bool {
    f.kind == Kind::Up && matches!(f.code, "MetaLeft" | "MetaRight")
}

/// 焦点在按钮、链接上时这些键是去按它的，不转给游戏。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn activates_control(code: &str) -> bool {
    matches!(code, "Enter" | "Space")
}

/// 一个按键事件该怎么处理。`held` 是转发过按下、还没松开的键。
/// 松开只要按下时转发过就转发；按下只在 `eligible` 且没按着 ⌘ 时转发（修饰键自己不看 ⌘），`capture` 的不再往下传。
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
    if !f.eligible || (f.kind == Kind::Down && f.cmd && !is_modifier(f.code)) {
        return Decision::PASS;
    }
    let fresh = held.insert(f.code);
    match f.kind {
        // 修饰键别的位变了（另一个修饰键）时这一位还是按着，不重复转发
        Kind::Flags => Decision { forward: fresh.then_some((true, false)), consume: false },
        // 浏览器上修饰键也是 keydown：转发但不吞（留给浏览器和 NSH 的快捷键）
        _ if is_modifier(f.code) => Decision { forward: Some((true, f.repeat && !fresh)), consume: false },
        _ => Decision { forward: Some((true, f.repeat && !fresh)), consume: f.capture },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::json;

    use super::*;

    #[test]
    fn web_names_are_the_ones_the_game_knows() {
        assert_eq!(name("KeyJ"), Some("KeyJ"));
        assert_eq!(name("ArrowLeft"), Some("ArrowLeft"));
        assert_eq!(name("Numpad1"), None, "小键盘不转发");
        assert_eq!(name(""), None);
        let mac: BTreeSet<_> = (0..=u16::MAX).filter_map(crate::keys::code).collect();
        let web: BTreeSet<_> = NAMES.iter().copied().collect();
        assert_eq!(mac, web, "浏览器和 macOS 转发同一组键");
    }

    #[test]
    fn capture_keys_prop() {
        assert_eq!(capture_keys(None), None);
        assert_eq!(capture_keys(Some(&JsonValue::Null)), None);
        assert_eq!(capture_keys(Some(&json!(["KeyJ", "Space"]))), Some(vec!["KeyJ".to_owned(), "Space".to_owned()]));
        assert_eq!(capture_keys(Some(&json!(["KeyJ", 1]))), None, "不是字符串数组就用默认的");
        assert_eq!(capture_keys(Some(&json!([]))), Some(vec![]), "空数组：什么都不吞");
    }

    #[test]
    fn captured_uses_the_list_or_the_default() {
        let list = vec!["KeyJ".to_owned()];
        assert!(captured(Some(&list), "KeyJ") && !captured(Some(&list), "KeyA"));
        assert!(captured(None, "KeyA") && !captured(None, "Tab"));
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

    #[test]
    fn releasing_command_releases_everything() {
        // macOS 的浏览器：按着 ⌘ 时别的键松开收不到 keyup
        assert!(releases_all(&facts("MetaLeft", Kind::Up)) && releases_all(&facts("MetaRight", Kind::Up)));
        assert!(!releases_all(&facts("MetaLeft", Kind::Down)));
        assert!(!releases_all(&facts("ShiftLeft", Kind::Up)) && !releases_all(&facts("KeyA", Kind::Up)));
    }

    #[test]
    fn modifier_keys_themselves_are_forwarded_even_with_command_down() {
        // 浏览器上按下 Ctrl 时 ctrlKey 已经是 true；修饰键自己的按下照样转发（macOS 上走 Flags，本来就不看 ⌘）
        let mut held = BTreeSet::new();
        let ctrl = decide(&mut held, Facts { cmd: true, ..facts("ControlLeft", Kind::Down) });
        assert_eq!(ctrl.forward, Some((true, false)));
        assert!(!ctrl.consume, "修饰键不吞");
        assert_eq!(decide(&mut held, Facts { cmd: true, ..facts("KeyC", Kind::Down) }), Decision::PASS, "Ctrl+C 留给浏览器");
        assert_eq!(decide(&mut held, Facts { cmd: true, ..facts("ControlLeft", Kind::Up) }).forward, Some((false, false)));
    }

    #[test]
    fn enter_and_space_on_a_focused_control_are_left_to_it() {
        assert!(activates_control("Enter") && activates_control("Space"));
        assert!(!activates_control("KeyJ") && !activates_control("ArrowLeft"));
    }
}
