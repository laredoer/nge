//! 灯光组件，以及打包成着色器的 uniform。

use bevy_ecs::prelude::{Component, Resource};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};

use crate::fog::FogRaw;
use crate::shadow_soft::{Candidate, MAX_LOCAL_LAYERS};
use crate::shadow_views::MAX_CASCADES;

pub const MAX_DIRECTIONAL_LIGHTS: usize = 4;
pub const MAX_POINT_LIGHTS: usize = 8;
pub const MAX_SPOT_LIGHTS: usize = 4;
const MIN_OUTER_ANGLE: f32 = 1e-3;

/// 平行光。`direction` 是光的传播方向（从光源指向场景），不需要归一化。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DirectionalLight {
    pub direction: Vec3,
    /// 线性 RGB。
    pub color: Vec3,
    pub illuminance: f32,
    /// 投影子（见 [`Shadows`](crate::Shadows)）。只有第一盏开着的生效。
    pub shadows: bool,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self { direction: Vec3::new(-1.0, -2.0, -1.0), color: Vec3::ONE, illuminance: 3.0, shadows: false }
    }
}

/// 点光源，位置取同一实体的 `Transform3d`。亮度按距离平方衰减，到 `range` 处平滑降到 0。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct PointLight {
    pub color: Vec3,
    pub intensity: f32,
    pub range: f32,
    /// 投六个方向的影子（见 [`Shadows`](crate::Shadows)）：和聚光灯一起按 [`ShadowPriority`](crate::ShadowPriority) 挑，最多 `max_lights` 盏。
    pub shadows: bool,
}

impl Default for PointLight {
    fn default() -> Self {
        Self { color: Vec3::ONE, intensity: 10.0, range: 20.0, shadows: false }
    }
}

/// 聚光灯，位置和朝向取同一实体的 `Transform3d`，照向 `rotation × -Z`。
/// 衰减和点光源一样，再乘锥的系数：内锥里全亮，外锥外是黑，中间平滑过渡。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct SpotLight {
    pub color: Vec3,
    pub intensity: f32,
    pub range: f32,
    /// 半角（弧度），不超过外锥。
    pub inner_angle: f32,
    /// 半角（弧度），限制在 (0, 89°]。
    pub outer_angle: f32,
    /// 投影子（见 [`Shadows`](crate::Shadows)）：和点光源一起按 [`ShadowPriority`](crate::ShadowPriority) 挑，最多 `max_lights` 盏。
    pub shadows: bool,
}

impl Default for SpotLight {
    fn default() -> Self {
        Self { color: Vec3::ONE, intensity: 10.0, range: 20.0, inner_angle: 20f32.to_radians(), outer_angle: 30f32.to_radians(), shadows: false }
    }
}

impl SpotLight {
    /// 限制以后的（内锥, 外锥）。
    pub(crate) fn cone(&self) -> (f32, f32) {
        let outer = if self.outer_angle > 0.0 { self.outer_angle.clamp(MIN_OUTER_ANGLE, 89f32.to_radians()) } else { MIN_OUTER_ANGLE };
        (self.inner_angle.max(0.0).min(outer), outer)
    }
}

/// 环境光：`albedo × color × brightness`，没有 IBL。
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct AmbientLight {
    pub color: Vec3,
    pub brightness: f32,
}

impl Default for AmbientLight {
    fn default() -> Self {
        Self { color: Vec3::ONE, brightness: 0.1 }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct DirRaw {
    /// 指向光源的单位向量。
    pub to_light: [f32; 4],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct PointRaw {
    /// xyz 位置，w 是 range。
    pub position_range: [f32; 4],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct SpotRaw {
    /// xyz 位置，w 是 range。
    pub position_range: [f32; 4],
    /// xyz 照射方向（单位向量），w 是外锥的 cos。
    pub direction: [f32; 4],
    /// rgb 预乘了亮度，w 是内锥的 cos。
    pub color: [f32; 4],
}

/// 和着色器里的 `Globals` 一一对应。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct Globals {
    pub view_proj: [[f32; 4]; 4],
    /// 画天空时从屏幕反推视线。
    pub inv_view_proj: [[f32; 4]; 4],
    /// 视点：w = 1 是相机位置，w = 0 是正交相机朝向相机的方向（[`Camera3d::eye`](crate::Camera3d::eye)）。
    pub eye: [f32; 4],
    pub ambient: [f32; 4],
    /// 方向光数量、点光源数量、投影子的方向光（第几盏 + 1，0 是没有）、聚光灯数量。
    pub counts: [u32; 4],
    /// 阴影过滤（0 PCF、1 泊松、2 PCSS）、级联调试色（1 开）、方向光阴影的段数、画天空（0 不画、1 程序天空、2 立方体）。
    pub casters: [u32; 4],
    pub dirs: [DirRaw; MAX_DIRECTIONAL_LIGHTS],
    pub points: [PointRaw; MAX_POINT_LIGHTS],
    pub spots: [SpotRaw; MAX_SPOT_LIGHTS],
    /// 方向光每段：世界 → 阴影图的裁剪空间。
    pub cascades: [[[f32; 4]; 4]; MAX_CASCADES],
    /// 聚光灯（1 层）、点光源（6 层：+X、-X、+Y、-Y、+Z、-Z）：世界 → 阴影图的裁剪空间。
    pub locals: [[[f32; 4]; 4]; MAX_LOCAL_LAYERS],
    /// 相机前方：xyz 单位向量，w = -dot(前方, 相机位置)，片元在相机前方的距离 = dot(前方, 位置) + w。
    pub view_forward: [f32; 4],
    /// 每段的远端（相机前方的距离）。
    pub splits: [f32; 4],
    /// 每段法线方向往外推多少（世界单位）。
    pub cascade_offset: [f32; 4],
    /// 方向光、聚光灯和点光源的阴影图一个像素在 uv 里多大；w：点光源离灯 1 处往外推多少。
    pub shadow: [f32; 4],
    /// 每盏聚光灯的阴影从第几层开始 + 1（0 是不投影）。
    pub spot_layers: [u32; MAX_SPOT_LIGHTS],
    /// 每盏点光源的阴影从第几层开始 + 1。
    pub point_layers: [u32; MAX_POINT_LIGHTS],
    /// 每盏聚光灯离灯 1 处往外推多少。
    pub spot_offset: [f32; MAX_SPOT_LIGHTS],
    /// 泊松盘半径（像素）、级联混合比例、深度 bias、PCSS 最大半影（像素）。
    pub soft: [f32; 4],
    /// PCSS：每段深度每差 1 半影宽几个像素。
    pub pcss: [f32; 4],
    /// 环境漫反射强度、镜面强度、预滤波最高一级、有没有环境光照（1 有）。
    pub env: [f32; 4],
    /// 天空的 SH9（辐射度，rgb）。
    pub sh: [[f32; 4]; 9],
    /// 程序天空：上、地平线、下（rgb）；指向太阳（xyz，w 是太阳盘半角）；太阳色（w：1 有太阳）。
    pub sky: [[f32; 4]; 5],
    pub fog: FogRaw,
}

impl Globals {
    /// 超出上限的灯丢弃，返回值的第二项表示有没有丢。开了阴影的聚光灯、点光源记进 `candidates`（再按优先级挑，见 `shadow_soft::pick`）。
    #[allow(clippy::too_many_arguments)]
    pub fn new<'a>(
        view_proj: Mat4,
        eye: Vec4,
        ambient: AmbientLight,
        dirs: impl IntoIterator<Item = &'a DirectionalLight>,
        points: impl IntoIterator<Item = (&'a PointLight, Vec3, i32)>,
        spots: impl IntoIterator<Item = (&'a SpotLight, Vec3, Vec3, i32)>,
        candidates: &mut Vec<Candidate>,
    ) -> (Self, bool) {
        let mut g = Self::zeroed();
        g.view_proj = view_proj.to_cols_array_2d();
        g.inv_view_proj = view_proj.inverse().to_cols_array_2d();
        g.eye = eye.into();
        g.ambient = (ambient.color * ambient.brightness).extend(0.0).into();
        candidates.clear();
        // 正交相机没有位置：同优先级按查询顺序
        let distance = |p: Vec3| if eye.w == 0.0 { 0.0 } else { p.distance(eye.truncate()) };
        let mut dropped = false;
        let mut n = 0;
        for light in dirs {
            let Some(to_light) = (-light.direction).try_normalize() else { continue };
            if n == MAX_DIRECTIONAL_LIGHTS {
                dropped = true;
                break;
            }
            g.dirs[n] = DirRaw { to_light: to_light.extend(0.0).into(), color: (light.color * light.illuminance).extend(0.0).into() };
            if light.shadows && g.counts[2] == 0 {
                g.counts[2] = n as u32 + 1;
            }
            n += 1;
        }
        g.counts[0] = n as u32;
        n = 0;
        for (light, position, priority) in points {
            if n == MAX_POINT_LIGHTS {
                dropped = true;
                break;
            }
            g.points[n] = PointRaw {
                position_range: position.extend(light.range).into(),
                color: (light.color * light.intensity).extend(0.0).into(),
            };
            if light.shadows {
                candidates.push(Candidate { point: true, index: n, priority, distance: distance(position) });
            }
            n += 1;
        }
        g.counts[1] = n as u32;
        n = 0;
        for (light, position, direction, priority) in spots {
            let Some(direction) = direction.try_normalize() else { continue };
            if n == MAX_SPOT_LIGHTS {
                dropped = true;
                break;
            }
            let (inner, outer) = light.cone();
            g.spots[n] = SpotRaw {
                position_range: position.extend(light.range).into(),
                direction: direction.extend(outer.cos()).into(),
                color: (light.color * light.intensity).extend(inner.cos()).into(),
            };
            if light.shadows {
                candidates.push(Candidate { point: false, index: n, priority, distance: distance(position) });
            }
            n += 1;
        }
        g.counts[3] = n as u32;
        (g, dropped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack<'a>(
        dirs: impl IntoIterator<Item = &'a DirectionalLight>,
        points: impl IntoIterator<Item = (&'a PointLight, Vec3)>,
        spots: impl IntoIterator<Item = (&'a SpotLight, Vec3, Vec3)>,
    ) -> (Globals, bool) {
        pack_with(dirs, points, spots, &mut Vec::new())
    }

    fn pack_with<'a>(
        dirs: impl IntoIterator<Item = &'a DirectionalLight>,
        points: impl IntoIterator<Item = (&'a PointLight, Vec3)>,
        spots: impl IntoIterator<Item = (&'a SpotLight, Vec3, Vec3)>,
        candidates: &mut Vec<Candidate>,
    ) -> (Globals, bool) {
        let points = points.into_iter().map(|(l, p)| (l, p, 0));
        let spots = spots.into_iter().map(|(l, p, d)| (l, p, d, 0));
        Globals::new(Mat4::IDENTITY, Vec4::W, AmbientLight::default(), dirs, points, spots, candidates)
    }

    #[test]
    fn packs_lights_with_premultiplied_color() {
        let dir = DirectionalLight { direction: Vec3::new(0.0, -2.0, 0.0), color: Vec3::new(1.0, 0.5, 0.0), illuminance: 2.0, shadows: false };
        let point = PointLight { color: Vec3::ONE, intensity: 3.0, range: 7.0, shadows: false };
        let ambient = AmbientLight { color: Vec3::new(0.5, 0.5, 1.0), brightness: 0.2 };
        let (g, dropped) = Globals::new(Mat4::IDENTITY, Vec4::new(1.0, 2.0, 3.0, 1.0), ambient, [&dir], [(&point, Vec3::X, 0)], [], &mut Vec::new());
        assert!(!dropped);
        assert_eq!(g.counts, [1, 1, 0, 0]);
        assert_eq!(g.eye, [1.0, 2.0, 3.0, 1.0]);
        assert_eq!(g.ambient, [0.1, 0.1, 0.2, 0.0]);
        assert_eq!(g.dirs[0].to_light, [0.0, 1.0, 0.0, 0.0], "存的是指向光源的单位向量");
        assert_eq!(g.dirs[0].color, [2.0, 1.0, 0.0, 0.0]);
        assert_eq!(g.points[0].position_range, [1.0, 0.0, 0.0, 7.0]);
        assert_eq!(g.points[0].color, [3.0, 3.0, 3.0, 0.0]);
    }

    #[test]
    fn packs_spot_lights_with_their_cone() {
        let spot = SpotLight { color: Vec3::new(1.0, 0.5, 0.0), intensity: 4.0, range: 6.0, inner_angle: 0.2, outer_angle: 0.5, shadows: false };
        let (g, dropped) = pack([], [], [(&spot, Vec3::new(1.0, 2.0, 3.0), Vec3::new(0.0, -2.0, 0.0))]);
        assert!(!dropped);
        assert_eq!(g.counts[3], 1, "第四个计数是聚光灯");
        let s = g.spots[0];
        assert_eq!(s.position_range, [1.0, 2.0, 3.0, 6.0]);
        assert_eq!(s.direction[..3], [0.0, -1.0, 0.0], "照射方向归一化");
        assert!((s.direction[3] - 0.5f32.cos()).abs() < 1e-6, "w 是外锥的 cos");
        assert_eq!(s.color[..3], [4.0, 2.0, 0.0]);
        assert!((s.color[3] - 0.2f32.cos()).abs() < 1e-6, "w 是内锥的 cos");
    }

    #[test]
    fn spot_cone_is_clamped() {
        let cone = |inner: f32, outer: f32| SpotLight { inner_angle: inner, outer_angle: outer, ..Default::default() }.cone();
        let max = 89f32.to_radians();
        assert_eq!(cone(0.1, 2.0), (0.1, max), "外锥最多 89°");
        assert_eq!(cone(0.6, 0.4), (0.4, 0.4), "内锥不超过外锥");
        assert_eq!(cone(-1.0, 0.4), (0.0, 0.4));
        for bad in [0.0, -1.0, f32::NAN] {
            let (inner, outer) = cone(0.1, bad);
            assert!(outer > 0.0 && outer < 0.01 && inner <= outer, "{bad} → {outer}");
        }
        let d = SpotLight::default();
        assert!(d.inner_angle < d.outer_angle && d.cone() == (d.inner_angle, d.outer_angle) && !d.shadows);
    }

    #[test]
    fn extra_lights_are_dropped() {
        let dir = DirectionalLight::default();
        let point = PointLight::default();
        let (g, dropped) = pack([&dir; 5], [(&point, Vec3::ZERO); 9], []);
        assert!(dropped);
        assert_eq!(g.counts[..2], [MAX_DIRECTIONAL_LIGHTS as u32, MAX_POINT_LIGHTS as u32]);
        let spot = SpotLight::default();
        let (g, dropped) = pack([], [], [(&spot, Vec3::ZERO, Vec3::NEG_Z); 5]);
        assert!(dropped);
        assert_eq!(g.counts[3], MAX_SPOT_LIGHTS as u32);
        let (_, dropped) = pack([], [], [(&spot, Vec3::ZERO, Vec3::NEG_Z); 4]);
        assert!(!dropped);
    }

    #[test]
    fn zero_direction_light_is_skipped() {
        let dir = DirectionalLight { direction: Vec3::ZERO, ..Default::default() };
        let spot = SpotLight::default();
        let (g, _) = pack([&dir], [], [(&spot, Vec3::ZERO, Vec3::ZERO)]);
        assert_eq!(g.counts[0], 0);
        assert_eq!(g.counts[3], 0);
    }

    #[test]
    fn layout_matches_shader() {
        let lights = 32 * MAX_DIRECTIONAL_LIGHTS + 32 * MAX_POINT_LIGHTS + 48 * MAX_SPOT_LIGHTS;
        let shadows = 64 * (MAX_CASCADES + MAX_LOCAL_LAYERS) + 16 * 4 + 4 * (MAX_SPOT_LIGHTS + MAX_POINT_LIGHTS + MAX_SPOT_LIGHTS) + 16 * 2;
        assert_eq!(size_of::<Globals>(), 64 * 2 + 16 * 4 + lights + shadows + 16 + 16 * 9 + 16 * 5 + size_of::<FogRaw>());
        assert_eq!(size_of::<Globals>() % 16, 0);
    }

    #[test]
    fn first_shadowed_light_casts_and_zero_ones_do_not_count() {
        let plain = DirectionalLight::default();
        let zero = DirectionalLight { direction: Vec3::ZERO, shadows: true, ..Default::default() };
        let shadowed = DirectionalLight { shadows: true, ..Default::default() };
        let (g, _) = pack([&plain, &zero, &shadowed, &shadowed], [], []);
        assert_eq!(g.counts[..3], [3, 0, 2], "第二盏（排掉方向是 0 的以后）投影子");
        let (g, _) = pack([&plain], [], []);
        assert_eq!(g.counts[2], 0);
    }

    #[test]
    fn every_shadowed_spot_and_point_is_a_candidate() {
        let (spot, lit_spot) = (SpotLight::default(), SpotLight { shadows: true, ..Default::default() });
        let (point, lit_point) = (PointLight::default(), PointLight { shadows: true, ..Default::default() });
        let mut candidates = Vec::new();
        let (g, _) = pack_with(
            [],
            [(&point, Vec3::ZERO), (&lit_point, Vec3::X), (&lit_point, Vec3::Y)],
            [(&spot, Vec3::ZERO, Vec3::ZERO), (&lit_spot, Vec3::ZERO, Vec3::ZERO), (&spot, Vec3::ZERO, Vec3::NEG_Z), (&lit_spot, Vec3::Z, Vec3::NEG_Z)],
            &mut candidates,
        );
        let got: Vec<_> = candidates.iter().map(|c| (c.point, c.index)).collect();
        assert_eq!(got, [(true, 1), (true, 2), (false, 1)], "方向是 0 的聚光灯不算，下标按打包以后的");
        assert_eq!(g.counts[3], 2);
        // 透视相机按到相机的距离，正交相机都是 0
        let p = [(&lit_point, Vec3::new(3.0, 4.0, 0.0), 5)];
        Globals::new(Mat4::IDENTITY, Vec4::W, AmbientLight::default(), [], p, [], &mut candidates);
        assert_eq!((candidates[0].distance, candidates[0].priority), (5.0, 5));
        Globals::new(Mat4::IDENTITY, Vec4::Z, AmbientLight::default(), [], p, [], &mut candidates);
        assert_eq!(candidates[0].distance, 0.0);
        assert!(!PointLight::default().shadows);
    }
}
