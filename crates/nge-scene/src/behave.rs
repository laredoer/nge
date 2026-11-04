//! 节点上的内置行为：自转、点击。

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_ecs::{Input, Outbox, Time};
use nge_render2d::{Assets, Atlases, Image, Sprite, Transform2d};

use crate::protocol::{TAP, Tap};
use crate::sync::SceneId;

/// 每秒转多少弧度。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Spin(pub f32);

/// 按下时发 `tap` 事件。
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tappable;

pub(crate) fn spin(time: Res<Time>, mut q: Query<(&Spin, &mut Transform2d)>) {
    for (spin, mut t) in &mut q {
        t.rotation = (t.rotation + spin.0 * time.delta) % std::f32::consts::TAU;
    }
}

pub(crate) fn tap(
    input: Res<Input>,
    images: Res<Assets<Image>>,
    atlases: Res<Atlases>,
    sprites: Query<(&SceneId, &Transform2d, &Sprite), With<Tappable>>,
    mut outbox: ResMut<Outbox>,
) {
    for p in input.pointers_pressed() {
        let hit = sprites
            .iter()
            .filter(|(_, t, s)| contains(t, atlases.sprite_size(&images, s), p.world))
            .max_by(|a, b| a.1.z.total_cmp(&b.1.z));
        if let Some((id, ..)) = hit {
            outbox.send(TAP, &Tap { id: id.0.clone(), x: p.world.x, y: p.world.y });
        }
    }
}

/// 世界坐标 `p` 是否落在精灵（考虑旋转和缩放）里面。
fn contains(t: &Transform2d, size: Vec2, p: Vec2) -> bool {
    if t.scale.x == 0.0 || t.scale.y == 0.0 {
        return false;
    }
    let local = Vec2::from_angle(-t.rotation).rotate(p - t.translation) / t.scale;
    local.abs().cmple(size / 2.0).all()
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use serde_json::json;

    use super::*;
    use crate::testing::{app, frame};

    #[test]
    fn hit_test_respects_rotation_and_scale() {
        let t = Transform2d { translation: Vec2::new(10.0, 0.0), rotation: FRAC_PI_2, scale: Vec2::new(2.0, 1.0), z: 0.0 };
        // 4×2 的精灵横向放大 2 倍再转 90°：世界里是竖着的 2 宽 8 高
        let size = Vec2::new(4.0, 2.0);
        assert!(contains(&t, size, Vec2::new(10.0, 3.9)));
        assert!(!contains(&t, size, Vec2::new(10.0, 4.1)));
        assert!(!contains(&t, size, Vec2::new(11.1, 0.0)));
        assert!(!contains(&Transform2d { scale: Vec2::ZERO, ..t }, size, t.translation));
    }

    #[test]
    fn spin_advances_rotation_in_degrees_per_second() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "spin": 90 }] })));
        frame(&mut app, None);
        let world = app.world_mut();
        let t = world.query::<&Transform2d>().iter(world).find(|t| t.rotation != 0.0).copied().unwrap();
        // 每帧 0.5s，两帧共 90°
        assert!((t.rotation - FRAC_PI_2).abs() < 1e-5, "{}", t.rotation);
    }

    #[test]
    fn tap_hits_topmost_tappable_sprite() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [
            { "id": "low", "w": 20, "tap": true },
            { "id": "high", "w": 20, "z": 1, "tap": true },
            { "id": "top_not_tappable", "w": 20, "z": 2 },
        ] })));
        // 视口 100×100：(50, 50) 是世界原点
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 50, "y": 50 }));
        let events = frame(&mut app, None);
        let taps: Vec<Tap> = events.iter().filter(|(n, _)| n == "tap").map(|(_, d)| TAP.decode(d).unwrap()).collect();
        assert_eq!(taps, [Tap { id: "high".into(), x: 0.0, y: 0.0 }]);

        app.command("touch", &json!({ "phase": "end", "id": 1, "x": 50, "y": 50 }));
        app.command("touch", &json!({ "phase": "start", "id": 2, "x": 0, "y": 0 }));
        assert!(frame(&mut app, None).iter().all(|(n, _)| n != "tap"), "点在空白处不发");
    }

    fn taps(events: &[(String, serde_json::Value)]) -> Vec<Tap> {
        events.iter().filter(|(n, _)| n == "tap").map(|(_, d)| TAP.decode(d).unwrap()).collect()
    }

    #[test]
    fn tap_maps_screen_y_down_to_world_y_up() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [
            { "id": "up", "w": 10, "y": 30, "tap": true },
            { "id": "down", "w": 10, "y": -30, "tap": true },
        ] })));
        // 屏幕 y=20 在中心上方 30 point
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 50, "y": 20 }));
        assert_eq!(taps(&frame(&mut app, None)), [Tap { id: "up".into(), x: 0.0, y: 30.0 }]);
    }

    #[test]
    fn tap_uses_camera_changed_in_same_frame() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "w": 10, "x": 110, "tap": true }] })));
        // 相机和点击同一帧到达：zoom 2 时屏幕右移 20 point = 世界右移 10
        app.prop("scene", Some(&json!({ "camera": { "x": 100, "zoom": 2 }, "sprites": [{ "id": "a", "w": 10, "x": 110, "tap": true }] })));
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 70, "y": 50 }));
        assert_eq!(taps(&frame(&mut app, None)), [Tap { id: "a".into(), x: 110.0, y: 0.0 }]);
    }

    #[test]
    fn tap_flag_can_be_removed() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "w": 20, "tap": true, "spin": 10 }] })));
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "w": 20 }] })));
        let world = app.world_mut();
        assert_eq!(world.query::<&Tappable>().iter(world).count(), 0);
        assert_eq!(world.query::<&Spin>().iter(world).count(), 0);
    }
}
