//! 湍流：三个值噪声做势场，解析求 curl（无散度，粒子不会往一处聚）。

use glam::Vec3;

/// 湍流参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Noise3d {
    /// 加速度的大小（世界单位 / 秒²）。
    pub strength: f32,
    /// 噪声的频率：每世界单位几格。
    pub frequency: f32,
    /// 噪声场每秒沿 y 移动多少格。
    pub scroll: f32,
}

impl Default for Noise3d {
    fn default() -> Self {
        Self { strength: 1.0, frequency: 1.0, scroll: 0.0 }
    }
}

/// 格点每 `PERIOD` 格重复一次：滚动的偏移可以先取模，跑得再久也不丢精度。
pub(crate) const PERIOD: i32 = 256;

/// 一个格点上三个势场的值，各在 [-1, 1]（一次哈希出三个）。
fn corner(x: i32, y: i32, z: i32) -> Vec3 {
    let (x, y, z) = (x.rem_euclid(PERIOD) as u32, y.rem_euclid(PERIOD) as u32, z.rem_euclid(PERIOD) as u32);
    let mut h = x.wrapping_mul(0x8DA6_B343) ^ y.wrapping_mul(0xD816_3841) ^ z.wrapping_mul(0xCB1A_B31F);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5BD1_E995);
    h ^= h >> 15;
    let mut g = h.wrapping_mul(0x9E37_79B9);
    g ^= g >> 16;
    let unit = |bits: u32| (bits & 0x3FF) as f32 / 511.5 - 1.0;
    Vec3::new(unit(g), unit(g >> 10), unit(g >> 20))
}

/// 五次的平滑插值和它的导数：导数在格子边上连续。
fn fade(t: f32) -> (f32, f32) {
    (t * t * t * (t * (t * 6.0 - 15.0) + 10.0), 30.0 * t * t * (t - 1.0) * (t - 1.0))
}

/// 三个值噪声势场在 `p` 处的值和对 x、y、z 的偏导（解析求，八个格点各哈希一次）。
fn field(p: Vec3) -> (Vec3, [Vec3; 3]) {
    let base = p.floor();
    let f = p - base;
    let (x, y, z) = (base.x as i32, base.y as i32, base.z as i32);
    let ((u, du), (v, dv), (w, dw)) = (fade(f.x), fade(f.y), fade(f.z));
    let c = |dx: i32, dy: i32, dz: i32| corner(x + dx, y + dy, z + dz);
    let (c000, c100, c010, c110, c001, c101, c011, c111) = (c(0, 0, 0), c(1, 0, 0), c(0, 1, 0), c(1, 1, 0), c(0, 0, 1), c(1, 0, 1), c(0, 1, 1), c(1, 1, 1));
    // 三线性插值展开成 u、v、w 的多项式
    let (k1, k2, k3) = (c100 - c000, c010 - c000, c001 - c000);
    let (k4, k5, k6) = (c000 - c100 - c010 + c110, c000 - c010 - c001 + c011, c000 - c100 - c001 + c101);
    let k7 = c100 + c010 + c001 + c111 - c000 - c110 - c101 - c011;
    let value = c000 + k1 * u + k2 * v + k3 * w + k4 * (u * v) + k5 * (v * w) + k6 * (w * u) + k7 * (u * v * w);
    let dx = (k1 + k4 * v + k6 * w + k7 * (v * w)) * du;
    let dy = (k2 + k4 * u + k5 * w + k7 * (u * w)) * dv;
    let dz = (k3 + k5 * v + k6 * u + k7 * (u * v)) * dw;
    (value, [dx, dy, dz])
}

#[cfg(test)]
fn potential(p: Vec3) -> Vec3 {
    field(p).0
}

/// 势场在 `p` 处的 curl，长度大致在 0 … 几。
pub(crate) fn curl(p: Vec3) -> Vec3 {
    let (_, [dx, dy, dz]) = field(p);
    Vec3::new(dy.z - dz.y, dz.x - dx.z, dx.y - dy.x)
}

/// 噪声场在时刻 `time` 沿 y 滚过的格数，按周期取模（用 f64 算，跑得再久也准）。
pub(crate) fn scroll(time: f64, speed: f32) -> f32 {
    (time * speed as f64).rem_euclid(PERIOD as f64) as f32
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;

    #[test]
    fn curl_noise_is_smooth_bounded_and_divergence_free() {
        let points: Vec<Vec3> = (0..400).map(|i| Vec3::new(i as f32 * 0.137, (i * 7 % 23) as f32 * 0.31, (i * 13 % 17) as f32 * 0.29)).collect();
        let values: Vec<Vec3> = points.iter().map(|p| curl(*p)).collect();
        assert!(values.iter().all(|v| v.is_finite() && v.length() < 20.0));
        assert_eq!(curl(Vec3::new(1.3, 2.7, -0.4)), curl(Vec3::new(1.3, 2.7, -0.4)), "同一个点一样");
        let mean = values.iter().map(|v| v.length()).sum::<f32>() / values.len() as f32;
        assert!(mean > 0.1, "不是常数 0：{mean}");
        // 散度：有限差分后应该远小于场本身的变化
        let h = 1e-2;
        let div = |p: Vec3| {
            (curl(p + Vec3::X * h).x - curl(p - Vec3::X * h).x + curl(p + Vec3::Y * h).y - curl(p - Vec3::Y * h).y + curl(p + Vec3::Z * h).z - curl(p - Vec3::Z * h).z) / (2.0 * h)
        };
        let grad = |p: Vec3| (curl(p + Vec3::X * h).y - curl(p - Vec3::X * h).y).abs() / (2.0 * h);
        let (d, g) = points.iter().fold((0.0, 0.0), |(d, g), p| (d + div(*p).abs(), g + grad(*p)));
        assert!(d < g * 0.05, "散度 {d} 对比变化 {g}");
        // 相邻的点差不多（连续）
        assert!((curl(Vec3::splat(0.5)) - curl(Vec3::splat(0.5001))).length() < 0.01);
    }

    #[test]
    fn the_curl_matches_finite_differences_of_the_potentials() {
        let h = 1e-3;
        for i in 0..200 {
            let p = Vec3::new(i as f32 * 0.173 - 7.0, (i * 7 % 29) as f32 * 0.27, (i * 11 % 19) as f32 * -0.31);
            let d = |axis: Vec3| (potential(p + axis * h) - potential(p - axis * h)) / (2.0 * h);
            let (dx, dy, dz) = (d(Vec3::X), d(Vec3::Y), d(Vec3::Z));
            let expected = Vec3::new(dy.z - dz.y, dz.x - dx.z, dx.y - dy.x);
            assert!((curl(p) - expected).length() < 0.02 * (1.0 + expected.length()), "{p}: {} 对比 {expected}", curl(p));
        }
    }

    #[test]
    fn the_field_repeats_and_scrolling_stays_precise() {
        let p = Vec3::new(1.3, 2.7, -0.4);
        assert!((curl(p) - curl(p + Vec3::Y * PERIOD as f32)).length() < 1e-3, "沿 y 每 {PERIOD} 格重复");
        // 跑了很久：f32 的时间早就丢了小数，取模以后还是准的
        let late = scroll(1.0e9 + 0.25, 2.0);
        assert!((late - 0.5).abs() < 1e-4, "{late}");
        assert!((0.0..PERIOD as f32).contains(&scroll(-3.0, 1.0)));
    }
}
