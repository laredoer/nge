//! 法线贴图和挡光（见 docs/render2d-more.md）。

use bevy_ecs::prelude::Component;
use glam::{Vec2, Vec4};
use nge_assets::ImageId;

use crate::effects::Light2d;

/// 光照 uniform 里最多放多少条挡光的边。
pub const MAX_OCCLUDER_EDGES: usize = 128;

/// 精灵的法线图：和精灵同样排布（图集一格对一格），RGB = 法线 × 0.5 + 0.5，绿色朝上、蓝色朝外。
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalMap(pub ImageId);

/// 挡住开了 `shadows` 的灯。`points` 空时是精灵画出来的矩形；不空时是本地坐标的多边形（首尾自动连上），跟着 `Transform2d` 走。
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct Occluder2d {
    pub points: Vec<Vec2>,
}

impl Occluder2d {
    pub fn polygon(points: impl Into<Vec<Vec2>>) -> Self {
        Self { points: points.into() }
    }
}

/// 以原点为中心、`size` 大的矩形的四个角（逆时针）。
pub(crate) fn rect(size: Vec2) -> [Vec2; 4] {
    let h = size / 2.0;
    [Vec2::new(-h.x, -h.y), Vec2::new(h.x, -h.y), Vec2::new(h.x, h.y), Vec2::new(-h.x, h.y)]
}

/// 本地多边形按缩放、旋转、位置变到世界里，首尾连成边；两个点时只有一条边，不到两个点没有边。
pub(crate) fn outline(points: &[Vec2], position: Vec2, rotation: f32, scale: Vec2, out: &mut Vec<(Vec2, Vec2)>) {
    if points.len() < 2 {
        return;
    }
    let turn = Vec2::from_angle(rotation);
    let world = |p: Vec2| position + turn.rotate(p * scale);
    let closing = if points.len() > 2 { points.len() } else { 1 };
    for i in 0..closing {
        out.push((world(points[i]), world(points[(i + 1) % points.len()])));
    }
}

/// 点到线段的距离。
fn distance(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 0.0 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

/// 只留离某盏开了 `shadows` 的灯不到半径的边，打成 uniform 的样子；超过上限时返回丢掉了几条。
pub(crate) fn pack_edges(edges: &[(Vec2, Vec2)], lights: &[Light2d], out: &mut Vec<[f32; 4]>) -> usize {
    out.clear();
    // 和打进 uniform 的灯一样：跳过坏的，只看前 MAX_LIGHTS 盏
    let usable = lights.iter().filter(|l| l.radius.is_finite() && l.radius > 0.0 && l.position.is_finite()).take(crate::MAX_LIGHTS);
    let casting: Vec<&Light2d> = usable.filter(|l| l.shadows).collect();
    let mut dropped = 0;
    for &(a, b) in edges.iter().filter(|(a, b)| a.is_finite() && b.is_finite() && a != b) {
        if !casting.iter().any(|l| distance(a, b, l.position) < l.radius) {
            continue;
        }
        if out.len() == MAX_OCCLUDER_EDGES {
            dropped += 1;
            continue;
        }
        out.push(Vec4::new(a.x, a.y, b.x, b.y).into());
    }
    dropped
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;

    fn light(x: f32, radius: f32, shadows: bool) -> Light2d {
        Light2d { position: Vec2::new(x, 0.0), radius, color: Vec3::ONE, intensity: 1.0, height: 0.0, shadows }
    }

    #[test]
    fn outlines_close_polygons_and_follow_the_transform() {
        let mut edges = Vec::new();
        outline(&rect(Vec2::new(4.0, 2.0)), Vec2::new(10.0, 0.0), 0.0, Vec2::ONE, &mut edges);
        assert_eq!(edges.len(), 4);
        assert_eq!(edges[0], (Vec2::new(8.0, -1.0), Vec2::new(12.0, -1.0)));
        assert_eq!(edges[3], (Vec2::new(8.0, 1.0), Vec2::new(8.0, -1.0)), "首尾连上");
        edges.clear();
        // 两个点一条边；缩放、转 90°
        outline(&[Vec2::ZERO, Vec2::X], Vec2::new(1.0, 1.0), std::f32::consts::FRAC_PI_2, Vec2::splat(3.0), &mut edges);
        assert_eq!(edges.len(), 1);
        assert!((edges[0].1 - Vec2::new(1.0, 4.0)).length() < 1e-5, "{:?}", edges[0]);
        edges.clear();
        outline(&[Vec2::ZERO], Vec2::ZERO, 0.0, Vec2::ONE, &mut edges);
        assert!(edges.is_empty(), "一个点没有边");
    }

    #[test]
    fn only_edges_near_shadow_casting_lights_are_packed() {
        let edges = [
            (Vec2::new(5.0, -1.0), Vec2::new(5.0, 1.0)),
            (Vec2::new(50.0, -1.0), Vec2::new(50.0, 1.0)),
            // 两头都在半径外，中间穿过半径里
            (Vec2::new(-20.0, 3.0), Vec2::new(20.0, 3.0)),
            (Vec2::new(1.0, 1.0), Vec2::new(1.0, 1.0)),
            (Vec2::new(f32::NAN, 0.0), Vec2::ZERO),
        ];
        let mut out = Vec::new();
        assert_eq!(pack_edges(&edges, &[light(0.0, 10.0, true), light(50.0, 10.0, false)], &mut out), 0);
        assert_eq!(out, [[5.0, -1.0, 5.0, 1.0], [-20.0, 3.0, 20.0, 3.0]], "没开 shadows 的灯附近的不要，退化的、坏的不要");
        assert_eq!(pack_edges(&edges, &[light(0.0, 10.0, false)], &mut out), 0);
        assert!(out.is_empty(), "没有灯开 shadows");
        // 排在 32 盏以外的灯不画，它附近的边也不要
        let mut lights: Vec<Light2d> = (0..crate::MAX_LIGHTS).map(|_| light(500.0, 1.0, false)).collect();
        lights.push(light(0.0, 10.0, true));
        pack_edges(&edges, &lights, &mut out);
        assert!(out.is_empty());
        let many: Vec<_> = (0..MAX_OCCLUDER_EDGES + 3).map(|i| (Vec2::new(1.0, i as f32 * 0.01), Vec2::new(2.0, 0.0))).collect();
        assert_eq!(pack_edges(&many, &[light(0.0, 10.0, true)], &mut out), 3);
        assert_eq!(out.len(), MAX_OCCLUDER_EDGES);
    }
}
