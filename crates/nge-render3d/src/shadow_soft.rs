//! 软阴影、多灯投影和级联过渡的纯函数（见 docs/render3d-quality.md）：采样盘、级联混合、挑投影的灯、PCSS 系数。

use bevy_ecs::prelude::Component;
use glam::{Mat4, Vec2};

/// 采样盘的点数。
pub(crate) const DISK: usize = 16;
/// 聚光灯、点光源最多几盏同时投影子；层数最多 4 × 6。
pub(crate) const MAX_SHADOWED_LOCAL: usize = 4;
pub(crate) const MAX_LOCAL_LAYERS: usize = MAX_SHADOWED_LOCAL * 6;

/// 灯实体上的投影优先级：聚光灯、点光源超过 [`Shadows::max_lights`](crate::Shadows) 盏开了阴影时，大的先投；没有算 0。
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShadowPriority(pub i32);

/// 单位圆里分布均匀的 16 个点（Vogel 螺旋：半径 √((i + ½) / n)，每个转一个黄金角）；着色器里每个像素再转一个随机角度。
pub(crate) fn disk() -> [Vec2; DISK] {
    const GOLDEN: f32 = 2.399_963;
    std::array::from_fn(|i| Vec2::from_angle(i as f32 * GOLDEN) * ((i as f32 + 0.5) / DISK as f32).sqrt())
}

/// 采样盘写成 WGSL 的常量数组，替换进着色器源码里的 `POISSON_DISK`。
pub(crate) fn disk_wgsl() -> String {
    let points: Vec<String> = disk().iter().map(|p| format!("vec2f({:.6}, {:.6})", p.x, p.y)).collect();
    format!("array<vec2f, {DISK}>({})", points.join(", "))
}

/// 片元在相机前方 `depth` 处：用第几段，以及往下一段（最后一段是往「照到」）混多少。
/// 着色器里 `cascade_of` 是同一个公式，这份给测试对照。
/// `ends` 是每段的远端；混合带是每段最后 `blend` 的长度（第 0 段从 0 算起）。最后一段以外是 `None`。
#[cfg(test)]
fn cascade_blend(depth: f32, ends: &[f32], blend: f32) -> Option<(usize, f32)> {
    let c = ends.iter().position(|&end| depth < end)?;
    let start = if c == 0 { 0.0 } else { ends[c - 1] };
    let end = ends[c];
    // 不分段时远端是 f32::MAX：不淡出
    if end >= f32::MAX {
        return Some((c, 0.0));
    }
    let band = blend.clamp(0.0, 1.0) * (end - start);
    let w = if band > 0.0 { ((depth - (end - band)) / band).clamp(0.0, 1.0) } else { 0.0 };
    Some((c, w))
}

/// 一盏想投影子的聚光灯（`point` 为假）或点光源。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Candidate {
    pub point: bool,
    /// 在 `Globals` 的聚光灯 / 点光源里是第几盏。
    pub index: usize,
    pub priority: i32,
    /// 离相机多远（正交相机都是 0）。
    pub distance: f32,
}

/// 挑最多 `max` 盏：优先级大的先，同优先级离相机近的先，再按原来的顺序。返回（第几个候选, 阴影从第几层开始）：
/// 聚光灯占 1 层，点光源占 6 层，按挑中的顺序排。
pub(crate) fn pick(candidates: &[Candidate], max: usize) -> Vec<(usize, usize)> {
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    let distance = |i: usize| if candidates[i].distance.is_nan() { f32::INFINITY } else { candidates[i].distance };
    order.sort_by(|&a, &b| candidates[b].priority.cmp(&candidates[a].priority).then(distance(a).total_cmp(&distance(b))).then(a.cmp(&b)));
    let mut layer = 0;
    order
        .into_iter()
        .take(max.min(MAX_SHADOWED_LOCAL))
        .map(|i| {
            let first = layer;
            layer += if candidates[i].point { 6 } else { 1 };
            (i, first)
        })
        .collect()
}

/// 正交阴影矩阵（世界 → 0–1 的深度）的深度范围有多少世界单位：第三行的长度是 1 / 范围。
pub(crate) fn depth_range(m: Mat4) -> f32 {
    1.0 / glam::Vec3::new(m.x_axis.z, m.y_axis.z, m.z_axis.z).length()
}

#[cfg(test)]
mod tests {
    use glam::camera::rh::proj::directx;
    use glam::Vec3;

    use super::*;

    #[test]
    fn disk_points_are_spread_inside_the_unit_circle() {
        let d = disk();
        assert!(d.iter().all(|p| p.length() <= 1.0 && p.length() > 0.0), "{d:?}");
        for (i, a) in d.iter().enumerate() {
            for b in &d[i + 1..] {
                assert!(a.distance(*b) > 0.2, "{a} {b} 太近");
            }
        }
        let mean = d.iter().sum::<Vec2>() / DISK as f32;
        assert!(mean.length() < 0.1, "居中 {mean}");
        assert!(d.iter().any(|p| p.length() > 0.9), "铺到边上");
        // 转一下半径不变
        let r = Vec2::from_angle(1.3);
        assert!(d.iter().all(|p| (r.rotate(*p).length() - p.length()).abs() < 1e-6));
        let wgsl = disk_wgsl();
        assert!(wgsl.starts_with("array<vec2f, 16>(vec2f(") && wgsl.matches("vec2f(").count() == 16, "{wgsl}");
    }

    #[test]
    fn cascades_blend_into_the_next_near_their_end() {
        let ends = [10.0, 30.0, 60.0];
        assert_eq!(cascade_blend(5.0, &ends, 0.2), Some((0, 0.0)), "段中间不混");
        assert_eq!(cascade_blend(8.0, &ends, 0.2), Some((0, 0.0)), "混合带从 8 开始");
        let (c, w) = cascade_blend(9.0, &ends, 0.2).unwrap();
        assert!(c == 0 && (w - 0.5).abs() < 1e-5, "带子中间一半 {w}");
        let (c, w) = cascade_blend(28.0, &ends, 0.2).unwrap();
        assert!(c == 1 && (w - 0.5).abs() < 1e-5, "第 1 段带长 4，从 26 开始 {w}");
        assert_eq!(cascade_blend(25.0, &ends, 0.2), Some((1, 0.0)));
        let (c, w) = cascade_blend(59.0, &ends, 0.2).unwrap();
        assert!(c == 2 && w > 0.8, "最后一段末尾往照到混 {w}");
        assert_eq!(cascade_blend(60.0, &ends, 0.2), None, "最后一段以外没有影子");
        assert_eq!(cascade_blend(9.9, &ends, 0.0), Some((0, 0.0)), "blend 0 不混");
        // 不分段（远端是 f32::MAX）时不淡出
        assert_eq!(cascade_blend(1e6, &[f32::MAX], 0.2), Some((0, 0.0)));
        // 权重随深度单调
        let mut last = 0.0;
        for i in 0..20 {
            let (_, w) = cascade_blend(8.0 + i as f32 * 0.1, &ends, 0.2).unwrap();
            assert!(w >= last - 1e-6);
            last = w;
        }
    }

    fn spot(index: usize, priority: i32, distance: f32) -> Candidate {
        Candidate { point: false, index, priority, distance }
    }

    #[test]
    fn picks_by_priority_then_distance_then_order() {
        let c = [spot(0, 0, 5.0), spot(1, 2, 50.0), spot(2, 0, 1.0), spot(3, 0, 1.0)];
        assert_eq!(pick(&c, 2), [(1, 0), (2, 1)], "优先级大的先，再按近的");
        assert_eq!(pick(&c, 3), [(1, 0), (2, 1), (3, 2)], "一样近按原来的顺序");
        assert_eq!(pick(&c, 0), []);
        assert_eq!(pick(&c, 9).len(), 4);
        assert_eq!(pick(&[spot(0, 0, f32::NAN), spot(1, 0, 3.0)], 1), [(1, 0)], "算坏了的距离排最后");
    }

    #[test]
    fn point_lights_take_six_layers() {
        let point = |index, priority| Candidate { point: true, index, priority, distance: 0.0 };
        let c = [spot(0, 0, 0.0), point(0, 1), spot(1, 0, 0.0), point(1, 0)];
        assert_eq!(pick(&c, 4), [(1, 0), (0, 6), (2, 7), (3, 8)], "层号连续：点光源 6 层、聚光灯 1 层");
        let last = pick(&c, 4).last().copied().unwrap();
        assert!(last.1 + 6 <= MAX_LOCAL_LAYERS);
    }

    #[test]
    fn depth_range_reads_the_ortho_matrix() {
        let proj = directx::orthographic(-1.0, 1.0, -1.0, 1.0, 3.0, 43.0);
        let view = crate::shadow_views::light_view(Vec3::new(-1.0, -5.0, -2.0)).unwrap() * Mat4::from_translation(Vec3::new(-1.0, -5.0, -2.0));
        assert!((depth_range(proj * view) - 40.0).abs() < 1e-3, "{}", depth_range(proj * view));
    }
}
