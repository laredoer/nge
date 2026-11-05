//! `camera3d.match2d`：3D 相机每帧跟着 2D 相机走，3D 的一个单位就是 2D 的一个 point（见 docs/camera25d.md）。

use bevy_ecs::prelude::*;
use glam::{Quat, Vec3};
use nge_ecs::Screen;
use nge_render2d::Camera2d;
use nge_render3d::{Camera3d, Transform3d};

/// 场景相机上有它时，3D 相机每帧对齐 2D 相机；`z` 是相机离 z = 0 平面的距离。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub(crate) struct Match2d {
    pub z: f32,
}

/// 跑两次：`PreUpdate` 里场景同步之后（同一帧到达的点击用新相机），`PostUpdate` 里 2D 相机跟随、震屏之后（顺序是硬性的）。
/// 3D 相机放在 2D 相机正前方，正交投影，竖直方向看到的范围和 2D 相机一样。
pub(crate) fn match_2d(screen: Res<Screen>, mut cameras: Query<(&Match2d, &Camera2d, &mut Camera3d, &mut Transform3d)>) {
    for (matched, cam2d, mut cam3d, mut transform) in &mut cameras {
        let z = if matched.z.is_finite() && matched.z > 0.0 { matched.z } else { 1000.0 };
        // 正交投影的近平面可以是 0：z 在 [-z, z] 之间的模型都看得到
        *cam3d = Camera3d { ortho: Some(screen.size.y.max(1.0) / cam2d.zoom.max(1e-4)), near: 0.0, far: 2.0 * z, ..Camera3d::default() };
        *transform = Transform3d { translation: cam2d.position.extend(z), rotation: Quat::from_rotation_z(cam2d.rotation), scale: Vec3::ONE };
    }
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Vec2, Vec3};
    use nge_render2d::Camera2d;
    use nge_render3d::{Camera3d, Transform3d};
    use serde_json::json;

    use nge_core::Viewport;

    use crate::testing::app;

    /// 宽高不一样，宽和高用反了才看得出来。
    const WIDE: Viewport = Viewport { size: [200.0, 100.0], pixels: [200, 100] };

    fn frame(app: &mut nge_ecs::App, scene: Option<serde_json::Value>) {
        if let Some(scene) = scene {
            app.prop("scene", Some(&scene));
        }
        app.tick(WIDE, 0.5);
    }

    fn cameras(app: &mut nge_ecs::App) -> (Camera2d, Camera3d, Transform3d) {
        let world = app.world_mut();
        world.query::<(&Camera2d, &Camera3d, &Transform3d)>().single(world).map(|(a, b, c)| (*a, *b, *c)).unwrap()
    }

    fn project(m: Mat4, p: Vec3) -> Vec2 {
        let c = m * p.extend(1.0);
        c.truncate().truncate() / c.w
    }

    /// z = 0 平面上的点，3D 相机和 2D 相机投到屏幕上的同一个位置。
    fn assert_aligned(app: &mut nge_ecs::App) {
        let (cam2d, cam3d, eye) = cameras(app);
        let size = Vec2::from(WIDE.size);
        let (m2, m3) = (cam2d.view_proj(size), cam3d.view_proj(&eye, size.x / size.y));
        for p in [Vec2::new(0.0, 0.0), Vec2::new(130.0, 40.0), Vec2::new(-20.0, 95.0)] {
            let (a, b) = (project(m2, p.extend(0.0)), project(m3, p.extend(0.0)));
            assert!((a - b).length() < 1e-4, "{p}: 2D {a}，3D {b}");
        }
    }

    #[test]
    fn ortho_is_declared() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera3d": { "z": 10, "ortho": 600 } })));
        assert_eq!(cameras(&mut app).1.ortho, Some(600.0));
        frame(&mut app, Some(json!({ "camera3d": { "z": 10 } })));
        assert_eq!(cameras(&mut app).1.ortho, None, "去掉就回到透视");
    }

    #[test]
    fn match2d_follows_the_2d_camera_every_frame() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera": { "x": 100, "y": 50, "zoom": 2 }, "camera3d": { "match2d": true, "z": 400, "x": 9, "fov": 30 } })));
        let (_, cam3d, eye) = cameras(&mut app);
        assert_eq!(cam3d.ortho, Some(WIDE.size[1] / 2.0), "竖直方向看到视口高度 / zoom");
        assert_eq!(eye.translation, Vec3::new(100.0, 50.0, 400.0), "x、y 跟 2D 相机，z 用声明的");
        assert_eq!(cam3d.far, 800.0, "远平面在 2 × z");
        assert_aligned(&mut app);

        // Rust 改了 2D 相机（转角、位置），下一帧 3D 相机跟上
        let world = app.world_mut();
        let mut cam2d = world.query::<&mut Camera2d>().single_mut(world).unwrap();
        cam2d.rotation = 0.3;
        cam2d.position = Vec2::new(-40.0, 10.0);
        frame(&mut app, None);
        assert_aligned(&mut app);
    }

    #[test]
    fn match2d_follows_the_shake() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera": { "x": 10, "y": 20 }, "camera3d": { "match2d": true } })));
        app.command("shake", &json!({ "strength": 8, "duration": 1 }));
        frame(&mut app, None);
        let (cam2d, _, eye) = cameras(&mut app);
        assert_ne!(cam2d.position, Vec2::new(10.0, 20.0), "在抖");
        assert_eq!(eye.translation, cam2d.position.extend(1000.0), "z 默认 1000，抖动也跟上");
        assert_aligned(&mut app);
    }

    #[test]
    fn turning_match2d_off_restores_the_declared_camera() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera": { "x": 100 }, "camera3d": { "match2d": true } })));
        frame(&mut app, Some(json!({ "camera": { "x": 100 }, "camera3d": { "x": 0, "y": 0, "z": 10 } })));
        let (_, cam3d, eye) = cameras(&mut app);
        assert_eq!((cam3d.ortho, eye.translation), (None, Vec3::new(0.0, 0.0, 10.0)));
        assert_eq!((cam3d.near, cam3d.far), (Camera3d::default().near, Camera3d::default().far), "近远平面回到默认");
        frame(&mut app, None);
        assert_eq!(cameras(&mut app).2.translation, Vec3::new(0.0, 0.0, 10.0), "关掉以后不再跟着 2D 相机");
    }

    #[test]
    fn redeclaring_keeps_near_and_far_set_by_rust() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera3d": { "z": 10, "fov": 40 } })));
        let world = app.world_mut();
        let mut cam = world.query::<&mut Camera3d>().single_mut(world).unwrap();
        (cam.near, cam.far) = (0.5, 50.0);
        frame(&mut app, Some(json!({ "camera3d": { "z": 10, "fov": 50 } })));
        let cam = cameras(&mut app).1;
        assert!((cam.fov_y - 50f32.to_radians()).abs() < 1e-6);
        assert_eq!((cam.near, cam.far), (0.5, 50.0));
    }

    #[test]
    fn whole_depth_range_is_visible() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera3d": { "match2d": true, "z": 100 } })));
        let (_, cam3d, eye) = cameras(&mut app);
        let m = cam3d.view_proj(&eye, 2.0);
        for z in [-99.95, 0.0, 99.95] {
            let c = m * Vec3::new(0.0, 0.0, z).extend(1.0);
            let depth = c.z / c.w;
            assert!((0.0..=1.0).contains(&depth), "z = {z} 在可见范围里: {depth}");
        }
    }
}
