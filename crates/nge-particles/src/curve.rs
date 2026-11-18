//! 随寿命变化的曲线和渐变（见 docs/particles3d-more.md）：若干关键点，中间线性插值，两头之外取两头的值。

use glam::Vec4;

/// 一条曲线：`(t, 值)`，`t` 一般是年龄 / 寿命。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Curve(Vec<(f32, f32)>);

/// 颜色渐变：`(t, 线性 RGBA)`。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Gradient(Vec<(f32, Vec4)>);

/// 按 `t` 排序、丢掉有 NaN 的点。
fn keys<T>(keys: impl IntoIterator<Item = (f32, T)>, bad: impl Fn(&T) -> bool) -> Vec<(f32, T)> {
    let mut keys: Vec<(f32, T)> = keys.into_iter().filter(|(t, v)| !t.is_nan() && !bad(v)).collect();
    keys.sort_by(|a, b| a.0.total_cmp(&b.0));
    keys
}

fn eval<T: Copy>(keys: &[(f32, T)], t: f32, lerp: impl Fn(T, T, f32) -> T) -> Option<T> {
    let t = if t.is_nan() { 0.0 } else { t };
    let (first, last) = (keys.first()?, keys.last()?);
    if t <= first.0 {
        return Some(first.1);
    }
    if t >= last.0 {
        return Some(last.1);
    }
    let i = keys.partition_point(|(k, _)| *k <= t);
    let ((t0, a), (t1, b)) = (keys[i - 1], keys[i]);
    let span = t1 - t0;
    Some(if span > 0.0 { lerp(a, b, (t - t0) / span) } else { a })
}

impl Curve {
    pub fn new(points: impl IntoIterator<Item = (f32, f32)>) -> Self {
        Self(keys(points, |v| v.is_nan()))
    }

    pub fn keys(&self) -> &[(f32, f32)] {
        &self.0
    }

    /// 在 `t` 处的值；没有点时是 `None`（当没设）。
    pub fn eval(&self, t: f32) -> Option<f32> {
        eval(&self.0, t, |a, b, k| a + (b - a) * k)
    }
}

impl Gradient {
    pub fn new(points: impl IntoIterator<Item = (f32, Vec4)>) -> Self {
        Self(keys(points, |v| v.is_nan()))
    }

    pub fn keys(&self) -> &[(f32, Vec4)] {
        &self.0
    }

    pub fn eval(&self, t: f32) -> Option<Vec4> {
        eval(&self.0, t, Vec4::lerp)
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec4;

    use super::*;

    #[test]
    fn curves_interpolate_between_keys_and_hold_the_ends() {
        let c = Curve::new([(1.0, 0.0), (0.0, 2.0), (0.5, 4.0), (f32::NAN, 9.0)]);
        assert_eq!(c.keys().len(), 3, "NaN 的点丢掉、按 t 排序");
        assert_eq!(c.eval(0.0), Some(2.0));
        assert_eq!(c.eval(0.25), Some(3.0));
        assert_eq!(c.eval(0.75), Some(2.0));
        assert_eq!((c.eval(-1.0), c.eval(2.0)), (Some(2.0), Some(0.0)), "两头之外取两头的值");
        assert_eq!(Curve::new([(0.3, 5.0)]).eval(0.9), Some(5.0), "一个点是常数");
        assert_eq!(Curve::default().eval(0.5), None, "空的当没设");
        assert_eq!(Curve::new([(0.5, 1.0), (0.5, 3.0)]).eval(0.5), Some(1.0), "同一个 t 两个点不除 0");
        assert!(Curve::new([(0.0, 1.0), (1.0, 3.0)]).eval(f32::NAN).is_some_and(f32::is_finite), "NaN 的 t 不出 NaN");
    }

    #[test]
    fn gradients_mix_colors_per_channel() {
        let g = Gradient::new([(0.0, Vec4::new(1.0, 0.0, 0.0, 1.0)), (1.0, Vec4::new(0.0, 0.0, 1.0, 0.0)), (0.5, Vec4::ONE)]);
        assert_eq!(g.eval(0.25), Some(Vec4::new(1.0, 0.5, 0.5, 1.0)));
        assert_eq!(g.eval(1.0), Some(Vec4::new(0.0, 0.0, 1.0, 0.0)));
        assert_eq!(Gradient::default().eval(0.5), None);
    }
}
