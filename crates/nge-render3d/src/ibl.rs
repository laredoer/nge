//! 环境光照的 CPU 烘焙（见 docs/render3d-quality.md）：立方体的方向约定、SH9、预滤波的 mip 链、BRDF LUT。
//! 全在 CPU 上算，GPU 只采样：不用计算着色器、storage texture、cube array。

use glam::{Vec2, Vec3};

/// 环境立方体的边长和级数（64 → 1）。
pub(crate) const ENV_SIZE: u32 = 64;
pub(crate) const ENV_LEVELS: u32 = 7;
/// 预滤波每格采几个方向。
pub(crate) const ENV_SAMPLES: u32 = 64;
pub(crate) const LUT_SIZE: u32 = 32;
pub(crate) const LUT_SAMPLES: u32 = 128;

/// 每个面的（前、u 增大的方向、图上方）。
type Frame = [Vec3; 3];

/// GPU 立方体纹理的采样约定（D3D / Vulkan / Metal / WebGPU 一样），层顺序 +X、-X、+Y、-Y、+Z、-Z。
const GPU: [Frame; 6] = [
    [Vec3::X, Vec3::NEG_Z, Vec3::Y],
    [Vec3::NEG_X, Vec3::Z, Vec3::Y],
    [Vec3::Y, Vec3::X, Vec3::NEG_Z],
    [Vec3::NEG_Y, Vec3::X, Vec3::Z],
    [Vec3::Z, Vec3::X, Vec3::Y],
    [Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y],
];

/// 用户的 6 张图：从里面往外看到的画面，不镜像；侧面上方是 +Y，抬头那张下边接 -Z 面，低头那张上边接 -Z 面。
const USER: [Frame; 6] = [
    [Vec3::X, Vec3::Z, Vec3::Y],
    [Vec3::NEG_X, Vec3::NEG_Z, Vec3::Y],
    [Vec3::Y, Vec3::X, Vec3::Z],
    [Vec3::NEG_Y, Vec3::X, Vec3::NEG_Z],
    [Vec3::Z, Vec3::NEG_X, Vec3::Y],
    [Vec3::NEG_Z, Vec3::X, Vec3::Y],
];

fn frame_dir(frames: &[Frame; 6], face: usize, u: f32, v: f32) -> Vec3 {
    let [f, r, up] = frames[face];
    (f + r * (2.0 * u - 1.0) + up * (1.0 - 2.0 * v)).normalize()
}

/// 绝对值最大的轴挑面，再投到面上。
fn frame_uv(frames: &[Frame; 6], d: Vec3) -> (usize, f32, f32) {
    let a = d.abs();
    let face = if a.x >= a.y && a.x >= a.z {
        usize::from(d.x < 0.0)
    } else if a.y >= a.z {
        2 + usize::from(d.y < 0.0)
    } else {
        4 + usize::from(d.z < 0.0)
    };
    let [f, r, up] = frames[face];
    let p = d / d.dot(f);
    (face, (p.dot(r) + 1.0) * 0.5, (1.0 - p.dot(up)) * 0.5)
}

/// GPU 立方体第 `face` 层 (u, v) 处的方向（u、v 在 0–1，v 朝下）。
pub(crate) fn texel_dir(face: usize, u: f32, v: f32) -> Vec3 {
    frame_dir(&GPU, face, u, v)
}

/// 方向落在 GPU 立方体的哪一层哪里。
pub(crate) fn dir_face_uv(d: Vec3) -> (usize, f32, f32) {
    frame_uv(&GPU, d)
}

/// 方向落在用户的哪张图的哪里。
pub(crate) fn user_face_uv(d: Vec3) -> (usize, f32, f32) {
    frame_uv(&USER, d)
}

#[cfg(test)]
fn user_face_dir(face: usize, u: f32, v: f32) -> Vec3 {
    frame_dir(&USER, face, u, v)
}

/// 线性颜色的立方体，GPU 的层顺序，每面 `size × size`、一行一行从上往下。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Cube {
    pub size: u32,
    pub texels: Vec<Vec3>,
}

impl Cube {
    /// 每格按格子中心的方向取 `f`。
    pub fn from_fn(size: u32, f: impl Fn(Vec3) -> Vec3) -> Self {
        Self::from_fn_supersampled(size, 1, f)
    }

    /// 每格在格子里均匀取 `ss × ss` 个方向，平均 `f`（从大图缩小时不闪）。
    pub fn from_fn_supersampled(size: u32, ss: u32, f: impl Fn(Vec3) -> Vec3) -> Self {
        let n = size as usize;
        let ss = ss.max(1);
        let texels = (0..6 * n * n)
            .map(|i| {
                let (face, y, x) = (i / (n * n), (i / n) % n, i % n);
                let mut sum = Vec3::ZERO;
                for k in 0..ss * ss {
                    let (sx, sy) = ((k % ss) as f32 + 0.5, (k / ss) as f32 + 0.5);
                    let (u, v) = ((x as f32 + sx / ss as f32) / size as f32, (y as f32 + sy / ss as f32) / size as f32);
                    sum += f(texel_dir(face, u, v));
                }
                sum / (ss * ss) as f32
            })
            .collect();
        Self { size, texels }
    }

    fn at(&self, face: usize, x: u32, y: u32) -> Vec3 {
        let n = self.size as usize;
        self.texels[face * n * n + y as usize * n + x as usize]
    }

    /// 面内双线性，面的边上夹住（不跨面）。
    pub fn sample(&self, d: Vec3) -> Vec3 {
        let (face, u, v) = dir_face_uv(d);
        let s = self.size as f32;
        let p = Vec2::new(u * s - 0.5, v * s - 0.5).clamp(Vec2::ZERO, Vec2::splat(s - 1.0));
        let (x0, y0) = (p.x.floor() as u32, p.y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(self.size - 1), (y0 + 1).min(self.size - 1));
        let (fx, fy) = (p.x - x0 as f32, p.y - y0 as f32);
        let top = self.at(face, x0, y0).lerp(self.at(face, x1, y0), fx);
        let bottom = self.at(face, x0, y1).lerp(self.at(face, x1, y1), fx);
        top.lerp(bottom, fy)
    }

    /// 边长减半，2 × 2 平均。
    pub fn half(&self) -> Self {
        let size = (self.size / 2).max(1);
        if size == self.size {
            return self.clone();
        }
        let n = size as usize;
        let texels = (0..6 * n * n)
            .map(|i| {
                let (face, y, x) = (i / (n * n), (i / n) % n, i % n);
                let (x, y) = (2 * x as u32, 2 * y as u32);
                (self.at(face, x, y) + self.at(face, x + 1, y) + self.at(face, x, y + 1) + self.at(face, x + 1, y + 1)) * 0.25
            })
            .collect();
        Self { size, texels }
    }

    /// 每格的方向和立体角（总和是 4π）。
    fn texels_with_solid_angle(&self) -> impl Iterator<Item = (Vec3, Vec3, f32)> + '_ {
        let n = self.size as usize;
        let s = self.size as f32;
        self.texels.iter().enumerate().map(move |(i, &c)| {
            let (face, y, x) = (i / (n * n), (i / n) % n, i % n);
            let (u, v) = ((x as f32 + 0.5) / s, (y as f32 + 0.5) / s);
            let (a, b) = (2.0 * u - 1.0, 2.0 * v - 1.0);
            let w = (2.0 / s).powi(2) / (1.0 + a * a + b * b).powf(1.5);
            (texel_dir(face, u, v), c, w)
        })
    }
}

/// SH9 的 9 个基函数在方向 `d` 处的值。
pub(crate) fn sh_basis(d: Vec3) -> [f32; 9] {
    let (x, y, z) = (d.x, d.y, d.z);
    [
        0.282_095,
        0.488_603 * y,
        0.488_603 * z,
        0.488_603 * x,
        1.092_548 * x * y,
        1.092_548 * y * z,
        0.315_392 * (3.0 * z * z - 1.0),
        1.092_548 * x * z,
        0.546_274 * (x * x - y * y),
    ]
}

/// 余弦卷积的系数：0 阶 π，1 阶 2π/3，2 阶 π/4。
#[cfg(test)]
const SH_COSINE: [f32; 3] = [std::f32::consts::PI, 2.0 * std::f32::consts::PI / 3.0, std::f32::consts::PI / 4.0];

/// 把立方体投影到 SH9（辐射度），每格按立体角加权。
pub(crate) fn sh9(cube: &Cube) -> [Vec3; 9] {
    let mut sh = [Vec3::ZERO; 9];
    let mut total = 0.0;
    for (d, c, w) in cube.texels_with_solid_angle() {
        for (s, y) in sh.iter_mut().zip(sh_basis(d)) {
            *s += c * y * w;
        }
        total += w;
    }
    // 立体角的近似和 4π 差一点，按总和归一
    let scale = 4.0 * std::f32::consts::PI / total;
    sh.map(|s| s * scale)
}

/// SH 在方向 `d` 处的辐射度（模糊过的天空色）。着色器里 fog.wgsl 的 `sh_radiance` 是同一个公式，这份给测试对照。
#[cfg(test)]
fn sh9_radiance(sh: &[Vec3; 9], d: Vec3) -> Vec3 {
    sh.iter().zip(sh_basis(d)).map(|(s, y)| *s * y).sum()
}

/// 法线 `n` 处的辐照度（着色器里的 `sh_irradiance`）。
#[cfg(test)]
fn sh9_irradiance(sh: &[Vec3; 9], n: Vec3) -> Vec3 {
    let band = [0, 1, 1, 1, 2, 2, 2, 2, 2];
    sh.iter().zip(sh_basis(n)).zip(band).map(|((s, y), l)| *s * y * SH_COSINE[l]).sum()
}

/// Hammersley 低差异序列的第 `i` 个（共 `n` 个）。
fn hammersley(i: u32, n: u32) -> Vec2 {
    Vec2::new(i as f32 / n as f32, i.reverse_bits() as f32 * 2.328_306_4e-10)
}

/// GGX 重要性采样：切线空间（z 朝法线）里的半角向量。`a` 是 α = 粗糙度²。
fn ggx_half(xi: Vec2, a: f32) -> Vec3 {
    let phi = std::f32::consts::TAU * xi.x;
    let cos = ((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y)).max(0.0).sqrt();
    let sin = (1.0 - cos * cos).max(0.0).sqrt();
    Vec3::new(sin * phi.cos(), sin * phi.sin(), cos)
}

fn ggx_d(n_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = n_h * n_h * (a2 - 1.0) + 1.0;
    a2 / (std::f32::consts::PI * d * d)
}

/// 预滤波的 mip 链：第 0 级是 `base`，第 `m` 级粗糙度 `m / (levels - 1)`，按 GGX 重要性采样 `samples` 个方向。
pub(crate) fn prefilter(base: &Cube, levels: u32, samples: u32) -> Vec<Cube> {
    // 源图的 mip：按每个采样方向的 pdf 挑一级，粗糙时少噪点
    let mut mips = vec![base.clone()];
    while mips.last().expect("至少一级").size > 1 {
        let next = mips.last().expect("至少一级").half();
        mips.push(next);
    }
    let texel_angle = 4.0 * std::f32::consts::PI / (6.0 * (base.size * base.size) as f32);
    let xi: Vec<Vec2> = (0..samples).map(|i| hammersley(i, samples)).collect();
    let mut out = vec![base.clone()];
    for m in 1..levels.max(1) {
        let roughness = m as f32 / (levels - 1) as f32;
        let a = roughness * roughness;
        let size = (base.size >> m).max(1);
        out.push(Cube::from_fn(size, |n| {
            // 视线 = 法线 = 反射方向
            let (t, b) = n.any_orthonormal_pair();
            let (mut sum, mut weight) = (Vec3::ZERO, 0.0);
            for x in &xi {
                let h = ggx_half(*x, a);
                let h = t * h.x + b * h.y + n * h.z;
                let l = 2.0 * n.dot(h) * h - n;
                let n_l = n.dot(l);
                if n_l <= 0.0 {
                    continue;
                }
                // pdf = D × n·h / (4 v·h)，视线就是法线时 = D / 4
                let pdf = (ggx_d(n.dot(h).max(0.0), a) / 4.0).max(1e-6);
                let lod = (0.5 * (1.0 / (samples as f32 * pdf * texel_angle)).log2()).clamp(0.0, (mips.len() - 1) as f32);
                sum += mips[lod.round() as usize].sample(l) * n_l;
                weight += n_l;
            }
            if weight > 0.0 { sum / weight } else { base.sample(n) }
        }));
    }
    out
}

/// split-sum 的 (A, B)：镜面 = 预滤波 × (F0 × A + B)。
pub(crate) fn brdf(n_v: f32, roughness: f32, samples: u32) -> (f32, f32) {
    let n_v = n_v.clamp(1e-4, 1.0);
    let a = roughness * roughness;
    // 环境光照的 Smith G 用 k = α / 2
    let k = a / 2.0;
    let g1 = |c: f32| c / (c * (1.0 - k) + k);
    let v = Vec3::new((1.0 - n_v * n_v).sqrt(), 0.0, n_v);
    let (mut sa, mut sb) = (0.0, 0.0);
    for i in 0..samples {
        let h = ggx_half(hammersley(i, samples), a);
        let l = 2.0 * v.dot(h) * h - v;
        let (n_l, n_h, v_h) = (l.z, h.z.max(0.0), v.dot(h).max(0.0));
        if n_l <= 0.0 {
            continue;
        }
        let g_vis = g1(n_v) * g1(n_l) * v_h / (n_h * n_v).max(1e-6);
        let fc = (1.0 - v_h).powi(5);
        sa += (1.0 - fc) * g_vis;
        sb += fc * g_vis;
    }
    (sa / samples as f32, sb / samples as f32)
}

/// `size × size` 的 LUT（RGBA8，r = A、g = B），横轴 n·v、纵轴粗糙度，都取格子中心。
pub(crate) fn brdf_lut(size: u32, samples: u32) -> Vec<[u8; 4]> {
    let center = |i: u32| (i as f32 + 0.5) / size as f32;
    let unorm = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    (0..size * size)
        .map(|i| {
            let (a, b) = brdf(center(i % size), center(i / size), samples);
            [unorm(a), unorm(b), 0, 255]
        })
        .collect()
}

/// sRGB 8 位 → 线性。
pub(crate) fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// 线性 → sRGB 8 位（截到 0–1）。
pub(crate) fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> Vec<Vec3> {
        let mut seed = 7u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0
        };
        (0..300).filter_map(|_| Vec3::new(next(), next(), next()).try_normalize()).collect()
    }

    #[test]
    fn face_uv_and_direction_round_trip() {
        for d in dirs() {
            let (face, u, v) = dir_face_uv(d);
            assert!((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v), "{d} → {u} {v}");
            assert!(texel_dir(face, u, v).abs_diff_eq(d, 1e-4), "GPU {d}");
            let (face, u, v) = user_face_uv(d);
            assert!(user_face_dir(face, u, v).abs_diff_eq(d, 1e-4), "用户 {d}");
        }
        // 每个面中心是自己的轴
        for (i, axis) in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z].into_iter().enumerate() {
            assert_eq!(dir_face_uv(axis), (i, 0.5, 0.5));
            assert_eq!(user_face_uv(axis), (i, 0.5, 0.5));
        }
    }

    /// GPU 约定（WebGPU 规范的立方体采样）：几个已知的点。
    #[test]
    fn gpu_convention_matches_the_spec() {
        // +X 层：u 增大是 -Z，v 增大是 -Y
        let (f, u, v) = dir_face_uv(Vec3::new(1.0, 0.5, 0.5));
        assert_eq!(f, 0);
        assert!(u < 0.5 && v < 0.5, "{u} {v}");
        // +Y 层：u 增大是 +X，v 增大是 +Z
        let (f, u, v) = dir_face_uv(Vec3::new(0.5, 1.0, 0.5));
        assert_eq!(f, 2);
        assert!(u > 0.5 && v > 0.5, "{u} {v}");
        // -Z 层：u 增大是 -X
        let (f, u, _) = dir_face_uv(Vec3::new(0.5, 0.0, -1.0));
        assert_eq!(f, 5);
        assert!(u < 0.5);
    }

    /// 用户约定：朝某个方向看，右手边在图的右边、上方在图的上边。
    #[test]
    fn user_faces_are_what_you_see_looking_out() {
        // 朝 -Z 看：右边是 +X
        let (f, u, v) = user_face_uv(Vec3::new(0.5, 0.2, -1.0));
        assert_eq!(f, 5);
        assert!(u > 0.5 && v < 0.5, "右上 {u} {v}");
        // 朝 +X 看：右边是 +Z
        let (f, u, _) = user_face_uv(Vec3::new(1.0, 0.0, 0.5));
        assert_eq!(f, 0);
        assert!(u > 0.5);
        // 朝 +Z 看：右边是 -X
        let (f, u, _) = user_face_uv(Vec3::new(-0.5, 0.0, 1.0));
        assert_eq!(f, 4);
        assert!(u > 0.5);
        // 抬头：靠 -Z 的那边在图的下边；低头：靠 -Z 的在图的上边
        let (f, _, v) = user_face_uv(Vec3::new(0.0, 1.0, -0.5));
        assert_eq!(f, 2);
        assert!(v > 0.5);
        let (f, _, v) = user_face_uv(Vec3::new(0.0, -1.0, -0.5));
        assert_eq!(f, 3);
        assert!(v < 0.5);
    }

    #[test]
    fn cube_sample_and_half() {
        let cube = Cube::from_fn(8, |d| d.max(Vec3::ZERO));
        for d in dirs() {
            let s = cube.sample(d);
            assert!((s - d.max(Vec3::ZERO)).length() < 0.2, "{d}: {s}");
        }
        let half = cube.half();
        assert_eq!((half.size, half.texels.len()), (4, 6 * 16));
        let one = Cube::from_fn(1, |_| Vec3::ONE);
        assert_eq!(one.half(), one, "1 × 1 不再减半");
    }

    #[test]
    fn constant_sky_irradiance_is_pi_times_its_color() {
        let c = Vec3::new(0.2, 0.5, 1.0);
        let sh = sh9(&Cube::from_fn(16, |_| c));
        for n in dirs().into_iter().take(40) {
            let e = sh9_irradiance(&sh, n);
            assert!((e - c * std::f32::consts::PI).abs().max_element() < 0.01, "{n}: {e}");
            assert!((sh9_radiance(&sh, n) - c).abs().max_element() < 0.005);
        }
    }

    #[test]
    fn half_white_sky_lights_the_top_not_the_bottom() {
        let sh = sh9(&Cube::from_fn(32, |d| if d.y > 0.0 { Vec3::ONE } else { Vec3::ZERO }));
        let (up, down, side) = (sh9_irradiance(&sh, Vec3::Y).x, sh9_irradiance(&sh, Vec3::NEG_Y).x, sh9_irradiance(&sh, Vec3::X).x);
        assert!((up - std::f32::consts::PI).abs() < 0.12 * std::f32::consts::PI, "朝上 {up}");
        assert!(down.abs() < 0.12, "朝下 {down}");
        assert!((side - std::f32::consts::FRAC_PI_2).abs() < 0.1, "侧面一半 {side}");
        // 模糊过的天空色：上亮下暗
        assert!(sh9_radiance(&sh, Vec3::Y).x > 0.8 && sh9_radiance(&sh, Vec3::NEG_Y).x < 0.2);
    }

    #[test]
    fn prefiltered_levels_get_blurrier() {
        let base = Cube::from_fn(16, |d| if d.y > 0.7 { Vec3::ONE } else { Vec3::ZERO });
        let levels = prefilter(&base, 5, 32);
        assert_eq!(levels.len(), 5);
        assert_eq!(levels[0], base, "第 0 级是原图");
        assert_eq!(levels.iter().map(|c| c.size).collect::<Vec<_>>(), [16, 8, 4, 2, 1]);
        // 正上方：越粗糙越暗（亮斑被摊开）；侧面：越粗糙越亮
        let top: Vec<f32> = levels.iter().map(|c| c.sample(Vec3::Y).x).collect();
        // 最后两级只有 2 × 2、1 × 1，格子中心不在正上方，只比前面几级
        assert!(top[..4].windows(2).all(|w| w[1] <= w[0] + 1e-3), "{top:?}");
        assert!(top[4] < 0.6, "最粗糙的一级接近平均 {top:?}");
        let side = Vec3::new(1.0, 0.5, 0.0).normalize();
        assert!(levels[3].sample(side).x > levels[0].sample(side).x, "侧面被摊亮");
        // 常量天空每一级都不变
        for c in prefilter(&Cube::from_fn(8, |_| Vec3::splat(0.5)), 4, 16) {
            assert!(c.texels.iter().all(|t| (t.x - 0.5).abs() < 1e-3), "常量天空");
        }
    }

    /// Karis 的手机近似（EnvBRDFApprox）。
    fn approx(n_v: f32, r: f32) -> (f32, f32) {
        let c0 = [-1.0, -0.0275, -0.572, 0.022];
        let c1 = [1.0, 0.0425, 1.04, -0.04];
        let r4: [f32; 4] = std::array::from_fn(|i| r * c0[i] + c1[i]);
        let a004 = (r4[0] * r4[0]).min((-9.28 * n_v).exp2()) * r4[0] + r4[1];
        (-1.04 * a004 + r4[2], 1.04 * a004 + r4[3])
    }

    /// 不用重要性采样，在半球上按 θ、φ 的格子直接积分 D G F / (4 n·v)：A 是 (1 - Fc) 那部分，B 是 Fc 那部分。
    fn brute_force(n_v: f32, r: f32) -> (f32, f32) {
        let a = r * r;
        let k = a / 2.0;
        let g1 = |c: f32| c / (c * (1.0 - k) + k);
        let v = Vec3::new((1.0 - n_v * n_v).sqrt(), 0.0, n_v);
        let (nt, np) = (400, 400);
        let (dt, dp) = (std::f32::consts::FRAC_PI_2 / nt as f32, std::f32::consts::TAU / np as f32);
        let (mut sa, mut sb) = (0.0f64, 0.0f64);
        for i in 0..nt {
            let t = (i as f32 + 0.5) * dt;
            for j in 0..np {
                let p = (j as f32 + 0.5) * dp;
                let l = Vec3::new(t.sin() * p.cos(), t.sin() * p.sin(), t.cos());
                let h = (v + l).normalize();
                let f = ggx_d(h.z, a) * g1(n_v) * g1(l.z) / (4.0 * n_v) * t.sin() * dt * dp;
                let fc = (1.0 - v.dot(h)).powi(5);
                sa += ((1.0 - fc) * f) as f64;
                sb += (fc * f) as f64;
            }
        }
        (sa as f32, sb as f32)
    }

    #[test]
    fn brdf_lut_matches_the_known_shape() {
        let (a, b) = brdf(1.0, 0.02, 256);
        assert!(a > 0.95 && b < 0.03, "光滑、正对：几乎全是 F0 {a} {b}");
        // 近似公式在掠射角（n·v 很小）偏差大，那里只查范围
        assert!(brdf(0.1, 0.3, 256).0 > 0.2 && brdf(0.1, 0.3, 256).1 > 0.1);
        for n_v in [0.3, 0.5, 0.8, 1.0] {
            for r in [0.1, 0.3, 0.5, 0.8, 1.0] {
                let (a, b) = brdf(n_v, r, 256);
                assert!(a >= 0.0 && b >= 0.0 && a + b <= 1.0 + 1e-3, "{n_v} {r}: {a} {b}");
                // 粗糙度小时波瓣太尖，格子积分分辨不出来
                let (ea, eb) = if r >= 0.3 { brute_force(n_v, r) } else { (a, b) };
                assert!((a - ea).abs() < 0.03 && (b - eb).abs() < 0.03, "{n_v} {r}: {a} {b} 直接积分 {ea} {eb}");
                let (ka, kb) = approx(n_v, r);
                assert!((a - ka).abs() < 0.2 && (b - kb).abs() < 0.1, "{n_v} {r}: {a} {b} 近似 {ka} {kb}");
            }
        }
        // 越粗糙 A 越小（正对时）
        assert!(brdf(1.0, 0.9, 256).0 < brdf(1.0, 0.2, 256).0);
        let lut = brdf_lut(8, 64);
        assert_eq!(lut.len(), 64);
        let (a, b) = brdf(0.5 / 8.0 + 3.0 / 8.0, 5.5 / 8.0, 64);
        let px = lut[5 * 8 + 3];
        assert!(px[0].abs_diff((a * 255.0).round() as u8) <= 1 && px[1].abs_diff((b * 255.0).round() as u8) <= 1, "{px:?} {a} {b}");
    }

    #[test]
    fn srgb_round_trip() {
        for c in [0u8, 1, 10, 100, 128, 200, 255] {
            assert_eq!(linear_to_srgb(srgb_to_linear(c)), c);
        }
        assert_eq!(linear_to_srgb(3.0), 255);
    }
}
