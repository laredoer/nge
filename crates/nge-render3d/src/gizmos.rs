//! 3D 调试画线（见 docs/gameplay3d.md）：即时模式，这一帧画的只显示这一帧。

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use glam::{Quat, Vec3, Vec4};

/// 圆分几段。
pub const CIRCLE_SEGMENTS: usize = 32;

/// 和 `lines.wgsl` 的顶点输入一一对应。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct LineVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
}

/// 一组线段（两个顶点一段）。
#[derive(Debug, Default, Clone)]
pub struct Lines(pub(crate) Vec<LineVertex>);

impl Lines {
    /// 线段数。
    pub fn len(&self) -> usize {
        self.0.len() / 2
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 第 `i` 段的两头和颜色。
    pub fn segment(&self, i: usize) -> Option<(Vec3, Vec3, Vec4)> {
        let (a, b) = (self.0.get(2 * i)?, self.0.get(2 * i + 1)?);
        Some((Vec3::from(a.position), Vec3::from(b.position), Vec4::from(a.color)))
    }

    /// 一条线段。两头不是有限数时不画。
    pub fn line(&mut self, a: Vec3, b: Vec3, color: Vec4) {
        if !(a.is_finite() && b.is_finite() && color.is_finite()) {
            return;
        }
        let color = color.to_array();
        self.0.push(LineVertex { position: a.to_array(), color });
        self.0.push(LineVertex { position: b.to_array(), color });
    }

    /// 首尾相连的折线。
    fn strip(&mut self, points: impl IntoIterator<Item = Vec3>, color: Vec4) {
        let mut last = None;
        for p in points {
            if let Some(a) = last {
                self.line(a, p, color);
            }
            last = Some(p);
        }
    }

    /// 盒子的 12 条边：`size` 是整个长宽高。
    pub fn cuboid(&mut self, center: Vec3, rotation: Quat, size: Vec3, color: Vec4) {
        if !ok(center, rotation) || !size.is_finite() || size.min_element() < 0.0 {
            return;
        }
        let h = size / 2.0;
        let corner = |i: usize| center + rotation * (h * Vec3::new(sign(i & 1), sign(i & 2), sign(i & 4)));
        for i in 0..8 {
            // 每个角往三个方向中坐标为负的那几个邻居连，每条边只画一次
            for bit in [1, 2, 4] {
                if i & bit == 0 {
                    self.line(corner(i), corner(i | bit), color);
                }
            }
        }
    }

    /// 圆：`normal` 是圆所在平面的法线。
    pub fn circle(&mut self, center: Vec3, normal: Vec3, radius: f32, color: Vec4) {
        let Some(n) = normal.try_normalize() else { return };
        if !center.is_finite() || !radius.is_finite() || radius < 0.0 {
            return;
        }
        let (u, v) = n.any_orthonormal_pair();
        self.arc(center, u * radius, v * radius, std::f32::consts::TAU, color);
    }

    /// 从 `from` 方向往 `toward` 方向转 `angle` 的弧（两个向量互相垂直、一样长）。
    fn arc(&mut self, center: Vec3, from: Vec3, toward: Vec3, angle: f32, color: Vec4) {
        let n = ((CIRCLE_SEGMENTS as f32 * angle / std::f32::consts::TAU).round() as usize).max(1);
        self.strip(
            (0..=n).map(|i| {
                let t = angle * i as f32 / n as f32;
                center + from * t.cos() + toward * t.sin()
            }),
            color,
        );
    }

    /// 球：三个互相垂直的圆。
    pub fn sphere(&mut self, center: Vec3, radius: f32, color: Vec4) {
        for n in [Vec3::X, Vec3::Y, Vec3::Z] {
            self.circle(center, n, radius, color);
        }
    }

    /// 竖着（本地 y 轴）的胶囊：上下两个圆、四条竖线、两个方向的半圆。
    pub fn capsule(&mut self, center: Vec3, rotation: Quat, half_height: f32, radius: f32, color: Vec4) {
        if !ok(center, rotation) || !(half_height.is_finite() && radius.is_finite()) || half_height < 0.0 || radius < 0.0 {
            return;
        }
        let (x, y, z) = (rotation * Vec3::X, rotation * Vec3::Y, rotation * Vec3::Z);
        for s in [1.0, -1.0] {
            let c = center + y * half_height * s;
            self.circle(c, y, radius, color);
            // 半圆：从一侧经过顶点（底下的经过最低点）到另一侧
            self.arc(c, x * radius, y * radius * s, std::f32::consts::PI, color);
            self.arc(c, z * radius, y * radius * s, std::f32::consts::PI, color);
        }
        for side in [x, -x, z, -z] {
            self.line(center + side * radius + y * half_height, center + side * radius - y * half_height, color);
        }
    }

    /// 坐标轴：x 红、y 绿、z 蓝，各长 `length`。
    pub fn axes(&mut self, center: Vec3, rotation: Quat, length: f32) {
        if !ok(center, rotation) || !length.is_finite() {
            return;
        }
        for (axis, color) in [(Vec3::X, Vec4::new(1.0, 0.2, 0.2, 1.0)), (Vec3::Y, Vec4::new(0.2, 1.0, 0.2, 1.0)), (Vec3::Z, Vec4::new(0.3, 0.5, 1.0, 1.0))] {
            self.line(center, center + rotation * axis * length, color);
        }
    }
}

fn sign(bit: usize) -> f32 {
    if bit == 0 { -1.0 } else { 1.0 }
}

fn ok(center: Vec3, rotation: Quat) -> bool {
    center.is_finite() && rotation.is_finite() && rotation.length_squared() > 0.0
}

/// 3D 调试画线：画的东西默认按深度被模型挡住；[`Gizmos3d::on_top`] 的画在所有模型上面。
/// 每帧 `PreUpdate` 清空，在 `Update` / `PostUpdate` 里画。
#[derive(Resource, Debug, Default)]
pub struct Gizmos3d {
    depth: Lines,
    top: Lines,
}

impl Gizmos3d {
    /// 画在最前面，不被模型挡住。
    pub fn on_top(&mut self) -> &mut Lines {
        &mut self.top
    }

    /// 按深度被挡住的线。
    pub fn depth_tested(&self) -> &Lines {
        &self.depth
    }

    /// 画在最前面的线。
    pub fn top(&self) -> &Lines {
        &self.top
    }

    pub fn clear(&mut self) {
        self.depth.0.clear();
        self.top.0.clear();
    }
}

impl std::ops::Deref for Gizmos3d {
    type Target = Lines;

    fn deref(&self) -> &Lines {
        &self.depth
    }
}

impl std::ops::DerefMut for Gizmos3d {
    fn deref_mut(&mut self) -> &mut Lines {
        &mut self.depth
    }
}

pub(crate) fn clear_gizmos(mut gizmos: ResMut<Gizmos3d>) {
    gizmos.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Vec4 = Vec4::new(1.0, 0.0, 0.0, 1.0);

    fn ends(lines: &Lines) -> Vec<(Vec3, Vec3)> {
        (0..lines.len()).map(|i| lines.segment(i).map(|(a, b, _)| (a, b)).unwrap()).collect()
    }

    fn near(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1e-4
    }

    #[test]
    fn a_line_is_two_vertices_with_its_color() {
        let mut g = Gizmos3d::default();
        g.line(Vec3::ZERO, Vec3::X, RED);
        assert_eq!(g.len(), 1);
        assert_eq!(g.segment(0), Some((Vec3::ZERO, Vec3::X, RED)));
        assert!(g.top().is_empty(), "默认的不画在最前面");
        g.on_top().line(Vec3::Y, Vec3::Z, RED);
        assert_eq!((g.depth_tested().len(), g.top().len()), (1, 1));
    }

    #[test]
    fn bad_input_is_skipped() {
        let mut g = Gizmos3d::default();
        g.line(Vec3::NAN, Vec3::X, RED);
        g.line(Vec3::ZERO, Vec3::X, Vec4::splat(f32::INFINITY));
        g.cuboid(Vec3::ZERO, Quat::IDENTITY, Vec3::new(1.0, -1.0, 1.0), RED);
        g.cuboid(Vec3::ZERO, Quat::from_xyzw(0.0, 0.0, 0.0, 0.0), Vec3::ONE, RED);
        g.sphere(Vec3::ZERO, -1.0, RED);
        g.circle(Vec3::ZERO, Vec3::ZERO, 1.0, RED);
        g.capsule(Vec3::ZERO, Quat::IDENTITY, f32::NAN, 1.0, RED);
        g.axes(Vec3::INFINITY, Quat::IDENTITY, 1.0);
        assert!(g.is_empty());
    }

    #[test]
    fn a_cuboid_has_twelve_edges_on_its_turned_corners() {
        let mut g = Gizmos3d::default();
        let rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        g.cuboid(Vec3::new(10.0, 0.0, 0.0), rotation, Vec3::new(2.0, 4.0, 6.0), RED);
        let edges = ends(&g);
        assert_eq!(edges.len(), 12);
        for (a, b) in &edges {
            // 转了 90° 以后 x 方向是原来的 z：半长 3；z 方向是原来的 x：半长 1
            for p in [a, b] {
                assert!(near(Vec3::new((p.x - 10.0).abs(), p.y.abs(), p.z.abs()), Vec3::new(3.0, 2.0, 1.0)), "{p}");
            }
            let d = (*a - *b).abs();
            assert!([2.0, 4.0, 6.0].iter().any(|&l| (d.length() - l).abs() < 1e-4), "边长是 2、4、6 之一 {d}");
        }
    }

    #[test]
    fn a_sphere_is_three_circles_of_its_radius() {
        let mut g = Gizmos3d::default();
        g.sphere(Vec3::Y, 2.0, RED);
        assert_eq!(g.len(), 3 * CIRCLE_SEGMENTS);
        for (a, b) in ends(&g) {
            assert!((a.distance(Vec3::Y) - 2.0).abs() < 1e-4 && (b.distance(Vec3::Y) - 2.0).abs() < 1e-4);
        }
        // 三个圆分别垂直于 x、y、z
        for (i, axis) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().enumerate() {
            let (a, _, _) = g.segment(i * CIRCLE_SEGMENTS).unwrap();
            assert!((a - Vec3::Y).dot(axis).abs() < 1e-4);
        }
    }

    #[test]
    fn a_capsule_reaches_its_tips_and_sides() {
        let mut g = Gizmos3d::default();
        g.capsule(Vec3::ZERO, Quat::IDENTITY, 1.0, 0.5, RED);
        let points: Vec<Vec3> = ends(&g).into_iter().flat_map(|(a, b)| [a, b]).collect();
        let top = points.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        let bottom = points.iter().map(|p| p.y).fold(f32::MAX, f32::min);
        assert!((top - 1.5).abs() < 1e-4 && (bottom + 1.5).abs() < 1e-4, "顶到 ±(半高 + 半径) {top} {bottom}");
        assert!(points.iter().all(|p| Vec3::new(p.x, 0.0, p.z).length() <= 0.5 + 1e-4), "不超出半径");
        // 转 90°（绕 z）以后躺平，最远在 x 上
        let mut g = Gizmos3d::default();
        g.capsule(Vec3::ZERO, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 1.0, 0.5, RED);
        let far = ends(&g).into_iter().flat_map(|(a, b)| [a, b]).map(|p| p.x.abs()).fold(0.0, f32::max);
        assert!((far - 1.5).abs() < 1e-4, "{far}");
    }

    #[test]
    fn axes_are_red_green_blue() {
        let mut g = Gizmos3d::default();
        g.axes(Vec3::ONE, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 2.0);
        assert_eq!(g.len(), 3);
        let (a, b, c) = g.segment(0).unwrap();
        assert!(near(a, Vec3::ONE) && near(b, Vec3::ONE + Vec3::Y * 2.0), "x 轴转 90° 后朝上 {b}");
        assert!(c.x > c.y && c.x > c.z);
        assert!(g.segment(1).unwrap().2.y > 0.9 && g.segment(2).unwrap().2.z > 0.9);
    }

    #[test]
    fn gizmos_are_cleared_every_frame() {
        let mut app = nge_ecs::App::new();
        app.add_plugins(crate::Render3dPlugin);
        app.add_systems(nge_ecs::Update, |mut g: ResMut<Gizmos3d>| g.line(Vec3::ZERO, Vec3::X, RED));
        let viewport = nge_core::Viewport { size: [10.0, 10.0], pixels: [10, 10] };
        app.tick(viewport, 0.016);
        app.tick(viewport, 0.016);
        assert_eq!(app.world().resource::<Gizmos3d>().len(), 1, "上一帧画的清掉，只剩这一帧的");
    }
}
