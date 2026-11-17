//! GameController 的读数：手柄（macOS、iOS）和 iOS 的硬件键盘，每帧在主线程读一遍（见 docs/gamepad.md、docs/ios-input.md）。

#[cfg(target_os = "ios")]
use std::collections::BTreeSet;

use objc2::rc::Retained;
#[cfg(target_os = "ios")]
use objc2_game_controller::GCKeyboard;
use objc2_game_controller::{GCController, GCControllerButtonInput, GCControllerDirectionPad, GCExtendedGamepad};

use crate::pad;

/// 所有连着的、有 extendedGamepad 的手柄这一帧的读数。
pub fn read_pads() -> Vec<pad::Reading> {
    let pressed = |b: Retained<GCControllerButtonInput>| unsafe { b.isPressed() };
    let maybe = |b: Option<Retained<GCControllerButtonInput>>| b.is_some_and(pressed);
    let axes = |d: Retained<GCControllerDirectionPad>| unsafe { [d.xAxis().value(), d.yAxis().value()] };
    let read = |g: &GCExtendedGamepad| unsafe {
        pad::Reading {
            a: pressed(g.buttonA()),
            b: pressed(g.buttonB()),
            x: pressed(g.buttonX()),
            y: pressed(g.buttonY()),
            l1: pressed(g.leftShoulder()),
            r1: pressed(g.rightShoulder()),
            l3: maybe(g.leftThumbstickButton()),
            r3: maybe(g.rightThumbstickButton()),
            menu: pressed(g.buttonMenu()),
            options: maybe(g.buttonOptions()),
            l2: g.leftTrigger().value(),
            r2: g.rightTrigger().value(),
            dpad: axes(g.dpad()),
            left: axes(g.leftThumbstick()),
            right: axes(g.rightThumbstick()),
        }
    };
    unsafe { GCController::controllers() }.iter().filter_map(|c| unsafe { c.extendedGamepad() }).map(|g| read(&g)).collect()
}

/// 硬件键盘这一帧按着的键（所有键盘合起来）；没连键盘时是空的。
#[cfg(target_os = "ios")]
pub fn read_keyboard(out: &mut BTreeSet<&'static str>) {
    let Some(input) = (unsafe { GCKeyboard::coalescedKeyboard() }).and_then(|k| unsafe { k.keyboardInput() }) else { return };
    if !unsafe { input.isAnyKeyPressed() } {
        return;
    }
    for &(usage, code) in crate::hid::USAGES {
        if unsafe { input.buttonForKeyCode(usage) }.is_some_and(|b| unsafe { b.isPressed() }) {
            out.insert(code);
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use objc2_game_controller::*;

    use crate::hid::code;

    /// `GCKeyCode` 就是 HID 用法码：拿 GameController 自己的常量对一遍。
    #[test]
    fn hid_usages_match_the_game_controller_constants() {
        let pairs = unsafe {
            [
                (GCKeyCodeKeyA, "KeyA"),
                (GCKeyCodeKeyZ, "KeyZ"),
                (GCKeyCodeOne, "Digit1"),
                (GCKeyCodeZero, "Digit0"),
                (GCKeyCodeReturnOrEnter, "Enter"),
                (GCKeyCodeEscape, "Escape"),
                (GCKeyCodeSpacebar, "Space"),
                (GCKeyCodeGraveAccentAndTilde, "Backquote"),
                (GCKeyCodeF12, "F12"),
                (GCKeyCodeDeleteForward, "Delete"),
                (GCKeyCodeRightArrow, "ArrowRight"),
                (GCKeyCodeUpArrow, "ArrowUp"),
                (GCKeyCodeLeftGUI, "MetaLeft"),
                (GCKeyCodeRightAlt, "AltRight"),
            ]
        };
        for (usage, name) in pairs {
            assert_eq!(code(usage), Some(name), "{usage}");
        }
    }
}
