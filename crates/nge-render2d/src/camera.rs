//! 2D 相机和变换。

use bevy_ecs::prelude::Component;
use glam::camera::rh::proj::directx;
use glam::{Mat4, Vec2};

/// zoom 下限，避免 0 或负数导致除零。
const MIN_ZOOM: f32 = 1e-4;

/// 正交相机。`zoom` 2 表示放大两倍；没有相机实体时按默认值渲染。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Camera2d {
    pub position: Vec2,
    pub zoom: f32,
    pub rotation: f32,
}

impl Default for Camera2d {
    fn default() -> Self {
        Self { position: Vec2::ZERO, zoom: 1.0, rotation: 0.0 }
    }
}

impl Camera2d {
    /// 世界坐标 → 裁剪空间。`size` 是视口大小（point）。
    pub fn view_proj(&self, size: Vec2) -> Mat4 {
        let half = size.max(Vec2::ONE) / (2.0 * self.zoom.max(MIN_ZOOM));
        let proj = directx::orthographic(-half.x, half.x, -half.y, half.y, -1.0, 1.0);
        let view = Mat4::from_rotation_z(-self.rotation) * Mat4::from_translation(-self.position.extend(0.0));
        proj * view
    }

    /// 视图内坐标（左上角原点，y 朝下）→ 世界坐标。
    pub fn screen_to_world(&self, size: Vec2, screen: Vec2) -> Vec2 {
        let local = (screen - size / 2.0) * Vec2::new(1.0, -1.0) / self.zoom.max(MIN_ZOOM);
        Vec2::from_angle(self.rotation).rotate(local) + self.position
    }

    /// 世界坐标 → 视图内坐标（左上角原点，y 朝下），[`Self::screen_to_world`] 的逆。
    pub fn world_to_screen(&self, size: Vec2, world: Vec2) -> Vec2 {
        let local = Vec2::from_angle(-self.rotation).rotate(world - self.position) * self.zoom.max(MIN_ZOOM);
        local * Vec2::new(1.0, -1.0) + size / 2.0
    }
}

/// 精灵的位置、旋转（弧度，逆时针）、缩放和绘制顺序（z 大的在上面）。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Transform2d {
    pub translation: Vec2,
    pub rotation: f32,
    pub scale: Vec2,
    pub z: f32,
}

impl Default for Transform2d {
    fn default() -> Self {
        Self { translation: Vec2::ZERO, rotation: 0.0, scale: Vec2::ONE, z: 0.0 }
    }
}

/// 画在哪：物理每帧给有刚体的实体写的插值位置。有它时精灵和相机跟随用它的平移和旋转（缩放、z 照样取 [`Transform2d`]）。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DrawAt {
    pub translation: Vec2,
    pub rotation: f32,
}

impl Transform2d {
    pub fn from_xy(x: f32, y: f32) -> Self {
        Self { translation: Vec2::new(x, y), ..Default::default() }
    }

    pub fn with_z(self, z: f32) -> Self {
        Self { z, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn project(cam: &Camera2d, size: Vec2, world: Vec2) -> Vec2 {
        let clip = cam.view_proj(size) * world.extend(0.0).extend(1.0);
        let ndc = Vec3::new(clip.x, clip.y, clip.z) / clip.w;
        // ndc → 视图坐标
        Vec2::new((ndc.x + 1.0) / 2.0 * size.x, (1.0 - ndc.y) / 2.0 * size.y)
    }

    fn assert_close(a: Vec2, b: Vec2) {
        assert!((a - b).length() < 1e-3, "{a} != {b}");
    }

    #[test]
    fn default_camera_centers_origin_with_y_up() {
        let cam = Camera2d::default();
        let size = Vec2::new(400.0, 300.0);
        assert_close(project(&cam, size, Vec2::ZERO), Vec2::new(200.0, 150.0));
        assert_close(project(&cam, size, Vec2::new(100.0, 50.0)), Vec2::new(300.0, 100.0));
        assert_close(cam.screen_to_world(size, Vec2::new(0.0, 0.0)), Vec2::new(-200.0, 150.0));
    }

    #[test]
    fn screen_to_world_inverts_projection_with_pan_zoom_rotation() {
        let cam = Camera2d { position: Vec2::new(30.0, -20.0), zoom: 2.5, rotation: 0.7 };
        let size = Vec2::new(390.0, 844.0);
        for world in [Vec2::ZERO, Vec2::new(12.0, 34.0), Vec2::new(-50.0, 8.0)] {
            assert_close(cam.screen_to_world(size, project(&cam, size, world)), world);
        }
    }

    #[test]
    fn world_to_screen_matches_the_projection() {
        let cam = Camera2d { position: Vec2::new(30.0, -20.0), zoom: 2.5, rotation: 0.7 };
        let size = Vec2::new(390.0, 844.0);
        for world in [Vec2::ZERO, Vec2::new(12.0, 34.0), Vec2::new(-50.0, 8.0)] {
            assert_close(cam.world_to_screen(size, world), project(&cam, size, world));
        }
    }

    #[test]
    fn zoom_scales_distances() {
        let cam = Camera2d { zoom: 2.0, ..Default::default() };
        let size = Vec2::new(400.0, 300.0);
        assert_close(project(&cam, size, Vec2::new(10.0, 0.0)), Vec2::new(220.0, 150.0));
    }
}
