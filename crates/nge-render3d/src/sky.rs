//! 天空（见 docs/render3d-quality.md）：立方体贴图（6 张图或一张 KTX2）或程序天空，画在最远处、给环境光照烘焙用。

use bevy_ecs::prelude::Resource;
use glam::Vec3;
use nge_assets::{CubeImage, Handle, ImageId};

/// 天空和环境光照。默认没有天空：不画、没有环境光照（只有 [`AmbientLight`](crate::AmbientLight)）。
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct Sky {
    pub source: SkySource,
    /// 环境漫反射强度。
    pub diffuse: f32,
    /// 环境镜面反射强度。
    pub specular: f32,
    /// 画天空；关掉时只拿来照明。
    pub background: bool,
}

impl Default for Sky {
    fn default() -> Self {
        Self { source: SkySource::None, diffuse: 1.0, specular: 1.0, background: true }
    }
}

/// 天空从哪来。立方体的面按 +X、-X、+Y、-Y、+Z、-Z，每张是从里面朝那个方向看到的画面（不镜像，侧面上方是 +Y；
/// 抬头那张的下边接 -Z 面的上边，低头那张的上边接 -Z 面的下边）。
#[derive(Debug, Clone, PartialEq)]
pub enum SkySource {
    None,
    Gradient(SkyGradient),
    /// 6 张图，一样大的正方形。
    Faces([ImageId; 6]),
    /// 一张 KTX2 立方体贴图。
    Cube(Handle<CubeImage>),
}

/// 程序天空：上、地平线、下三色渐变加太阳。颜色都是线性的。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyGradient {
    pub top: Vec3,
    pub horizon: Vec3,
    pub ground: Vec3,
    /// 阳光照射的方向（和 [`DirectionalLight::direction`](crate::DirectionalLight) 一样，从太阳指向场景）；0 是没有太阳。
    pub sun: Vec3,
    pub sun_color: Vec3,
    /// 太阳盘的半角（弧度）。
    pub sun_size: f32,
}

impl Default for SkyGradient {
    fn default() -> Self {
        Self {
            top: Vec3::new(0.15, 0.3, 0.65),
            horizon: Vec3::new(0.6, 0.7, 0.8),
            ground: Vec3::new(0.25, 0.22, 0.2),
            sun: Vec3::new(-1.0, -2.0, -1.0),
            sun_color: Vec3::new(1.0, 0.95, 0.8),
            sun_size: 1.5f32.to_radians(),
        }
    }
}

/// 太阳盘外面的光晕：`cos^64 × 0.25`。
pub(crate) const SUN_GLOW: (f32, f32) = (64.0, 0.25);

impl SkyGradient {
    /// 方向 `d`（单位向量）上天空的颜色；着色器里 `sky_gradient` 是同一个公式。
    pub fn radiance(&self, d: Vec3) -> Vec3 {
        let base = if d.y >= 0.0 { self.horizon.lerp(self.top, d.y.min(1.0).sqrt()) } else { self.horizon.lerp(self.ground, (-d.y).min(1.0).sqrt()) };
        let Some(to_sun) = (-self.sun).try_normalize() else { return base };
        let c = d.dot(to_sun);
        let size = self.sun_size.max(1e-4);
        let disk = smoothstep((size * 1.2).cos(), size.cos(), c);
        let glow = c.max(0.0).powf(SUN_GLOW.0) * SUN_GLOW.1;
        base + self.sun_color * (disk + glow)
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_blends_top_horizon_and_ground() {
        let g = SkyGradient { sun: Vec3::ZERO, ..Default::default() };
        assert!(g.radiance(Vec3::Y).abs_diff_eq(g.top, 1e-5), "正上方");
        assert!(g.radiance(Vec3::X).abs_diff_eq(g.horizon, 1e-5), "地平线");
        assert!(g.radiance(Vec3::NEG_Y).abs_diff_eq(g.ground, 1e-5), "正下方");
        let mid = g.radiance(Vec3::new(1.0, 1.0, 0.0).normalize());
        assert!(mid.z > g.horizon.z.min(g.top.z) - 1e-5 && mid.z < g.horizon.z.max(g.top.z) + 1e-5, "中间 {mid}");
    }

    #[test]
    fn the_sun_sits_against_its_light_direction() {
        let g = SkyGradient::default();
        let to_sun = -g.sun.normalize();
        let disk = g.radiance(to_sun);
        assert!((disk - g.sun_color).min_element() > 0.0, "太阳盘整个加上太阳色 {disk}");
        let off = g.radiance((to_sun + Vec3::new(0.0, 0.0, 0.3)).normalize());
        assert!(off.x < disk.x - 0.5, "离开太阳盘就暗下去 {off} vs {disk}");
        let away = g.radiance(-to_sun);
        let plain = SkyGradient { sun: Vec3::ZERO, ..g }.radiance(-to_sun);
        assert!(away.abs_diff_eq(plain, 1e-5), "背着太阳没有光晕");
    }

    #[test]
    fn default_sky_draws_nothing() {
        let s = Sky::default();
        assert_eq!((s.source.clone(), s.diffuse, s.specular, s.background), (SkySource::None, 1.0, 1.0, true));
    }
}
