//! 统一的 Pointer 输入：NSH 的 touch command 转成按 id 跟踪的指针，带本帧边沿状态。

use bevy_ecs::prelude::Resource;
use glam::Vec2;

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

#[derive(Resource, Debug, Default)]
pub struct Input {
    pointers: Vec<Pointer>,
}

impl Input {
    /// 收到一次触摸变化。`phase` 是 NSH 的 start / move / end / cancel。
    pub fn touch(&mut self, phase: &str, id: u32, screen: Vec2) {
        let existing = self.pointers.iter_mut().find(|p| p.id == id && p.down);
        match (phase, existing) {
            ("start", stale) => {
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
            ("move", Some(p)) => p.screen = screen,
            ("end" | "cancel", Some(p)) => {
                p.screen = screen;
                p.down = false;
                p.just_released = true;
            }
            (phase, None) => log::debug!("忽略没有按下记录的触摸 {phase} #{id}"),
            (phase, Some(_)) => log::debug!("未知触摸阶段 {phase}"),
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_move_release_across_frames() {
        let mut input = Input::default();
        input.touch("start", 1, Vec2::new(10.0, 20.0));
        assert_eq!(input.pointers_pressed().count(), 1);
        input.end_frame();

        input.touch("move", 1, Vec2::new(15.0, 25.0));
        assert_eq!(input.pointers_pressed().count(), 0);
        let p = input.pointers_down().next().unwrap();
        assert_eq!((p.screen, p.start), (Vec2::new(15.0, 25.0), Vec2::new(10.0, 20.0)));
        input.end_frame();

        input.touch("end", 1, Vec2::new(16.0, 26.0));
        assert_eq!(input.pointers_released().count(), 1);
        assert_eq!(input.pointers_down().count(), 0);
        input.end_frame();
        assert_eq!(input.pointers().count(), 0);
    }

    #[test]
    fn tap_within_one_frame_is_pressed_and_released() {
        let mut input = Input::default();
        input.touch("start", 7, Vec2::ZERO);
        input.touch("end", 7, Vec2::ZERO);
        let p = input.pointers().next().unwrap();
        assert!(p.just_pressed && p.just_released && !p.down);
        input.end_frame();
        assert_eq!(input.pointers().count(), 0);
    }

    #[test]
    fn same_id_reused_after_release_is_a_new_pointer() {
        let mut input = Input::default();
        input.touch("start", 1, Vec2::ZERO);
        input.touch("end", 1, Vec2::ZERO);
        input.touch("start", 1, Vec2::ONE);
        assert_eq!(input.pointers().count(), 2);
        assert_eq!(input.pointers_down().count(), 1);
    }

    #[test]
    fn multitouch_tracked_separately_and_stray_events_ignored() {
        let mut input = Input::default();
        input.touch("move", 9, Vec2::ONE);
        input.touch("start", 1, Vec2::ZERO);
        input.touch("start", 2, Vec2::ONE);
        input.touch("cancel", 1, Vec2::ZERO);
        assert_eq!(input.pointers_down().map(|p| p.id).collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn repeated_start_without_end_does_not_leave_ghost() {
        let mut input = Input::default();
        input.touch("start", 1, Vec2::ZERO);
        input.end_frame();
        input.touch("start", 1, Vec2::ONE);
        input.touch("end", 1, Vec2::ONE);
        input.end_frame();
        assert_eq!(input.pointers().count(), 0);
    }

    #[test]
    fn map_world_applies_to_all() {
        let mut input = Input::default();
        input.touch("start", 1, Vec2::new(2.0, 3.0));
        input.map_world(|s| s * 2.0);
        assert_eq!(input.pointers().next().unwrap().world, Vec2::new(4.0, 6.0));
    }
}
