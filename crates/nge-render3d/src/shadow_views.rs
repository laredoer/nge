//! 阴影图的视野：方向光的视图、级联分段和每段的矩阵、聚光灯和点光源六面的透视，以及包围盒和视野相交（见 docs/render3d-more.md）。

use glam::camera::rh::proj::directx;
use glam::{Mat3, Mat4, Vec3};

use crate::camera::{Camera3d, Transform3d};

pub(crate) const MAX_CASCADES: usize = 4;

/// 光的视图：看向 -z，原点在世界原点（正交投影只要方向）。方向是 0 时是 `None`。
pub(crate) fn light_view(direction: Vec3) -> Option<Mat4> {
    let forward = direction.try_normalize()?;
    let right = forward.cross(up_for(forward)).normalize();
    Some(Mat4::from_mat3(Mat3::from_cols(right, right.cross(forward), -forward).transpose()))
}

/// 和 `forward` 不平行的上方向。
fn up_for(forward: Vec3) -> Vec3 {
    if forward.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y }
}

/// 相机前方 `near` 到 `end` 切成 `count` 段，返回每段远端的距离；分段点是对数和均匀各一半。
pub(crate) fn splits(near: f32, end: f32, count: usize) -> [f32; MAX_CASCADES] {
    let count = count.clamp(1, MAX_CASCADES);
    // 对数的那一半要求起点 > 0
    let base = near.max(1e-3);
    let mut s = [end; MAX_CASCADES];
    for (i, split) in s.iter_mut().enumerate().take(count - 1) {
        let t = (i + 1) as f32 / count as f32;
        *split = 0.5 * (base * (end / base).powf(t)) + 0.5 * (near + (end - near) * t);
    }
    s
}

/// 相机视锥里，相机前方 `from` 到 `to` 这一段的 8 个角（相机自己的坐标系）。
pub(crate) fn slice_corners(camera: &Camera3d, aspect: f32, from: f32, to: f32) -> [Vec3; 8] {
    let aspect = aspect.max(1e-4);
    let half = |d: f32| match camera.ortho_height() {
        Some(h) => (h * aspect / 2.0, h / 2.0),
        None => {
            let t = (camera.fov_y / 2.0).tan();
            (t * d * aspect, t * d)
        }
    };
    std::array::from_fn(|i| {
        let d = if i & 4 == 0 { from } else { to };
        let (hx, hy) = half(d);
        Vec3::new(if i & 1 == 0 { -hx } else { hx }, if i & 2 == 0 { -hy } else { hy }, -d)
    })
}

/// 一段视锥的光矩阵：外接球直径加两格做边长、中心对齐像素格，深度扩到所有投影物。
/// `corners` 是 [`slice_corners`] 的结果（相机坐标系），`eye` 是相机。返回（矩阵, 一个像素在世界里多大）。
pub(crate) fn cascade_matrix(
    direction: Vec3,
    eye: &Transform3d,
    corners: &[Vec3; 8],
    casters: impl IntoIterator<Item = (Vec3, Vec3)>,
    size: u32,
) -> Option<(Mat4, f32)> {
    let view = light_view(direction)?;
    // 外接球在相机坐标系里算：和相机的位置、朝向都无关，相机转动时边长不变
    let center = corners.iter().sum::<Vec3>() / 8.0;
    let radius = corners.iter().map(|c| c.distance(center)).fold(0.0, f32::max);
    if !(radius > 0.0 && radius.is_finite()) {
        return None;
    }
    let camera = Mat4::from_rotation_translation(eye.rotation, eye.translation);
    let center = view.transform_point3(camera.transform_point3(center));
    // 中心对齐像素格时往负方向最多挪一格，正方形每边多留一格：边长 = 直径 + 2 格 = `size` 格
    let texel = 2.0 * radius / (size.max(3) - 2) as f32;
    let half = radius + texel;
    // 中心对齐像素格：相机平移时阴影图只按整格挪，影子边缘不闪
    let snapped = (center.truncate() / texel).floor() * texel;
    // 看向 -z：近的 z 大。朝光的一侧扩到所有投影物，段外但挡在光路上的也投影子
    let far = center.z - radius;
    let mut near = center.z + radius;
    for (lo, hi) in casters {
        for i in 0..8 {
            let corner = Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
            near = near.max(view.transform_point3(corner).z);
        }
    }
    let proj = directx::orthographic(snapped.x - half, snapped.x + half, snapped.y - half, snapped.y + half, -near, -far);
    Some((proj * view, texel))
}

/// 聚光灯的透视：张角是外锥的两倍，近平面是 `range` 的 1%。
pub(crate) fn spot_matrix(position: Vec3, direction: Vec3, outer: f32, range: f32) -> Option<Mat4> {
    perspective(position, direction, (2.0 * outer).clamp(1e-3, 178f32.to_radians()), range)
}

/// 从 `position` 看向 `direction`、张角 `fov` 的方形透视，近平面是 `range` 的 1%。
fn perspective(position: Vec3, direction: Vec3, fov: f32, range: f32) -> Option<Mat4> {
    let forward = direction.try_normalize()?;
    if !(range > 0.0 && range.is_finite() && position.is_finite()) {
        return None;
    }
    let eye = Transform3d { translation: position, ..Default::default() }.looking_at(position + forward, up_for(forward));
    let view = Mat4::from_rotation_translation(eye.rotation, eye.translation).inverse();
    Some(directx::perspective(fov, 1.0, range * 0.01, range) * view)
}

/// 点光源六面 90° 的透视，顺序 +X、-X、+Y、-Y、+Z、-Z。
pub(crate) fn point_matrices(position: Vec3, range: f32) -> Option<[Mat4; 6]> {
    let axes = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];
    let faces = axes.map(|axis| perspective(position, axis, std::f32::consts::FRAC_PI_2, range));
    faces.iter().all(Option::is_some).then(|| faces.map(Option::unwrap))
}

/// 从灯指向片元的方向落在哪一面：绝对值最大的轴（和着色器的 `lit_point` 一致，测试用）。
#[cfg(test)]
fn point_face(v: Vec3) -> usize {
    let a = v.abs();
    if a.x >= a.y && a.x >= a.z {
        usize::from(v.x < 0.0)
    } else if a.y >= a.z {
        2 + usize::from(v.y < 0.0)
    } else {
        4 + usize::from(v.z < 0.0)
    }
}

/// 世界包围盒和这个视野相交（保守：8 个角全在同一个裁剪面外才算不相交）。
pub(crate) fn intersects(view_proj: Mat4, (lo, hi): (Vec3, Vec3)) -> bool {
    // 空盒（反的）、NaN 都算不相交
    if !lo.cmple(hi).all() {
        return false;
    }
    let clip: [glam::Vec4; 8] = std::array::from_fn(|i| {
        let corner = Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
        view_proj * corner.extend(1.0)
    });
    // 裁剪空间里 6 个面都是线性的，背后（w < 0）的角也照样判断
    let planes: [fn(glam::Vec4) -> bool; 6] = [|c| c.x < -c.w, |c| c.x > c.w, |c| c.y < -c.w, |c| c.y > c.w, |c| c.z < 0.0, |c| c.z > c.w];
    !planes.iter().any(|outside| clip.iter().all(|c| outside(*c)))
}

#[cfg(test)]
mod tests {
    use glam::Quat;

    use super::*;

    fn ndc(m: Mat4, p: Vec3) -> Option<Vec3> {
        let c = m * p.extend(1.0);
        (c.w > 0.0).then(|| c.truncate() / c.w)
    }

    fn inside(m: Mat4, p: Vec3) -> bool {
        ndc(m, p).is_some_and(|q| q.x.abs() <= 1.0 + 1e-4 && q.y.abs() <= 1.0 + 1e-4 && (-1e-4..=1.0 + 1e-4).contains(&q.z))
    }

    #[test]
    fn splits_grow_and_end_at_the_distance() {
        for count in 1..=MAX_CASCADES {
            let s = splits(0.1, 50.0, count);
            assert!(s[0] > 0.1, "{s:?}");
            assert!(s[..count].windows(2).all(|w| w[0] < w[1]), "单调 {s:?}");
            assert_eq!(s[count - 1], 50.0, "最后一段到 end {s:?}");
        }
        // 对数 0.1 × 500^½ ≈ 2.236 和均匀 25.05 各一半
        assert!((splits(0.1, 50.0, 2)[0] - 13.643).abs() < 1e-2, "{:?}", splits(0.1, 50.0, 2));
        // 远处一段更长
        let s = splits(0.1, 50.0, 4);
        assert!(s[1] - s[0] > s[0] - 0.1 && s[3] - s[2] > s[2] - s[1], "{s:?}");
    }

    fn camera() -> (Camera3d, Transform3d) {
        (Camera3d::default(), Transform3d::from_xyz(3.0, 5.0, 8.0).looking_at(Vec3::new(-2.0, 0.0, -4.0), Vec3::Y))
    }

    #[test]
    fn slice_corners_lie_on_the_frustum() {
        let (cam, _) = camera();
        let c = slice_corners(&cam, 2.0, 1.0, 10.0);
        let half = (cam.fov_y / 2.0).tan();
        for p in c {
            assert!((p.z + 1.0).abs() < 1e-5 || (p.z + 10.0).abs() < 1e-5, "相机看向 -z {p}");
            assert!((p.y.abs() - half * -p.z).abs() < 1e-4 && (p.x.abs() - 2.0 * half * -p.z).abs() < 1e-4, "{p}");
        }
        let ortho = Camera3d { ortho: Some(6.0), ..Camera3d::default() };
        for p in slice_corners(&ortho, 2.0, 1.0, 10.0) {
            assert!((p.y.abs() - 3.0).abs() < 1e-5 && (p.x.abs() - 6.0).abs() < 1e-5, "正交的截面不随距离变 {p}");
        }
    }

    #[test]
    fn each_cascade_frames_its_slice_and_the_casters_toward_the_light() {
        let (cam, eye) = camera();
        let s = splits(cam.near, 40.0, 3);
        let caster = (Vec3::new(-1.0, 30.0, -1.0), Vec3::new(1.0, 31.0, 1.0));
        let mut from = cam.near;
        for to in &s[..3] {
            let corners = slice_corners(&cam, 1.5, from, *to);
            let (m, texel) = cascade_matrix(Vec3::new(1.0, -2.0, 0.5), &eye, &corners, [caster], 1024).unwrap();
            for p in corners {
                let world = eye.matrix().transform_point3(p);
                assert!(inside(m, world), "{from}–{to}: {world} → {:?}", ndc(m, world));
            }
            // 光那边的高处挡板：深度不被切掉
            let z = ndc(m, Vec3::new(0.0, 30.5, 0.0)).unwrap().z;
            assert!((0.0..=1.0).contains(&z), "{z}");
            let diag = (corners[0] - corners[7]).length();
            assert!(texel * 1024.0 >= diag * 0.999, "边长至少是这段的对角线 {texel}");
            from = *to;
        }
    }

    #[test]
    fn cascade_size_does_not_change_when_the_camera_turns() {
        let (cam, eye) = camera();
        let corners = slice_corners(&cam, 1.5, 2.0, 12.0);
        let (ma, a) = cascade_matrix(Vec3::NEG_Y, &eye, &corners, [], 1024).unwrap();
        let turned = Transform3d { rotation: Quat::from_rotation_y(0.7) * eye.rotation, ..eye };
        let (mb, b) = cascade_matrix(Vec3::NEG_Y, &turned, &corners, [], 1024).unwrap();
        assert_eq!(a, b);
        assert!(ma.x_axis.abs_diff_eq(mb.x_axis, 1e-7) && ma.y_axis.abs_diff_eq(mb.y_axis, 1e-7));
    }

    #[test]
    fn moving_the_camera_a_little_moves_the_cascade_by_whole_texels() {
        let (cam, eye) = camera();
        let corners = slice_corners(&cam, 1.5, 2.0, 12.0);
        let size = 1024;
        let direction = Vec3::new(1.0, -2.0, 0.5);
        let (a, _) = cascade_matrix(direction, &eye, &corners, [], size).unwrap();
        let mut moved_any = false;
        for step in 1..20 {
            let moved = Transform3d { translation: eye.translation + Vec3::new(0.013, 0.007, -0.011) * step as f32, ..eye };
            let (b, _) = cascade_matrix(direction, &moved, &corners, [], size).unwrap();
            assert!(a.x_axis.abs_diff_eq(b.x_axis, 1e-6) && a.y_axis.abs_diff_eq(b.y_axis, 1e-6), "尺寸不变");
            for d in [b.w_axis.x - a.w_axis.x, b.w_axis.y - a.w_axis.y] {
                let texels = d * size as f32 / 2.0;
                assert!((texels - texels.round()).abs() < 1e-2, "第 {step} 步平移了 {texels} 格");
                moved_any |= texels.round() != 0.0;
            }
        }
        assert!(moved_any, "走远了总会挪格");
    }

    /// 段的外接球在光的视图里左右上下四个最远点（球的外切正方形的边）都要落在图里，包括对齐像素格时被往负方向挪了快一整格的情况。
    #[test]
    fn the_whole_bounding_sphere_stays_inside_after_snapping() {
        let cam = Camera3d::default();
        let corners = slice_corners(&cam, 1.5, 2.0, 12.0);
        let center = corners.iter().sum::<Vec3>() / 8.0;
        let radius = corners.iter().map(|c| c.distance(center)).fold(0.0, f32::max);
        // 光朝 -Z 照：光的视图里 x、y 就是世界的 x、y
        let check = |eye: Transform3d, size: u32| {
            let (m, texel) = cascade_matrix(Vec3::NEG_Z, &eye, &corners, [], size).unwrap();
            let c = eye.matrix().transform_point3(center);
            for d in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y] {
                let q = ndc(m, c + d * radius).unwrap();
                assert!(q.x.abs() <= 1.0 + 1e-5 && q.y.abs() <= 1.0 + 1e-5, "{eye:?} {d}: {q}");
            }
            texel
        };
        let size = 1024;
        let texel = check(Transform3d::default(), size);
        // 球心正好在格子的右上角前面一点：对齐时往左下挪了 0.999 格
        for frac in [0.0, 0.5, 0.999] {
            let at = Vec3::new((5.0 + frac) * texel, (-3.0 + frac) * texel, 0.0) - center;
            check(Transform3d { translation: at, ..Default::default() }, size);
        }
        for step in 0..50 {
            check(Transform3d::from_xyz(0.37 * step as f32, -0.21 * step as f32, 0.0), 64);
        }
    }

    #[test]
    fn spot_sees_what_is_in_front_inside_its_cone() {
        let position = Vec3::new(1.0, 4.0, 2.0);
        let direction = Vec3::new(0.2, -1.0, 0.1).normalize();
        let m = spot_matrix(position, direction, 30f32.to_radians(), 10.0).unwrap();
        // 锥里、range 以内
        for (angle, dist) in [(0.0, 1.0), (20.0, 5.0), (28.0, 9.0)] {
            let side = direction.cross(Vec3::X).normalize();
            let dir = Quat::from_axis_angle(side, f32::to_radians(angle)) * direction;
            assert!(inside(m, position + dir * dist), "{angle}° {dist}");
        }
        assert!(!inside(m, position - direction * 2.0), "灯背后");
        assert!(!inside(m, position + direction.cross(Vec3::X).normalize() * 2.0), "旁边 90°");
        assert!(!inside(m, position + direction * 0.05), "近平面以内");
        assert!(!inside(m, position + direction * 11.0), "range 以外");
        assert!(spot_matrix(position, Vec3::ZERO, 0.5, 10.0).is_none());
        assert!(spot_matrix(position, direction, 0.5, 0.0).is_none());
    }

    #[test]
    fn point_faces_cover_every_direction() {
        let position = Vec3::new(-2.0, 1.0, 3.0);
        let faces = point_matrices(position, 10.0).unwrap();
        let mut seed = 12345u32;
        let mut next = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0
        };
        for _ in 0..500 {
            let Some(dir) = Vec3::new(next(), next(), next()).try_normalize() else { continue };
            let p = position + dir * (0.5 + (next() + 1.0) * 4.0);
            let face = point_face(p - position);
            assert!(inside(faces[face], p), "{dir} 落在第 {face} 面");
        }
        // 每一面朝自己的轴
        for (i, axis) in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z].into_iter().enumerate() {
            assert_eq!(point_face(axis), i);
            assert!(inside(faces[i], position + axis * 3.0) && !inside(faces[i], position - axis * 3.0), "第 {i} 面");
        }
        assert!(point_matrices(position, -1.0).is_none());
    }

    #[test]
    fn boxes_intersect_the_view_they_overlap() {
        let spot = spot_matrix(Vec3::ZERO, Vec3::NEG_Z, 0.5, 10.0).unwrap();
        let unit = |c: Vec3| (c - Vec3::splat(0.5), c + Vec3::splat(0.5));
        assert!(intersects(spot, unit(Vec3::new(0.0, 0.0, -5.0))), "正前方");
        assert!(intersects(spot, unit(Vec3::new(2.8, 0.0, -5.0))), "压在锥的边上");
        assert!(!intersects(spot, unit(Vec3::new(0.0, 0.0, 5.0))), "背后");
        assert!(!intersects(spot, unit(Vec3::new(6.0, 0.0, -5.0))), "锥外旁边");
        assert!(!intersects(spot, unit(Vec3::new(0.0, 0.0, -12.0))), "range 以外");
        assert!(intersects(spot, (Vec3::new(-50.0, -1.0, -3.0), Vec3::new(50.0, 1.0, -2.0))), "横穿整个视野、角都在外面");
        let ortho = directx::orthographic(-1.0, 1.0, -1.0, 1.0, 0.0, 10.0);
        assert!(intersects(ortho, unit(Vec3::new(0.0, 0.0, -5.0))));
        assert!(!intersects(ortho, unit(Vec3::new(3.0, 0.0, -5.0))));
        assert!(!intersects(Mat4::IDENTITY, (Vec3::INFINITY, Vec3::NEG_INFINITY)), "空盒");
    }

    #[test]
    fn light_view_looks_down_the_light() {
        let v = light_view(Vec3::new(1.0, -1.0, 0.0)).unwrap();
        let d = v.transform_vector3(Vec3::new(1.0, -1.0, 0.0).normalize());
        assert!(d.abs_diff_eq(Vec3::NEG_Z, 1e-5), "{d}");
        assert!(light_view(Vec3::ZERO).is_none());
        assert!(Mat3::from_mat4(light_view(Vec3::NEG_Y).unwrap()).determinant() > 0.99);
    }
}
