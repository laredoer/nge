//! 统一的 Pointer 输入：NSH 的 touch command 转成按 id 跟踪的指针，带本帧边沿状态；
//! 平台层的 key command 记成按着的键（键名是 Web `KeyboardEvent.code`，如 `KeyA`、`ArrowLeft`；手柄按钮是 `GamepadA` 这类）；
//! gamepad command 记两根摇杆和两个扳机的模拟量（几个手柄合起来的）；pad command 记每个手柄各自的状态（见 docs/input-actions.md）。

use std::collections::{BTreeMap, BTreeSet};

use bevy_ecs::prelude::Resource;
use glam::Vec2;
use nge_core::bridge::TouchPhase;

/// 一根手指 / 一个鼠标指针。`screen` 是视图内的 point 坐标（左上角原点，y 朝下）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pointer {
    pub id: u32,
    pub screen: Vec2,
    /// 经主相机反投影后的世界坐标；没有相机时等于 `screen`。
    pub world: Vec2,
    /// 按下时的位置（screen）。
    pub start: Vec2,
    pub down: bool,
    pub just_pressed: bool,
    pub just_released: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stick {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    L2,
    R2,
}

/// 一个手柄：按着的键（`GamepadA`、`GamepadLeftStickLeft`…）、摇杆、扳机。
#[derive(Debug, Default)]
pub struct Pad {
    keys_down: BTreeSet<String>,
    keys_pressed: BTreeSet<String>,
    keys_released: BTreeSet<String>,
    sticks: [Vec2; 2],
    triggers: [f32; 2],
}

impl Pad {
    pub fn key_down(&self, code: &str) -> bool {
        self.keys_down.contains(code)
    }

    pub fn key_pressed(&self, code: &str) -> bool {
        self.keys_pressed.contains(code)
    }

    pub fn key_released(&self, code: &str) -> bool {
        self.keys_released.contains(code)
    }

    /// x 向右、y 向上，各在 −1 … 1，已经去掉死区。
    pub fn stick(&self, stick: Stick) -> Vec2 {
        self.sticks[stick as usize]
    }

    /// 0（没按）… 1（按到底）。
    pub fn trigger(&self, trigger: Trigger) -> f32 {
        self.triggers[trigger as usize]
    }
}

fn clean_stick(v: [f32; 2]) -> Vec2 {
    Vec2::from(v.map(|c| if c.is_finite() { c.clamp(-1.0, 1.0) } else { 0.0 }))
}

fn clean_trigger(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

#[derive(Resource, Debug, Default)]
pub struct Input {
    pointers: Vec<Pointer>,
    keys_down: BTreeSet<String>,
    keys_pressed: BTreeSet<String>,
    keys_released: BTreeSet<String>,
    sticks: [Vec2; 2],
    triggers: [f32; 2],
    pads: BTreeMap<u32, Pad>,
    pads_connected: Vec<u32>,
    pads_disconnected: Vec<u32>,
}

impl Input {
    /// 收到一次触摸变化。
    pub fn touch(&mut self, phase: TouchPhase, id: u32, screen: Vec2) {
        let existing = self.pointers.iter_mut().find(|p| p.id == id && p.down);
        match (phase, existing) {
            (TouchPhase::Start, stale) => {
                // 丢了 end / cancel 时同一 id 又按下：旧的当作已松开
                if let Some(old) = stale {
                    old.down = false;
                    old.just_released = true;
                }
                self.pointers.push(Pointer {
                id,
                screen,
                world: screen,
                start: screen,
                down: true,
                just_pressed: true,
                just_released: false,
                })
            }
            (TouchPhase::Move, Some(p)) => p.screen = screen,
            (TouchPhase::End | TouchPhase::Cancel, Some(p)) => {
                p.screen = screen;
                p.down = false;
                p.just_released = true;
            }
            (phase, None) => log::debug!("忽略没有按下记录的触摸 {phase:?} #{id}"),
        }
    }

    /// 本帧所有指针，包括本帧刚松开的。
    pub fn pointers(&self) -> impl Iterator<Item = &Pointer> {
        self.pointers.iter()
    }

    pub fn pointers_pressed(&self) -> impl Iterator<Item = &Pointer> {
        self.pointers.iter().filter(|p| p.just_pressed)
    }

    pub fn pointers_released(&self) -> impl Iterator<Item = &Pointer> {
        self.pointers.iter().filter(|p| p.just_released)
    }

    pub fn pointers_down(&self) -> impl Iterator<Item = &Pointer> {
        self.pointers.iter().filter(|p| p.down)
    }

    /// 收到一次按键变化。自动重复（`repeat`）不改状态；没按过的松开不算。
    pub fn key(&mut self, code: &str, down: bool, repeat: bool) {
        if down && !repeat && self.keys_down.insert(code.to_owned()) {
            self.keys_pressed.insert(code.to_owned());
        } else if !down && self.keys_down.remove(code) {
            self.keys_released.insert(code.to_owned());
        }
    }

    /// 现在按着。
    pub fn key_down(&self, code: &str) -> bool {
        self.keys_down.contains(code)
    }

    /// 这一帧刚按下。
    pub fn key_pressed(&self, code: &str) -> bool {
        self.keys_pressed.contains(code)
    }

    /// 这一帧刚松开（同一帧里按下又松开时 `key_pressed` 也是 true）。
    pub fn key_released(&self, code: &str) -> bool {
        self.keys_released.contains(code)
    }

    pub fn keys_down(&self) -> impl Iterator<Item = &str> {
        self.keys_down.iter().map(String::as_str)
    }

    /// 这一帧刚按下的所有键（「按任意键来绑定」用）。
    pub fn keys_pressed(&self) -> impl Iterator<Item = &str> {
        self.keys_pressed.iter().map(String::as_str)
    }

    /// `negative` 里有键按着时 -1，`positive` 里有键按着时 1，都按或都没按是 0。
    /// 只看现在按着的：同一帧里按下又松开的短按要用 `key_pressed` 兜底。
    pub fn axis(&self, negative: &[&str], positive: &[&str]) -> f32 {
        let any = |codes: &[&str]| codes.iter().any(|c| self.key_down(c));
        f32::from(i8::from(any(positive)) - i8::from(any(negative)))
    }

    /// 收到手柄摇杆的新值：每个分量限制在 −1 … 1，不是有限数的当 0。
    pub fn sticks(&mut self, left: [f32; 2], right: [f32; 2]) {
        self.sticks = [clean_stick(left), clean_stick(right)];
    }

    /// 收到扳机的新值（几个手柄里按得最深的）：限制在 0 … 1，不是有限数的当 0。
    pub fn triggers(&mut self, l2: f32, r2: f32) {
        self.triggers = [clean_trigger(l2), clean_trigger(r2)];
    }

    /// 扳机的模拟量：0（没按）… 1（按到底）；`GamepadL2` / `GamepadR2` 这两个键是按过一半。
    pub fn trigger(&self, trigger: Trigger) -> f32 {
        self.triggers[trigger as usize]
    }

    /// 收到一个手柄的整份状态；第一次见到这个号就是接入。和上一份比出刚按下、刚松开。
    pub fn pad_state(&mut self, id: u32, keys: &[String], left: [f32; 2], right: [f32; 2], l2: f32, r2: f32) {
        if !self.pads.contains_key(&id) {
            self.pads_connected.push(id);
        }
        let pad = self.pads.entry(id).or_default();
        let now: BTreeSet<String> = keys.iter().cloned().collect();
        pad.keys_released.extend(pad.keys_down.difference(&now).cloned());
        pad.keys_pressed.extend(now.difference(&pad.keys_down).cloned());
        pad.keys_down = now;
        pad.sticks = [clean_stick(left), clean_stick(right)];
        pad.triggers = [clean_trigger(l2), clean_trigger(r2)];
    }

    /// 手柄拔掉了。
    pub fn pad_removed(&mut self, id: u32) {
        if self.pads.remove(&id).is_some() {
            self.pads_disconnected.push(id);
        }
    }

    /// 号是 `id` 的手柄；没连着是 `None`。
    pub fn pad(&self, id: u32) -> Option<&Pad> {
        self.pads.get(&id)
    }

    /// 连着的手柄号，从小到大。
    pub fn pads(&self) -> impl Iterator<Item = u32> {
        self.pads.keys().copied()
    }

    /// 这一帧接入的手柄。
    pub fn pads_connected(&self) -> &[u32] {
        &self.pads_connected
    }

    /// 这一帧拔出的手柄。
    pub fn pads_disconnected(&self) -> &[u32] {
        &self.pads_disconnected
    }

    /// 手柄摇杆：x 向右、y 向上，各在 −1 … 1；没连手柄时是 0。
    pub fn stick(&self, stick: Stick) -> Vec2 {
        self.sticks[stick as usize]
    }

    /// 相机插件用它把 screen 换算成 world。
    pub fn map_world(&mut self, f: impl Fn(Vec2) -> Vec2) {
        for p in &mut self.pointers {
            p.world = f(p.screen);
        }
    }

    /// 帧末：去掉已松开的指针，清掉边沿状态。
    pub(crate) fn end_frame(&mut self) {
        self.pointers.retain(|p| p.down);
        for p in &mut self.pointers {
            p.just_pressed = false;
        }
        self.keys_pressed.clear();
        self.keys_released.clear();
        for pad in self.pads.values_mut() {
            pad.keys_pressed.clear();
            pad.keys_released.clear();
        }
        self.pads_connected.clear();
        self.pads_disconnected.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TouchPhase::*;

    #[test]
    fn press_move_release_across_frames() {
        let mut input = Input::default();
        input.touch(Start, 1, Vec2::new(10.0, 20.0));
        assert_eq!(input.pointers_pressed().count(), 1);
        input.end_frame();

        input.touch(Move, 1, Vec2::new(15.0, 25.0));
        assert_eq!(input.pointers_pressed().count(), 0);
        let p = input.pointers_down().next().unwrap();
        assert_eq!((p.screen, p.start), (Vec2::new(15.0, 25.0), Vec2::new(10.0, 20.0)));
        input.end_frame();

        input.touch(End, 1, Vec2::new(16.0, 26.0));
        assert_eq!(input.pointers_released().count(), 1);
        assert_eq!(input.pointers_down().count(), 0);
        input.end_frame();
        assert_eq!(input.pointers().count(), 0);
    }

    #[test]
    fn tap_within_one_frame_is_pressed_and_released() {
        let mut input = Input::default();
        input.touch(Start, 7, Vec2::ZERO);
        input.touch(End, 7, Vec2::ZERO);
        let p = input.pointers().next().unwrap();
        assert!(p.just_pressed && p.just_released && !p.down);
        input.end_frame();
        assert_eq!(input.pointers().count(), 0);
    }

    #[test]
    fn same_id_reused_after_release_is_a_new_pointer() {
        let mut input = Input::default();
        input.touch(Start, 1, Vec2::ZERO);
        input.touch(End, 1, Vec2::ZERO);
        input.touch(Start, 1, Vec2::ONE);
        assert_eq!(input.pointers().count(), 2);
        assert_eq!(input.pointers_down().count(), 1);
    }

    #[test]
    fn multitouch_tracked_separately_and_stray_events_ignored() {
        let mut input = Input::default();
        input.touch(Move, 9, Vec2::ONE);
        input.touch(Start, 1, Vec2::ZERO);
        input.touch(Start, 2, Vec2::ONE);
        input.touch(Cancel, 1, Vec2::ZERO);
        assert_eq!(input.pointers_down().map(|p| p.id).collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn repeated_start_without_end_does_not_leave_ghost() {
        let mut input = Input::default();
        input.touch(Start, 1, Vec2::ZERO);
        input.end_frame();
        input.touch(Start, 1, Vec2::ONE);
        input.touch(End, 1, Vec2::ONE);
        input.end_frame();
        assert_eq!(input.pointers().count(), 0);
    }

    #[test]
    fn keys_down_pressed_released() {
        let mut input = Input::default();
        input.key("KeyA", true, false);
        assert!(input.key_down("KeyA") && input.key_pressed("KeyA") && !input.key_released("KeyA"));
        input.end_frame();
        input.key("KeyA", true, true);
        assert!(input.key_down("KeyA") && !input.key_pressed("KeyA"), "自动重复不算新按下");
        input.end_frame();
        input.key("KeyA", false, false);
        assert!(!input.key_down("KeyA") && input.key_released("KeyA"));
        input.end_frame();
        assert!(!input.key_released("KeyA"));

        input.key("Space", true, false);
        input.key("Space", false, false);
        assert!(input.key_pressed("Space") && input.key_released("Space") && !input.key_down("Space"), "同一帧按下又松开");
        input.key("KeyQ", false, false);
        assert!(!input.key_released("KeyQ"), "没按过的松开不算");
    }

    #[test]
    fn axis_and_keys_down() {
        let mut input = Input::default();
        let (left, right) = (["ArrowLeft", "KeyA"], ["ArrowRight", "KeyD"]);
        assert_eq!(input.axis(&left, &right), 0.0);
        input.key("KeyA", true, false);
        assert_eq!(input.axis(&left, &right), -1.0);
        input.key("ArrowRight", true, false);
        assert_eq!(input.axis(&left, &right), 0.0, "两边都按着");
        input.key("KeyA", false, false);
        assert_eq!(input.axis(&left, &right), 1.0);
        assert_eq!(input.keys_down().collect::<Vec<_>>(), ["ArrowRight"]);
    }

    #[test]
    fn map_world_applies_to_all() {
        let mut input = Input::default();
        input.touch(Start, 1, Vec2::new(2.0, 3.0));
        input.map_world(|s| s * 2.0);
        assert_eq!(input.pointers().next().unwrap().world, Vec2::new(4.0, 6.0));
    }

    #[test]
    fn sticks_are_clamped_and_bad_values_are_zero() {
        let mut input = Input::default();
        input.sticks([2.0, f32::NAN], [-0.25, f32::NEG_INFINITY]);
        assert_eq!(input.stick(Stick::Left), Vec2::new(1.0, 0.0));
        assert_eq!(input.stick(Stick::Right), Vec2::new(-0.25, 0.0));
        input.end_frame();
        assert_eq!(input.stick(Stick::Right), Vec2::new(-0.25, 0.0), "帧末不清");
    }

    #[test]
    fn each_pad_has_its_own_keys_sticks_and_triggers() {
        let mut input = Input::default();
        let keys = |list: &[&str]| list.iter().map(|k| (*k).to_owned()).collect::<Vec<_>>();
        input.pad_state(3, &keys(&["GamepadA"]), [2.0, f32::NAN], [0.0, -0.5], 0.25, 7.0);
        assert_eq!(input.pads().collect::<Vec<_>>(), [3]);
        assert_eq!(input.pads_connected(), [3]);
        let pad = input.pad(3).unwrap();
        assert!(pad.key_down("GamepadA") && pad.key_pressed("GamepadA") && !pad.key_released("GamepadA"));
        assert_eq!((pad.stick(Stick::Left), pad.stick(Stick::Right)), (Vec2::new(1.0, 0.0), Vec2::new(0.0, -0.5)), "限幅，坏的当 0");
        assert_eq!((pad.trigger(Trigger::L2), pad.trigger(Trigger::R2)), (0.25, 1.0));
        assert!(!input.key_down("GamepadA"), "合并的那条路是平台层另发的");
        assert!(input.pad(4).is_none());
        input.end_frame();
        assert!(input.pads_connected().is_empty(), "接入只有一帧");
        assert!(input.pad(3).unwrap().key_down("GamepadA") && !input.pad(3).unwrap().key_pressed("GamepadA"));

        input.pad_state(3, &keys(&["GamepadB"]), [0.0; 2], [0.0; 2], 0.0, 0.0);
        let pad = input.pad(3).unwrap();
        assert!(pad.key_released("GamepadA") && pad.key_pressed("GamepadB") && !pad.key_down("GamepadA"));
        assert!(input.pads_connected().is_empty(), "已经连着的不算新接入");
        input.end_frame();

        input.pad_removed(3);
        input.pad_removed(8);
        assert!(input.pad(3).is_none());
        assert_eq!(input.pads_disconnected(), [3], "没连过的不算拔出");
        input.end_frame();
        assert!(input.pads_disconnected().is_empty());
    }

    #[test]
    fn a_tap_between_two_pad_states_keeps_both_edges() {
        let mut input = Input::default();
        input.pad_state(1, &["GamepadA".to_owned()], [0.0; 2], [0.0; 2], 0.0, 0.0);
        input.pad_state(1, &[], [0.0; 2], [0.0; 2], 0.0, 0.0);
        let pad = input.pad(1).unwrap();
        assert!(pad.key_pressed("GamepadA") && pad.key_released("GamepadA") && !pad.key_down("GamepadA"));
    }

    #[test]
    fn merged_triggers_are_clamped() {
        let mut input = Input::default();
        input.triggers(0.5, -3.0);
        assert_eq!((input.trigger(Trigger::L2), input.trigger(Trigger::R2)), (0.5, 0.0));
        input.triggers(f32::NAN, 2.0);
        assert_eq!((input.trigger(Trigger::L2), input.trigger(Trigger::R2)), (0.0, 1.0));
    }

    #[test]
    fn keys_pressed_this_frame_are_listed() {
        let mut input = Input::default();
        input.key("KeyB", true, false);
        input.key("KeyA", true, false);
        assert_eq!(input.keys_pressed().collect::<Vec<_>>(), ["KeyA", "KeyB"]);
        input.end_frame();
        assert_eq!(input.keys_pressed().count(), 0);
    }
}
