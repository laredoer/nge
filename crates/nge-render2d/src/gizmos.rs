//! 调试绘制：系统里每帧画线、框、圆，只显示这一帧（见 docs/gizmos.md）。

use bevy_ecs::prelude::ResMut;
use bevy_ecs::system::SystemParam;
use glam::{Vec2, Vec4};

use crate::assets::TextureId;
use crate::sprite::{SpriteImage, SpriteInstance, SpriteInstances};

/// 调试绘制的 z：盖在所有东西上面。
pub const GIZMO_Z: f32 = 1000.0;
/// 圆按这么多段折线画。
const CIRCLE_SEGMENTS: usize = 32;
const WHITE: SpriteImage = SpriteImage::Texture(TextureId::WHITE);

/// 往 [`SpriteInstances`] 里放白色实例的系统参数。坐标、宽度都是世界单位，颜色是线性 RGBA。
#[derive(SystemParam)]
pub struct Gizmos<'w> {
    instances: ResMut<'w, SpriteInstances>,
}

impl<'w> From<ResMut<'w, SpriteInstances>> for Gizmos<'w> {
    fn from(instances: ResMut<'w, SpriteInstances>) -> Self {
        Self { instances }
    }
}

impl Gizmos<'_> {
    /// 从 `a` 到 `b` 的线段，线宽 `width`。
    pub fn line(&mut self, a: Vec2, b: Vec2, width: f32, color: Vec4) {
        let d = b - a;
        let length = d.length();
        if !(length > 0.0 && length.is_finite() && width > 0.0 && width.is_finite()) {
            return;
        }
        self.quad((a + b) / 2.0, Vec2::new(length, width), d.to_angle(), color);
    }

    /// 实心矩形，不转。
    pub fn rect(&mut self, center: Vec2, size: Vec2, color: Vec4) {
        let size = size.abs();
        if !center.is_finite() || !size.is_finite() || size.min_element() <= 0.0 {
            return;
        }
        self.quad(center, size, 0.0, color);
    }

    /// 空心矩形，绕中心转 `rotation`（弧度，逆时针）。一边是 0 时画成一条线（碰撞体压扁了也看得到）。
    pub fn rect_outline(&mut self, center: Vec2, size: Vec2, rotation: f32, width: f32, color: Vec4) {
        let half = size.abs() / 2.0;
        let corners = [Vec2::new(-1.0, -1.0), Vec2::new(1.0, -1.0), Vec2::new(1.0, 1.0), Vec2::new(-1.0, 1.0)]
            .map(|c| center + Vec2::from_angle(rotation).rotate(c * half));
        self.polyline(&corners, width, color);
    }

    /// 空心圆。
    pub fn circle(&mut self, center: Vec2, radius: f32, width: f32, color: Vec4) {
        if !(radius > 0.0 && radius.is_finite()) {
            return;
        }
        let step = std::f32::consts::TAU / CIRCLE_SEGMENTS as f32;
        let points: Vec<Vec2> = (0..CIRCLE_SEGMENTS).map(|i| center + radius * Vec2::from_angle(i as f32 * step)).collect();
        self.polyline(&points, width, color);
    }

    /// 首尾相连的折线；线宽方向往两边各伸出半个线宽，转角处补上缝。
    fn polyline(&mut self, points: &[Vec2], width: f32, color: Vec4) {
        for (i, &a) in points.iter().enumerate() {
            let b = points[(i + 1) % points.len()];
            let d = (b - a).normalize_or_zero() * width / 2.0;
            self.line(a - d, b + d, width, color);
        }
    }

    fn quad(&mut self, position: Vec2, size: Vec2, rotation: f32, color: Vec4) {
        self.instances.push(WHITE, SpriteInstance { position, size, rotation, z: GIZMO_Z, color });
    }
}

#[cfg(test)]
mod tests {
    use bevy_ecs::prelude::*;
    use bevy_ecs::system::RunSystemOnce;

    use super::*;

    const RED: Vec4 = Vec4::new(1.0, 0.0, 0.0, 1.0);

    fn draw(f: fn(&mut Gizmos)) -> Vec<SpriteInstance> {
        let mut world = World::new();
        world.init_resource::<SpriteInstances>();
        world.run_system_once(move |mut g: Gizmos| f(&mut g)).unwrap();
        world.resource::<SpriteInstances>().items.iter().map(|&(image, i)| {
            assert_eq!(image, WHITE);
            i
        }).collect()
    }

    fn close(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn a_line_is_one_rotated_quad_between_its_ends() {
        let [q] = draw(|g| g.line(Vec2::new(0.0, 0.0), Vec2::new(0.0, 10.0), 2.0, RED))[..] else { panic!() };
        assert!(close(q.position, Vec2::new(0.0, 5.0)));
        assert!(close(q.size, Vec2::new(10.0, 2.0)));
        assert!((q.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert_eq!((q.z, q.color), (GIZMO_Z, RED));
    }

    #[test]
    fn a_rect_is_one_quad_and_its_outline_four_edges() {
        let [q] = draw(|g| g.rect(Vec2::new(1.0, 2.0), Vec2::new(-4.0, 6.0), RED))[..] else { panic!() };
        assert_eq!((q.position, q.size, q.rotation), (Vec2::new(1.0, 2.0), Vec2::new(4.0, 6.0), 0.0));

        let edges = draw(|g| g.rect_outline(Vec2::ZERO, Vec2::new(20.0, 10.0), 0.0, 2.0, RED));
        assert_eq!(edges.len(), 4);
        // 每条边在矩形边上，长度多出一个线宽把角补上
        for (at, len) in [(Vec2::new(0.0, -5.0), 22.0), (Vec2::new(10.0, 0.0), 12.0), (Vec2::new(0.0, 5.0), 22.0), (Vec2::new(-10.0, 0.0), 12.0)] {
            assert!(edges.iter().any(|e| close(e.position, at) && (e.size.x - len).abs() < 1e-3), "{at} {edges:?}");
        }

        let turned = draw(|g| g.rect_outline(Vec2::ZERO, Vec2::new(20.0, 10.0), std::f32::consts::FRAC_PI_2, 2.0, RED));
        assert!(turned.iter().any(|e| close(e.position, Vec2::new(5.0, 0.0))), "转 90° 后底边到了右边: {turned:?}");
    }

    #[test]
    fn a_circle_is_a_ring_of_segments() {
        let ring = draw(|g| g.circle(Vec2::new(3.0, 4.0), 50.0, 1.0, RED));
        assert_eq!(ring.len(), CIRCLE_SEGMENTS);
        for s in &ring {
            let r = (s.position - Vec2::new(3.0, 4.0)).length();
            assert!(r > 49.0 && r < 50.0, "每段的中点在圆上（弦略靠里）: {r}");
        }
    }

    #[test]
    fn bad_input_draws_nothing() {
        let drawn = draw(|g| {
            g.line(Vec2::ONE, Vec2::ONE, 2.0, RED);
            g.line(Vec2::ZERO, Vec2::NAN, 2.0, RED);
            g.line(Vec2::ZERO, Vec2::X, 0.0, RED);
            g.rect(Vec2::ZERO, Vec2::new(0.0, 3.0), RED);
            g.rect(Vec2::NAN, Vec2::ONE, RED);
            g.circle(Vec2::ZERO, 0.0, 1.0, RED);
            g.circle(Vec2::ZERO, f32::NAN, 1.0, RED);
            g.circle(Vec2::NAN, 5.0, 1.0, RED);
            g.rect_outline(Vec2::ZERO, Vec2::ONE, f32::NAN, 1.0, RED);
            g.rect_outline(Vec2::ZERO, Vec2::NAN, 0.0, 1.0, RED);
            g.rect_outline(Vec2::ZERO, Vec2::ONE, 0.0, -1.0, RED);
        });
        assert!(drawn.is_empty(), "{drawn:?}");
    }
}
