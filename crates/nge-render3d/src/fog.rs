//! 雾（见 docs/render3d-quality.md）：距离雾（线性、指数、指数平方）和高度雾，颜色可以往天空色混。
//! 着色器里的 `fog.wgsl` 是同一个公式，网格、半透明网格和粒子方片共用。

use bevy_ecs::prelude::Resource;
use bytemuck::{Pod, Zeroable};
use glam::{Vec3, Vec4};

pub(crate) const FOG_WGSL: &str = include_str!("fog.wgsl");

/// 默认没有雾。
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct Fog {
    pub mode: FogMode,
    /// 线性颜色。
    pub color: Vec3,
    /// 0–1：颜色往天空色（按视线方向）混多少；没有天空时不混。
    pub sky: f32,
    pub height: Option<HeightFog>,
    /// 最浓多少（0–1）。
    pub max: f32,
}

impl Default for Fog {
    fn default() -> Self {
        Self { mode: FogMode::None, color: Vec3::new(0.6, 0.65, 0.7), sky: 0.0, height: None, max: 1.0 }
    }
}

/// 距离雾。距离是片元到相机的距离（正交相机是沿相机前方的距离）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FogMode {
    None,
    /// `start` 以内没有雾，到 `end` 最浓。
    Linear { start: f32, end: f32 },
    /// `1 - e^(-density·d)`。
    Exp { density: f32 },
    /// `1 - e^(-(density·d)²)`。
    Exp2 { density: f32 },
}

/// 高度雾：密度 `density × e^(-falloff × (高度 - base))`，越低越浓，沿视线积分。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeightFog {
    pub base: f32,
    pub falloff: f32,
    pub density: f32,
}

impl Fog {
    pub(crate) fn enabled(&self) -> bool {
        self.mode != FogMode::None || self.height.is_some_and(|h| h.density > 0.0)
    }

    /// 从高度 `from_y` 的相机看 `distance` 远、高度 `to_y` 的片元，雾有多浓（0–`max`）。
    pub fn amount(&self, distance: f32, from_y: f32, to_y: f32) -> f32 {
        let d = distance.max(0.0);
        let by_distance = match self.mode {
            FogMode::None => 0.0,
            FogMode::Linear { start, end } => {
                if end > start { ((d - start) / (end - start)).clamp(0.0, 1.0) } else { f32::from(d > start) }
            }
            FogMode::Exp { density } => 1.0 - (-density.max(0.0) * d).exp(),
            FogMode::Exp2 { density } => 1.0 - (-(density.max(0.0) * d).powi(2)).exp(),
        };
        let depth = self.height.map_or(0.0, |h| height_depth(h, d, from_y, to_y));
        ((1.0 - (1.0 - by_distance) * (-depth).exp()) * self.max.clamp(0.0, 1.0)).clamp(0.0, 1.0)
    }

    /// 打包给着色器。`camera` 是相机位置，`perspective` 为假时按沿 `forward` 的距离算。
    pub(crate) fn raw(&self, camera: Vec3, perspective: bool, forward: Vec3, has_sky: bool) -> FogRaw {
        let (mode, a, b) = match self.mode {
            FogMode::None => (0.0, 0.0, 0.0),
            FogMode::Linear { start, end } => (1.0, start, end),
            FogMode::Exp { density } => (2.0, density, 0.0),
            FogMode::Exp2 { density } => (3.0, density, 0.0),
        };
        let h = self.height.unwrap_or(HeightFog { base: 0.0, falloff: 0.0, density: 0.0 });
        FogRaw {
            color: self.color.extend(if has_sky { self.sky.clamp(0.0, 1.0) } else { 0.0 }).into(),
            params: [mode, a, b, if self.enabled() { 1.0 } else { 0.0 }],
            height: [h.base, h.falloff, h.density.max(0.0), self.max.clamp(0.0, 1.0)],
            camera: camera.extend(if perspective { 1.0 } else { 0.0 }).into(),
            forward: Vec4::from((forward, -forward.dot(camera))).into(),
        }
    }
}

/// 高度雾沿视线的光学厚度：∫ density × e^(-falloff (y - base))，y 从 `from_y` 线性走到 `to_y`。
fn height_depth(h: HeightFog, distance: f32, from_y: f32, to_y: f32) -> f32 {
    if h.density <= 0.0 {
        return 0.0;
    }
    let falloff = h.falloff.max(0.0);
    let start = h.density * (-falloff * (from_y - h.base)).exp();
    let x = falloff * (to_y - from_y);
    // (1 - e^(-x)) / x，x 很小时取极限 1
    let average = if x.abs() < 1e-4 { 1.0 - x * 0.5 } else { (1.0 - (-x).exp()) / x };
    start * average * distance
}

/// 和 `fog.wgsl` 里的 `Fog` 一一对应。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct FogRaw {
    /// rgb 雾色，w 往天空色混多少。
    pub color: [f32; 4],
    /// 模式（0 没有、1 线性、2 指数、3 指数平方）、两个参数（线性：起止；指数：密度）、w 开没开。
    pub params: [f32; 4],
    /// 高度雾的 base、falloff、density，w 最浓多少。
    pub height: [f32; 4],
    /// 相机位置，w = 1 透视（按距离）、0 正交（按沿前方的距离）。
    pub camera: [f32; 4],
    /// 相机前方：沿前方的距离 = dot(xyz, 位置) + w。
    pub forward: [f32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fog(mode: FogMode) -> Fog {
        Fog { mode, ..Default::default() }
    }

    #[test]
    fn linear_fog_starts_and_ends() {
        let f = fog(FogMode::Linear { start: 10.0, end: 30.0 });
        assert_eq!(f.amount(5.0, 0.0, 0.0), 0.0);
        assert_eq!(f.amount(10.0, 0.0, 0.0), 0.0);
        assert!((f.amount(20.0, 0.0, 0.0) - 0.5).abs() < 1e-6);
        assert_eq!(f.amount(30.0, 0.0, 0.0), 1.0);
        assert_eq!(f.amount(300.0, 0.0, 0.0), 1.0);
        // 起止一样：过了就满
        let hard = fog(FogMode::Linear { start: 10.0, end: 10.0 });
        assert_eq!((hard.amount(9.0, 0.0, 0.0), hard.amount(11.0, 0.0, 0.0)), (0.0, 1.0));
    }

    #[test]
    fn exponential_fogs_grow_with_distance() {
        let exp = fog(FogMode::Exp { density: 0.1 });
        let exp2 = fog(FogMode::Exp2 { density: 0.1 });
        assert!((exp.amount(10.0, 0.0, 0.0) - (1.0 - (-1.0f32).exp())).abs() < 1e-6);
        assert!((exp2.amount(10.0, 0.0, 0.0) - (1.0 - (-1.0f32).exp())).abs() < 1e-6, "density·d = 1 时两种一样");
        assert!(exp2.amount(3.0, 0.0, 0.0) < exp.amount(3.0, 0.0, 0.0), "近处指数平方更淡");
        let mut last = 0.0;
        for d in [0.0, 1.0, 5.0, 20.0, 100.0] {
            let a = exp.amount(d, 0.0, 0.0);
            assert!(a >= last && a <= 1.0, "{d}: {a}");
            last = a;
        }
        assert_eq!(fog(FogMode::None).amount(1e6, 0.0, 0.0), 0.0);
    }

    #[test]
    fn height_fog_is_thicker_low_and_continuous_when_level() {
        let f = Fog { height: Some(HeightFog { base: 0.0, falloff: 0.5, density: 0.2 }), ..Default::default() };
        assert!(f.enabled());
        let (low, high) = (f.amount(20.0, 0.0, 0.0), f.amount(20.0, 10.0, 10.0));
        assert!(low > high * 5.0, "低处浓 {low} 高处淡 {high}");
        // 从低处往上看比水平看淡：一路上越来越稀
        assert!(f.amount(20.0, 0.0, 10.0) < low);
        // 视线水平和几乎水平时连续
        let (level, tilted) = (f.amount(20.0, 1.0, 1.0), f.amount(20.0, 1.0, 1.0 + 1e-4));
        assert!((level - tilted).abs() < 1e-3, "{level} {tilted}");
        // 水平时的解析值：1 - e^(-密度 × e^(-falloff × h) × d)
        assert!((level - (1.0 - (-0.2 * (-0.5f32).exp() * 20.0).exp())).abs() < 1e-4);
        // 和距离雾叠起来：透光率相乘
        let both = Fog { mode: FogMode::Exp { density: 0.05 }, ..f };
        let expected = 1.0 - (1.0 - fog(FogMode::Exp { density: 0.05 }).amount(20.0, 0.0, 0.0)) * (1.0 - low);
        assert!((both.amount(20.0, 0.0, 0.0) - expected).abs() < 1e-5);
    }

    #[test]
    fn max_caps_the_fog() {
        let f = Fog { mode: FogMode::Exp { density: 1.0 }, max: 0.6, ..Default::default() };
        assert!((f.amount(100.0, 0.0, 0.0) - 0.6).abs() < 1e-6);
        assert!(!Fog::default().enabled() && !Fog { height: Some(HeightFog { base: 0.0, falloff: 1.0, density: 0.0 }), ..Default::default() }.enabled());
    }

    #[test]
    fn raw_packs_mode_and_sky_mix() {
        let f = Fog { mode: FogMode::Linear { start: 1.0, end: 2.0 }, sky: 0.7, ..Default::default() };
        let r = f.raw(Vec3::new(0.0, 5.0, 0.0), true, Vec3::NEG_Z, true);
        assert_eq!(r.params, [1.0, 1.0, 2.0, 1.0]);
        assert_eq!(r.color[3], 0.7);
        assert_eq!(r.camera, [0.0, 5.0, 0.0, 1.0]);
        assert_eq!(f.raw(Vec3::ZERO, false, Vec3::NEG_Z, false).color[3], 0.0, "没有天空不混");
        assert_eq!(size_of::<FogRaw>(), 80);
    }
}
