//! 3D 变换和相机（透视或正交）。世界坐标右手系，y 朝上。

use bevy_ecs::prelude::Component;
use glam::camera::rh::proj::directx;
use glam::{Mat4, Quat, Vec3, Vec4};

#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Transform3d {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform3d {
    fn default() -> Self {
        Self { translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE }
    }
}

impl Transform3d {
    pub fn from_xyz(x: f32, y: f32, z: f32) -> Self {
        Self { translation: Vec3::new(x, y, z), ..Default::default() }
    }

    /// 转向 `target`：让自身的 -Z 指向它。
    /// 目标和自身重合，或视线和 `up` 平行时不转。
    pub fn looking_at(self, target: Vec3, up: Vec3) -> Self {
        let forward = (target - self.translation).normalize_or_zero();
        let right = forward.cross(up).normalize_or_zero();
        if right == Vec3::ZERO {
            return self;
        }
        let rotation = Quat::from_mat3(&glam::Mat3::from_cols(right, right.cross(forward), -forward));
        Self { rotation, ..self }
    }

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// 相机，看向所在 [`Transform3d`] 的 -Z。没有相机实体时按默认值放在 (0, 0, 5) 看向原点。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Camera3d {
    /// 垂直视角（弧度）。
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
    /// `Some(h)` 是正交投影，竖直方向看到 `h` 个世界单位（横向按宽高比），`fov_y` 不看；`h` 不是有限的正数时按透视。
    pub ortho: Option<f32>,
}

impl Default for Camera3d {
    fn default() -> Self {
        Self { fov_y: 60f32.to_radians(), near: 0.1, far: 1000.0, ortho: None }
    }
}

impl Camera3d {
    /// 世界坐标 → 裁剪空间。相机的缩放不参与。
    pub fn view_proj(&self, transform: &Transform3d, aspect: f32) -> Mat4 {
        let view = Mat4::from_rotation_translation(transform.rotation, transform.translation).inverse();
        let aspect = aspect.max(1e-4);
        let proj = match self.ortho_height() {
            Some(h) => directx::orthographic(-h * aspect / 2.0, h * aspect / 2.0, -h / 2.0, h / 2.0, self.near, self.far),
            None => directx::perspective(self.fov_y, aspect, self.near, self.far),
        };
        proj * view
    }

    /// 光照算高光用的视点：透视时是相机位置（w = 1），正交时所有片元的视线都一样，是朝向相机的方向（w = 0）。
    pub fn eye(&self, transform: &Transform3d) -> Vec4 {
        match self.ortho_height() {
            Some(_) => (transform.rotation * Vec3::Z).extend(0.0),
            None => transform.translation.extend(1.0),
        }
    }

    fn ortho_height(&self) -> Option<f32> {
        self.ortho.filter(|h| h.is_finite() && *h > 0.0)
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use super::*;

    fn ndc(m: Mat4, p: Vec3) -> Vec3 {
        let c = m * p.extend(1.0);
        c.truncate() / c.w
    }

    #[test]
    fn matrix_applies_scale_then_rotation_then_translation() {
        let t = Transform3d { translation: Vec3::new(1.0, 0.0, 0.0), rotation: Quat::from_rotation_z(FRAC_PI_2), scale: Vec3::splat(2.0) };
        assert!((t.matrix().transform_point3(Vec3::X) - Vec3::new(1.0, 2.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn looking_at_points_minus_z_at_target() {
        let t = Transform3d::from_xyz(0.0, 2.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y);
        let forward = t.rotation * Vec3::NEG_Z;
        assert!((forward - Vec3::new(0.0, -2.0, -5.0).normalize()).length() < 1e-5);
        assert_eq!(t.translation, Vec3::new(0.0, 2.0, 5.0));
    }

    #[test]
    fn target_projects_to_center_and_depth_in_zero_one() {
        let cam = Camera3d::default();
        let t = Transform3d::from_xyz(3.0, 4.0, 5.0).looking_at(Vec3::new(1.0, 1.0, 1.0), Vec3::Y);
        let p = ndc(cam.view_proj(&t, 1.5), Vec3::new(1.0, 1.0, 1.0));
        assert!(p.x.abs() < 1e-4 && p.y.abs() < 1e-4, "{p}");
        assert!(p.z > 0.0 && p.z < 1.0, "{p}");
    }

    #[test]
    fn y_up_and_x_right_on_screen() {
        let cam = Camera3d::default();
        let t = Transform3d::from_xyz(0.0, 0.0, 5.0);
        let m = cam.view_proj(&t, 1.0);
        assert!(ndc(m, Vec3::new(0.0, 1.0, 0.0)).y > 0.0);
        assert!(ndc(m, Vec3::new(1.0, 0.0, 0.0)).x > 0.0);
        // 近的 z 更小
        assert!(ndc(m, Vec3::new(0.0, 0.0, 1.0)).z < ndc(m, Vec3::new(0.0, 0.0, -1.0)).z);
        // 相机背后的点 w < 0（会被裁掉）
        assert!((m * Vec4::new(0.0, 0.0, 10.0, 1.0)).w < 0.0);
    }

    #[test]
    fn camera_scale_is_ignored() {
        let cam = Camera3d::default();
        let t = Transform3d::from_xyz(0.0, 0.0, 5.0);
        let scaled = Transform3d { scale: Vec3::splat(3.0), ..t };
        assert!(cam.view_proj(&t, 1.0).abs_diff_eq(cam.view_proj(&scaled, 1.0), 1e-6));
    }

    #[test]
    fn ortho_shows_the_same_size_at_any_distance() {
        let cam = Camera3d { ortho: Some(600.0), ..Camera3d::default() };
        let t = Transform3d::from_xyz(100.0, 50.0, 500.0);
        let m = cam.view_proj(&t, 2.0);
        for z in [-200.0, 0.0, 300.0] {
            let top = ndc(m, Vec3::new(100.0, 50.0 + 300.0, z));
            let right = ndc(m, Vec3::new(100.0 + 600.0, 50.0, z));
            assert!((top.y - 1.0).abs() < 1e-4 && top.x.abs() < 1e-4, "竖直方向看到 600 个单位: {top}");
            assert!((right.x - 1.0).abs() < 1e-4, "横向按宽高比: {right}");
            assert!(top.z > 0.0 && top.z < 1.0, "在远近平面之间: {top}");
        }
        assert!(ndc(m, Vec3::new(0.0, 0.0, 100.0)).z < ndc(m, Vec3::new(0.0, 0.0, -100.0)).z, "近的 z 更小");
    }

    #[test]
    fn bad_ortho_falls_back_to_perspective() {
        let t = Transform3d::from_xyz(0.0, 0.0, 5.0);
        let perspective = Camera3d::default().view_proj(&t, 1.0);
        for h in [0.0, -3.0, f32::NAN, f32::INFINITY] {
            let cam = Camera3d { ortho: Some(h), ..Camera3d::default() };
            assert!(cam.view_proj(&t, 1.0).abs_diff_eq(perspective, 1e-6), "{h}");
        }
    }

    #[test]
    fn eye_is_a_point_in_perspective_and_a_direction_in_ortho() {
        let t = Transform3d::from_xyz(1.0, 2.0, 3.0).looking_at(Vec3::new(1.0, 2.0, 0.0), Vec3::Y);
        assert_eq!(Camera3d::default().eye(&t), Vec4::new(1.0, 2.0, 3.0, 1.0));
        let ortho = Camera3d { ortho: Some(10.0), ..Camera3d::default() }.eye(&t);
        assert!((ortho - Vec4::new(0.0, 0.0, 1.0, 0.0)).length() < 1e-5, "朝向相机的方向（相机的 +Z）: {ortho}");
        // 斜着往下看的等距相机：方向跟着相机转
        let iso = Transform3d::from_xyz(10.0, 10.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y);
        let dir = Camera3d { ortho: Some(10.0), ..Camera3d::default() }.eye(&iso);
        assert!((dir - Vec3::ONE.normalize().extend(0.0)).length() < 1e-5, "{dir}");
    }
}
