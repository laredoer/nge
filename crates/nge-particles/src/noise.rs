//! 湍流：三个值噪声做势场，有限差分求 curl（无散度，粒子不会往一处聚）。

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

fn hash(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343) ^ (y as u32).wrapping_mul(0xD816_3841) ^ (z as u32).wrapping_mul(0xCB1A_B31F) ^ seed.wrapping_mul(0x9E37_79B9);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5BD1_E995);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 23) as f32 - 1.0
}

/// 五次的平滑插值：导数在格子边上连续。
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// 值噪声，[-1, 1]。
fn value(p: Vec3, seed: u32) -> f32 {
    let base = p.floor();
    let f = p - base;
    let (x, y, z) = (base.x as i32, base.y as i32, base.z as i32);
    let (u, v, w) = (fade(f.x), fade(f.y), fade(f.z));
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let corner = |dx: i32, dy: i32, dz: i32| hash(x + dx, y + dy, z + dz, seed);
    let x0 = lerp(lerp(corner(0, 0, 0), corner(1, 0, 0), u), lerp(corner(0, 1, 0), corner(1, 1, 0), u), v);
    let x1 = lerp(lerp(corner(0, 0, 1), corner(1, 0, 1), u), lerp(corner(0, 1, 1), corner(1, 1, 1), u), v);
    lerp(x0, x1, w)
}

const H: f32 = 1e-3;

/// 三个势场在 `p` 处的偏导：`d[i][j]` 是第 `i` 个场对第 `j` 个坐标。
fn jacobian(p: Vec3) -> [[f32; 3]; 3] {
    let mut d = [[0.0; 3]; 3];
    for (i, row) in d.iter_mut().enumerate() {
        for (j, axis) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().enumerate() {
            row[j] = (value(p + axis * H, i as u32) - value(p - axis * H, i as u32)) / (2.0 * H);
        }
    }
    d
}

/// 势场的 curl，长度大致在 0 … 几。
pub(crate) fn curl(p: Vec3) -> Vec3 {
    let d = jacobian(p);
    Vec3::new(d[2][1] - d[1][2], d[0][2] - d[2][0], d[1][0] - d[0][1])
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
}
