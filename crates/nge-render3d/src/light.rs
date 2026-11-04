//! 灯光组件，以及打包成着色器的 uniform。

use bevy_ecs::prelude::{Component, Resource};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

pub const MAX_DIRECTIONAL_LIGHTS: usize = 4;
pub const MAX_POINT_LIGHTS: usize = 8;

/// 平行光。`direction` 是光的传播方向（从光源指向场景），不需要归一化。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DirectionalLight {
    pub direction: Vec3,
    /// 线性 RGB。
    pub color: Vec3,
    pub illuminance: f32,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self { direction: Vec3::new(-1.0, -2.0, -1.0), color: Vec3::ONE, illuminance: 3.0 }
    }
}

/// 点光源，位置取同一实体的 `Transform3d`。亮度按距离平方衰减，到 `range` 处平滑降到 0。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct PointLight {
    pub color: Vec3,
    pub intensity: f32,
    pub range: f32,
}

impl Default for PointLight {
    fn default() -> Self {
        Self { color: Vec3::ONE, intensity: 10.0, range: 20.0 }
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

/// 和着色器里的 `Globals` 一一对应。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct Globals {
    pub view_proj: [[f32; 4]; 4],
    pub camera_pos: [f32; 4],
    pub ambient: [f32; 4],
    /// 方向光数量、点光源数量。
    pub counts: [u32; 4],
    pub dirs: [DirRaw; MAX_DIRECTIONAL_LIGHTS],
    pub points: [PointRaw; MAX_POINT_LIGHTS],
}

impl Globals {
    /// 超出上限的灯丢弃，返回值的第二项表示有没有丢。
    pub fn new<'a>(
        view_proj: Mat4,
        camera_pos: Vec3,
        ambient: AmbientLight,
        dirs: impl IntoIterator<Item = &'a DirectionalLight>,
        points: impl IntoIterator<Item = (&'a PointLight, Vec3)>,
    ) -> (Self, bool) {
        let mut g = Self::zeroed();
        g.view_proj = view_proj.to_cols_array_2d();
        g.camera_pos = camera_pos.extend(1.0).into();
        g.ambient = (ambient.color * ambient.brightness).extend(0.0).into();
        let mut dropped = false;
        let mut n = 0;
        for light in dirs {
            let Some(to_light) = (-light.direction).try_normalize() else { continue };
            if n == MAX_DIRECTIONAL_LIGHTS {
                dropped = true;
                break;
            }
            g.dirs[n] = DirRaw { to_light: to_light.extend(0.0).into(), color: (light.color * light.illuminance).extend(0.0).into() };
            n += 1;
        }
        g.counts[0] = n as u32;
        n = 0;
        for (light, position) in points {
            if n == MAX_POINT_LIGHTS {
                dropped = true;
                break;
            }
            g.points[n] = PointRaw {
                position_range: position.extend(light.range).into(),
                color: (light.color * light.intensity).extend(0.0).into(),
            };
            n += 1;
        }
        g.counts[1] = n as u32;
        (g, dropped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_lights_with_premultiplied_color() {
        let dir = DirectionalLight { direction: Vec3::new(0.0, -2.0, 0.0), color: Vec3::new(1.0, 0.5, 0.0), illuminance: 2.0 };
        let point = PointLight { color: Vec3::ONE, intensity: 3.0, range: 7.0 };
        let ambient = AmbientLight { color: Vec3::new(0.5, 0.5, 1.0), brightness: 0.2 };
        let (g, dropped) = Globals::new(Mat4::IDENTITY, Vec3::new(1.0, 2.0, 3.0), ambient, [&dir], [(&point, Vec3::X)]);
        assert!(!dropped);
        assert_eq!(g.counts, [1, 1, 0, 0]);
        assert_eq!(g.camera_pos, [1.0, 2.0, 3.0, 1.0]);
        assert_eq!(g.ambient, [0.1, 0.1, 0.2, 0.0]);
        assert_eq!(g.dirs[0].to_light, [0.0, 1.0, 0.0, 0.0], "存的是指向光源的单位向量");
        assert_eq!(g.dirs[0].color, [2.0, 1.0, 0.0, 0.0]);
        assert_eq!(g.points[0].position_range, [1.0, 0.0, 0.0, 7.0]);
        assert_eq!(g.points[0].color, [3.0, 3.0, 3.0, 0.0]);
    }

    #[test]
    fn extra_lights_are_dropped() {
        let dir = DirectionalLight::default();
        let point = PointLight::default();
        let (g, dropped) = Globals::new(Mat4::IDENTITY, Vec3::ZERO, AmbientLight::default(), [&dir; 5], [(&point, Vec3::ZERO); 9]);
        assert!(dropped);
        assert_eq!(g.counts[..2], [MAX_DIRECTIONAL_LIGHTS as u32, MAX_POINT_LIGHTS as u32]);
    }

    #[test]
    fn zero_direction_light_is_skipped() {
        let dir = DirectionalLight { direction: Vec3::ZERO, ..Default::default() };
        let (g, _) = Globals::new(Mat4::IDENTITY, Vec3::ZERO, AmbientLight::default(), [&dir], []);
        assert_eq!(g.counts[0], 0);
    }

    #[test]
    fn layout_matches_shader() {
        assert_eq!(size_of::<Globals>(), 64 + 16 * 3 + 32 * MAX_DIRECTIONAL_LIGHTS + 32 * MAX_POINT_LIGHTS);
    }
}
