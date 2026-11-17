//! GameController 的读数：手柄（macOS、iOS）和 iOS 的硬件键盘，每帧在主线程读一遍（见 docs/gamepad.md、docs/ios-input.md）；
//! 手柄的震动走 `GCController.haptics` 给的 `CHHapticEngine`（见 docs/input-actions.md）。

#[cfg(target_os = "ios")]
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::ffi::CStr;

use nge_core::bridge::RumbleOut;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol};
use objc2::{msg_send, sel};
use objc2_foundation::{NSArray, NSError, NSString};
#[cfg(target_os = "ios")]
use objc2_game_controller::GCKeyboard;
use objc2_game_controller::{
    GCController, GCControllerButtonInput, GCControllerDirectionPad, GCDeviceHaptics, GCExtendedGamepad, GCHapticsLocalityDefault,
    GCHapticsLocalityLeftHandle, GCHapticsLocalityRightHandle,
};

use crate::{pad, rumble};

// CoreHaptics 的常量（事件种类、参数名）；`objc2-game-controller` 在 macOS 上不带 CoreHaptics 的绑定，这里直接发消息
#[link(name = "CoreHaptics", kind = "framework")]
unsafe extern "C" {
    static CHHapticEventTypeHapticContinuous: &'static NSString;
    static CHHapticEventParameterIDHapticIntensity: &'static NSString;
}

/// 手柄上的马达：低频（左手柄）、高频（右手柄），或者不分左右的手柄上的那一个。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Motor {
    Low,
    High,
    Whole,
}

/// 连着的手柄：给每个编号，记着它们（震动时找得到），缓存震动引擎。
#[derive(Default)]
pub struct Pads {
    ids: pad::Ids<usize>,
    live: Vec<(u32, Retained<GCController>)>,
    engines: HashMap<(u32, Motor), Retained<AnyObject>>,
}

impl Pads {
    /// 所有连着的、有 extendedGamepad 的手柄这一帧的读数，带着手柄号。
    pub fn read(&mut self) -> Vec<(u32, pad::Reading)> {
        let found: Vec<_> = unsafe { GCController::controllers() }.iter().filter_map(|c| unsafe { c.extendedGamepad() }.map(|g| (c, g))).collect();
        let keys: Vec<usize> = found.iter().map(|(c, _)| Retained::as_ptr(c) as usize).collect();
        let ids = self.ids.assign(&keys);
        // 拔掉的手柄：它的震动引擎不要了
        self.engines.retain(|(id, _), _| ids.contains(id));
        let readings = ids.iter().zip(&found).map(|(id, (_, g))| (*id, read(g))).collect();
        self.live = ids.into_iter().zip(found.into_iter().map(|(c, _)| c)).collect();
        readings
    }

    /// 让手柄震一下（指定的那个，或者全部）。不能震的手柄、出了错的只记日志。
    pub fn rumble(&mut self, rumble: &RumbleOut) {
        let ids: Vec<u32> = self.live.iter().map(|(id, _)| *id).collect();
        for id in rumble::targets(rumble.pad, &ids) {
            let Some(haptics) = self.live.iter().find(|(i, _)| *i == id).and_then(|(_, c)| haptics(c)) else { continue };
            for (motor, strength) in motors(&haptics, rumble) {
                if strength <= 0.0 {
                    continue;
                }
                if let Err(e) = self.play(id, motor, &haptics, strength, rumble.duration) {
                    log::warn!("手柄 {id} 震不了: {e}");
                    // 引擎可能被系统停掉了：丢掉，下次重建
                    self.engines.remove(&(id, motor));
                }
            }
        }
    }

    fn play(&mut self, id: u32, motor: Motor, haptics: &GCDeviceHaptics, strength: f32, seconds: f32) -> Result<(), String> {
        let engine = match self.engines.get(&(id, motor)) {
            Some(engine) => engine.clone(),
            None => {
                let locality = unsafe {
                    match motor {
                        Motor::Low => GCHapticsLocalityLeftHandle,
                        Motor::High => GCHapticsLocalityRightHandle,
                        Motor::Whole => GCHapticsLocalityDefault,
                    }
                };
                let engine: Option<Retained<AnyObject>> = unsafe { msg_send![haptics, createEngineWithLocality: locality] };
                let engine = engine.ok_or("建不了震动引擎")?;
                self.engines.insert((id, motor), engine.clone());
                engine
            }
        };
        let pattern = pattern(strength, seconds)?;
        // 已经在跑时再 start 一次没有副作用
        let started: Result<(), Retained<NSError>> = unsafe { msg_send![&*engine, startAndReturnError: _] };
        started.map_err(describe)?;
        let player: Result<Retained<AnyObject>, Retained<NSError>> = unsafe { msg_send![&*engine, createPlayerWithPattern: &*pattern, error: _] };
        let player = player.map_err(describe)?;
        let played: Result<(), Retained<NSError>> = unsafe { msg_send![&*player, startAtTime: 0.0f64, error: _] };
        played.map_err(describe)
    }
}

/// 这个手柄的震动（没有马达的、系统太老的是 `None`）。
fn haptics(controller: &GCController) -> Option<Retained<GCDeviceHaptics>> {
    if !controller.respondsToSelector(sel!(haptics)) {
        return None;
    }
    unsafe { controller.haptics() }
}

/// 每个马达放多强：手柄分左右时左边是低频、右边是高频；不分时取大的。
fn motors(haptics: &GCDeviceHaptics, rumble: &RumbleOut) -> Vec<(Motor, f32)> {
    let supported = unsafe { haptics.supportedLocalities() };
    let split = unsafe { supported.containsObject(GCHapticsLocalityLeftHandle) && supported.containsObject(GCHapticsLocalityRightHandle) };
    if split {
        vec![(Motor::Low, rumble::unit(rumble.low)), (Motor::High, rumble::unit(rumble.high))]
    } else {
        vec![(Motor::Whole, rumble::single(rumble.low, rumble.high))]
    }
}

fn describe(error: Retained<NSError>) -> String {
    error.localizedDescription().to_string()
}

fn class(name: &CStr) -> Result<&'static AnyClass, String> {
    AnyClass::get(name).ok_or_else(|| format!("没有 {}", name.to_string_lossy()))
}

/// 一段持续 `seconds` 秒、强度 `strength`（0 … 1）的震动（`CHHapticPattern`）。
fn pattern(strength: f32, seconds: f32) -> Result<Retained<AnyObject>, String> {
    unsafe {
        let parameter: Allocated<AnyObject> = msg_send![class(c"CHHapticEventParameter")?, alloc];
        let parameter: Retained<AnyObject> = msg_send![parameter, initWithParameterID: CHHapticEventParameterIDHapticIntensity, value: rumble::unit(strength)];
        let parameters = NSArray::from_retained_slice(&[parameter]);
        let event: Allocated<AnyObject> = msg_send![class(c"CHHapticEvent")?, alloc];
        let event: Retained<AnyObject> = msg_send![
            event,
            initWithEventType: CHHapticEventTypeHapticContinuous,
            parameters: &*parameters,
            relativeTime: 0.0f64,
            duration: f64::from(seconds),
        ];
        let events = NSArray::from_retained_slice(&[event]);
        let none = NSArray::<AnyObject>::new();
        let pattern: Allocated<AnyObject> = msg_send![class(c"CHHapticPattern")?, alloc];
        let pattern: Result<Retained<AnyObject>, Retained<NSError>> = msg_send![pattern, initWithEvents: &*events, parameters: &*none, error: _];
        pattern.map_err(describe)
    }
}

fn read(g: &GCExtendedGamepad) -> pad::Reading {
    let pressed = |b: Retained<GCControllerButtonInput>| unsafe { b.isPressed() };
    let maybe = |b: Option<Retained<GCControllerButtonInput>>| b.is_some_and(pressed);
    let axes = |d: Retained<GCControllerDirectionPad>| unsafe { [d.xAxis().value(), d.yAxis().value()] };
    unsafe {
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
    }
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

    /// CoreHaptics 的类、选择子、常量都对得上：建得出一段震动（不用连手柄）。
    #[test]
    fn a_haptic_pattern_can_be_built() {
        let pattern = super::pattern(0.5, 0.25).expect("建得出 CHHapticPattern");
        let duration: f64 = unsafe { objc2::msg_send![&*pattern, duration] };
        assert!((duration - 0.25).abs() < 1e-6, "{duration}");
        assert!(super::pattern(7.0, 0.1).is_ok(), "强度先限制在 0 … 1");
    }

    #[test]
    fn reading_with_no_pads_connected_is_empty_or_numbered_from_zero() {
        let mut pads = super::Pads::default();
        let ids: Vec<u32> = pads.read().into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, (0..ids.len() as u32).collect::<Vec<_>>());
        // 没有这个号的手柄：什么都不做
        pads.rumble(&nge_core::bridge::RumbleOut { pad: Some(99), low: 1.0, high: 1.0, duration: 0.1 });
    }
}
