//! 重复铺满：一个精灵沿世界的 x 或 y 轴一个挨一个铺满视野，用来画无限长的远山、云、地面（见 docs/repeat.md）。

use bevy_ecs::prelude::Component;
use glam::Vec2;

/// 精灵沿哪个轴重复。步长是精灵画出来的宽、高（含缩放，不管旋转）。
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repeat {
    pub x: bool,
    pub y: bool,
}

/// 一个精灵一帧最多画几份，步长太小（几乎是 0）时不至于卡死。
pub const MAX_COPIES: usize = 4096;

/// 视野：世界里的外接框 (min, max)。
pub type View = (Vec2, Vec2);

/// 重复的精灵这一帧要画的中心位置，追加到 `out`：沿重复的轴，和视野相交的那几份；不重复的轴只有原来那一份（不在视野里时一份都不画）。
/// `half` 是一份画出来的外接框的一半（含旋转）。份数超过 [`MAX_COPIES`] 或步长不是正数时，只画原来那一份，返回要画的份数（`Err`）。
/// 在 f64 里算，坐标很大时也按原来的位置对齐。
pub fn copies(center: Vec2, step: Vec2, half: Vec2, repeat: Repeat, view: View, out: &mut Vec<Vec2>) -> Result<(), f64> {
    // 一个轴上要画的 k（中心 + k × 步长），空的时候 first > last
    let range = |on: bool, c: f32, step: f32, half: f32, lo: f32, hi: f32| -> Result<(f64, f64), f64> {
        let (c, half, lo, hi) = (c as f64, half as f64, lo as f64, hi as f64);
        if !on {
            let visible = c + half >= lo && c - half <= hi;
            return Ok(if visible { (0.0, 0.0) } else { (1.0, 0.0) });
        }
        if !(step > 0.0) {
            return Err(f64::INFINITY);
        }
        let step = step as f64;
        Ok((((lo - half - c) / step).ceil(), ((hi + half - c) / step).floor()))
    };
    let count = |(first, last): (f64, f64)| (last - first + 1.0).max(0.0);
    let xs = range(repeat.x, center.x, step.x, half.x, view.0.x, view.1.x)?;
    let ys = range(repeat.y, center.y, step.y, half.y, view.0.y, view.1.y)?;
    let n = count(xs) * count(ys);
    if !(n <= MAX_COPIES as f64) {
        out.push(center);
        return Err(n);
    }
    let (c, step) = (center.as_dvec2(), step.as_dvec2());
    let (mut ky, mut kx);
    ky = ys.0;
    while ky <= ys.1 {
        kx = xs.0;
        while kx <= xs.1 {
            out.push((c + glam::DVec2::new(kx * step.x, ky * step.y)).as_vec2());
            kx += 1.0;
        }
        ky += 1.0;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::{MAX_COPIES, Repeat, copies};

    const X: Repeat = Repeat { x: true, y: false };
    const VIEW: (Vec2, Vec2) = (Vec2::new(-100.0, -50.0), Vec2::new(100.0, 50.0));

    fn run(center: Vec2, step: Vec2, repeat: Repeat, view: (Vec2, Vec2)) -> (Vec<Vec2>, bool) {
        let mut out = Vec::new();
        let ok = copies(center, step, step / 2.0, repeat, view, &mut out).is_ok();
        (out, ok)
    }

    #[test]
    fn copies_cover_the_view_along_x_only() {
        let (xs, ok) = run(Vec2::new(5.0, 30.0), Vec2::new(40.0, 20.0), X, VIEW);
        assert!(ok);
        // 中心 5 + 40k，左右各伸出 20：和 [-100, 100] 相交的 k 是 -3 … 2（-115 的右边是 -95，相交；125 的左边 105 不相交）
        let want: Vec<Vec2> = (-3..=2).map(|k| Vec2::new(5.0 + 40.0 * k as f32, 30.0)).collect();
        assert_eq!(xs, want);
    }

    #[test]
    fn the_far_away_original_still_tiles_into_view() {
        // 原来那份远在视野外，重复出来的照样铺满视野
        let (xs, _) = run(Vec2::new(10_000.0, 0.0), Vec2::new(50.0, 10.0), X, VIEW);
        assert!(xs.first().unwrap().x - 25.0 <= -100.0 && xs.last().unwrap().x + 25.0 >= 100.0, "{xs:?}");
        assert!(xs.iter().all(|p| (p.x - 10_000.0) % 50.0 == 0.0), "按原来的位置对齐");
    }

    #[test]
    fn both_axes_make_a_grid() {
        let (xs, _) = run(Vec2::ZERO, Vec2::splat(100.0), Repeat { x: true, y: true }, VIEW);
        // x：-100、0、100；y：-100、0、100（只碰到视野的边也算）
        assert_eq!(xs.len(), 9, "{xs:?}");
    }

    #[test]
    fn a_tiny_step_or_too_many_copies_draws_only_the_original() {
        for step in [Vec2::new(0.0, 10.0), Vec2::new(-5.0, 10.0), Vec2::new(f32::NAN, 10.0), Vec2::new(200.0 / MAX_COPIES as f32 / 2.0, 10.0)] {
            let mut out = Vec::new();
            let err = copies(Vec2::new(3.0, 4.0), step, step / 2.0, X, VIEW, &mut out);
            assert!(err.is_err(), "{step}");
            assert!(out.len() <= 1 && out.iter().all(|&p| p == Vec2::new(3.0, 4.0)), "{step} {out:?}");
        }
        // 份数报回去，warn 里写得清楚
        let mut out = Vec::new();
        assert_eq!(copies(Vec2::ZERO, Vec2::splat(1.0), Vec2::splat(0.5), Repeat { x: true, y: true }, VIEW, &mut out), Err(201.0 * 101.0));
    }

    #[test]
    fn the_axis_that_does_not_repeat_out_of_view_draws_nothing() {
        // 横着重复的远山，竖直方向已经出了视野：一份都不画
        let (xs, ok) = run(Vec2::new(0.0, 500.0), Vec2::new(40.0, 20.0), X, VIEW);
        assert_eq!((xs.len(), ok), (0, true));
    }

    #[test]
    fn far_away_copies_stay_on_the_grid() {
        // 中心在 3e8、步长 16：f32 里算会挤成一份或者跑偏
        let (xs, ok) = run(Vec2::new(3.0e8, 0.0), Vec2::new(16.0, 8.0), X, VIEW);
        assert!(ok);
        assert!(xs.len() >= 13 && xs.first().unwrap().x <= -100.0 + 8.0 && xs.last().unwrap().x >= 100.0 - 8.0, "{xs:?}");
    }

    #[test]
    fn rotation_widens_what_counts_as_visible() {
        // 转了 90° 的 40 × 200：外接框半宽 100，离视野左边 150 的那份也露出来
        let mut out = Vec::new();
        copies(Vec2::new(-150.0, 0.0), Vec2::new(40.0, 200.0), Vec2::new(100.0, 20.0), X, VIEW, &mut out);
        assert_eq!(out.first().map(|p| p.x), Some(-190.0));
    }
}
