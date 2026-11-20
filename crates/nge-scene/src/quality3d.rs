//! 3D 画质的场景节点（见 docs/render3d-quality.md）：`sky`、`fog`、`shadows`，逐字段宽松解析，声明变了才写进资源。

use bevy_ecs::prelude::*;
use glam::Vec3;
use nge_assets::{Assets, CubeImage};
use nge_render2d::TextureId;
use nge_render3d::{Fog, FogMode, HeightFog, ShadowFilter, Shadows, Sky, SkyGradient, SkySource, Ssao, ToneMap, ToneMapMode};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::lenient::{Fields, warn_once};
use crate::nodes::LightKind;
use crate::protocol::{Color, Scene};
use crate::resolve::SceneAssets;

/// 一张 KTX2 立方体贴图，或者 +X、-X、+Y、-Y、+Z、-Z 六张图。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum CubeNode {
    Ktx2(String),
    Faces([String; 6]),
}

/// `scene.sky`：写了 `cube` 用立方体贴图，否则是程序天空。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkyNode {
    pub cube: Option<CubeNode>,
    pub top: Option<Color>,
    pub horizon: Option<Color>,
    pub ground: Option<Color>,
    /// 阳光照射的方向；不写跟第一盏方向光，[0, 0, 0] 是没有太阳。
    pub sun: Option<[f32; 3]>,
    pub sun_color: Option<Color>,
    /// 太阳盘的半角（度）。
    pub sun_size: Option<f32>,
    pub diffuse: Option<f32>,
    pub specular: Option<f32>,
    pub background: Option<bool>,
}

impl<'de> Deserialize<'de> for SkyNode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut f = Fields::new("sky", Value::deserialize(d)?).map_err(serde::de::Error::custom)?;
        // cube 写错时不退成程序天空：整个 sky 当没写
        let cube = f.strict("cube").map_err(serde::de::Error::custom)?;
        let node = Self {
            cube,
            top: f.get("top"),
            horizon: f.get("horizon"),
            ground: f.get("ground"),
            sun: f.get("sun"),
            sun_color: f.get("sunColor"),
            sun_size: f.get("sunSize"),
            diffuse: f.get("diffuse"),
            specular: f.get("specular"),
            background: f.get("background"),
        };
        f.finish();
        Ok(node)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FogKind {
    None,
    Linear,
    Exp,
    Exp2,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default)]
pub struct HeightFogNode {
    pub base: f32,
    pub falloff: f32,
    pub density: f32,
}

impl Default for HeightFogNode {
    fn default() -> Self {
        Self { base: 0.0, falloff: 0.5, density: 0.05 }
    }
}

/// `scene.fog`：不写 `mode` 时，写了 `start` / `end` 是线性、写了 `density` 是指数，都没写只有高度雾。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FogNode {
    pub mode: Option<FogKind>,
    pub start: Option<f32>,
    pub end: Option<f32>,
    pub density: Option<f32>,
    pub color: Option<Color>,
    pub sky: Option<f32>,
    pub height: Option<HeightFogNode>,
    pub max: Option<f32>,
}

impl<'de> Deserialize<'de> for FogNode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut f = Fields::new("fog", Value::deserialize(d)?).map_err(serde::de::Error::custom)?;
        let node = Self {
            mode: f.get("mode"),
            start: f.get("start"),
            end: f.get("end"),
            density: f.get("density"),
            color: f.get("color"),
            sky: f.get("sky"),
            height: f.get("height"),
            max: f.get("max"),
        };
        f.finish();
        Ok(node)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterKind {
    Pcf,
    Poisson,
    Pcss,
}

/// `scene.shadows`：分几段、覆盖多远还在 `camera3d` 里写。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShadowsNode {
    pub filter: Option<FilterKind>,
    pub softness: Option<f32>,
    pub sun_size: Option<f32>,
    pub bias: Option<f32>,
    pub normal_bias: Option<f32>,
    pub max_lights: Option<u32>,
    pub blend: Option<f32>,
    pub debug_cascades: Option<bool>,
    pub size: Option<u32>,
    pub local_size: Option<u32>,
}

impl<'de> Deserialize<'de> for ShadowsNode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut f = Fields::new("shadows", Value::deserialize(d)?).map_err(serde::de::Error::custom)?;
        let node = Self {
            filter: f.get("filter"),
            softness: f.get("softness"),
            sun_size: f.get("sunSize"),
            bias: f.get("bias"),
            normal_bias: f.get("normalBias"),
            max_lights: f.get("maxLights"),
            blend: f.get("blend"),
            debug_cascades: f.get("debugCascades"),
            size: f.get("size"),
            local_size: f.get("localSize"),
        };
        f.finish();
        Ok(node)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToneMapKind {
    None,
    Aces,
    Agx,
    Reinhard,
}

/// `scene.tonemap`：3D 线性 HDR 色调映射。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToneMapNode {
    pub mode: Option<ToneMapKind>,
    pub exposure: Option<f32>,
    pub saturation: Option<f32>,
}

impl<'de> Deserialize<'de> for ToneMapNode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut f = Fields::new("tonemap", Value::deserialize(d)?).map_err(serde::de::Error::custom)?;
        let node = Self {
            mode: f.get("mode"),
            exposure: f.get("exposure"),
            saturation: f.get("saturation"),
        };
        f.finish();
        Ok(node)
    }
}

/// `scene.ssao`：3D 屏幕空间环境光遮蔽。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SsaoNode {
    pub enabled: Option<bool>,
    pub intensity: Option<f32>,
    pub radius: Option<f32>,
    pub bias: Option<f32>,
}

impl<'de> Deserialize<'de> for SsaoNode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut f = Fields::new("ssao", Value::deserialize(d)?).map_err(serde::de::Error::custom)?;
        let node = Self {
            enabled: f.get("enabled"),
            intensity: f.get("intensity"),
            radius: f.get("radius"),
            bias: f.get("bias"),
        };
        f.finish();
        Ok(node)
    }
}

fn rgb(color: Option<Color>, or: Vec3) -> Vec3 {
    color.map_or(or, |c| c.0.truncate())
}

/// 第一盏方向光照射的方向（程序天空的太阳默认跟它）。
fn first_sun(scene: &Scene) -> Option<[f32; 3]> {
    scene.lights.iter().find(|l| l.kind == LightKind::Directional).map(|l| l.direction)
}

fn sky(node: &SkyNode, sun: Option<[f32; 3]>, assets: &mut SceneAssets, cubes: &mut Assets<CubeImage>) -> Sky {
    let (d, g) = (Sky::default(), SkyGradient::default());
    let source = match &node.cube {
        Some(CubeNode::Ktx2(uri)) => SkySource::Cube(assets.cube(cubes, uri)),
        Some(CubeNode::Faces(names)) => {
            let mut faces = [TextureId::WHITE; 6];
            for (face, name) in faces.iter_mut().zip(names) {
                let Some(id) = assets.image(name) else {
                    warn_once(format!("sky.cube 引用了未注册的纹理 {name:?}，整个 sky 当没写"));
                    return Sky::default();
                };
                *face = id;
            }
            SkySource::Faces(faces)
        }
        None => SkySource::Gradient(SkyGradient {
            top: rgb(node.top, g.top),
            horizon: rgb(node.horizon, g.horizon),
            ground: rgb(node.ground, g.ground),
            sun: node.sun.or(sun).map_or(g.sun, Vec3::from),
            sun_color: rgb(node.sun_color, g.sun_color),
            sun_size: node.sun_size.map_or(g.sun_size, f32::to_radians),
        }),
    };
    Sky { source, diffuse: node.diffuse.unwrap_or(d.diffuse), specular: node.specular.unwrap_or(d.specular), background: node.background.unwrap_or(d.background) }
}

fn fog(node: &FogNode) -> Fog {
    let d = Fog::default();
    let kind = node.mode.unwrap_or(if node.start.is_some() || node.end.is_some() {
        FogKind::Linear
    } else if node.density.is_some() {
        FogKind::Exp
    } else {
        FogKind::None
    });
    let density = node.density.unwrap_or(0.02);
    let mode = match kind {
        FogKind::None => FogMode::None,
        FogKind::Linear => FogMode::Linear { start: node.start.unwrap_or(0.0), end: node.end.unwrap_or(100.0) },
        FogKind::Exp => FogMode::Exp { density },
        FogKind::Exp2 => FogMode::Exp2 { density },
    };
    let height = node.height.map(|h| HeightFog { base: h.base, falloff: h.falloff, density: h.density });
    Fog { mode, color: rgb(node.color, d.color), sky: node.sky.unwrap_or(d.sky), height, max: node.max.unwrap_or(d.max) }
}

/// 声明了的字段写进去；上次声明了、这次去掉的回到默认；其余留着 Rust 设的。
fn shadows(s: &mut Shadows, node: &ShadowsNode, before: &ShadowsNode) {
    let d = Shadows::default();
    macro_rules! field {
        ($($name:ident $(=> $map:expr)?),*) => {$(
            match (node.$name, before.$name) {
                (Some(v), _) => s.$name = field!(@map v $(, $map)?),
                (None, Some(_)) => s.$name = d.$name,
                (None, None) => {}
            }
        )*};
        (@map $v:ident) => { $v };
        (@map $v:ident, $map:expr) => { $map($v) };
    }
    field!(filter => filter, softness, sun_size, bias, normal_bias, max_lights, blend, debug_cascades, size, local_size);
}

fn filter(kind: FilterKind) -> ShadowFilter {
    match kind {
        FilterKind::Pcf => ShadowFilter::Pcf,
        FilterKind::Poisson => ShadowFilter::Poisson,
        FilterKind::Pcss => ShadowFilter::Pcss,
    }
}

type SkyKey = (Option<SkyNode>, Option<[f32; 3]>);

fn tonemap(node: &ToneMapNode) -> ToneMap {
    let d = ToneMap::default();
    let mode = match node.mode {
        Some(ToneMapKind::None) => ToneMapMode::None,
        Some(ToneMapKind::Aces) | None => ToneMapMode::Aces,
        Some(ToneMapKind::Agx) => ToneMapMode::Agx,
        Some(ToneMapKind::Reinhard) => ToneMapMode::Reinhard,
    };
    ToneMap { mode, exposure: node.exposure.unwrap_or(d.exposure), saturation: node.saturation.unwrap_or(d.saturation) }
}

fn ssao(node: &SsaoNode) -> Ssao {
    let d = Ssao::default();
    Ssao {
        enabled: node.enabled.unwrap_or(true),
        intensity: node.intensity.unwrap_or(d.intensity),
        radius: node.radius.unwrap_or(d.radius),
        bias: node.bias.unwrap_or(d.bias),
    }
}

/// 天空、雾、阴影、色调映射、SSAO 设置：各自的声明变了才覆盖。
#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_quality3d(
    scene: Res<Scene>,
    mut assets: SceneAssets,
    mut cubes: ResMut<Assets<CubeImage>>,
    mut sky_res: ResMut<Sky>,
    mut fog_res: ResMut<Fog>,
    mut shadows_res: ResMut<Shadows>,
    mut tonemap_res: ResMut<ToneMap>,
    mut ssao_res: ResMut<Ssao>,
    mut last_sky: Local<Option<SkyKey>>,
    mut last_fog: Local<Option<Option<FogNode>>>,
    mut last_shadows: Local<Option<Option<ShadowsNode>>>,
    mut last_tonemap: Local<Option<Option<ToneMapNode>>>,
    mut last_ssao: Local<Option<Option<SsaoNode>>>,
) {
    let key = (scene.sky.clone(), scene.sky.as_ref().filter(|s| s.sun.is_none() && s.cube.is_none()).and_then(|_| first_sun(&scene)));
    // 场景从没写过时不动：留着 Rust 设的（和 shadows 一样）
    if last_sky.as_ref() != Some(&key) {
        if last_sky.is_some() || key.0.is_some() {
            *sky_res = key.0.as_ref().map_or_else(Sky::default, |node| sky(node, key.1, &mut assets, &mut cubes));
        }
        *last_sky = Some(key);
    }
    if last_fog.as_ref() != Some(&scene.fog) {
        if last_fog.is_some() || scene.fog.is_some() {
            *fog_res = scene.fog.as_ref().map_or_else(Fog::default, fog);
        }
        *last_fog = Some(scene.fog.clone());
    }
    if last_shadows.as_ref() != Some(&scene.shadows) {
        let before = last_shadows.clone().flatten().unwrap_or_default();
        shadows(&mut shadows_res, &scene.shadows.clone().unwrap_or_default(), &before);
        *last_shadows = Some(scene.shadows.clone());
    }
    if last_tonemap.as_ref() != Some(&scene.tonemap) {
        if last_tonemap.is_some() || scene.tonemap.is_some() {
            *tonemap_res = scene.tonemap.as_ref().map_or_else(ToneMap::default, tonemap);
        }
        *last_tonemap = Some(scene.tonemap.clone());
    }
    if last_ssao.as_ref() != Some(&scene.ssao) {
        if last_ssao.is_some() || scene.ssao.is_some() {
            *ssao_res = scene.ssao.as_ref().map_or_else(Ssao::default, ssao);
        }
        *last_ssao = Some(scene.ssao.clone());
    }
}

#[cfg(test)]
mod tests {
    use nge_render3d::{ShadowPriority, SpotLight};
    use serde_json::json;

    use super::*;
    use crate::testing::{PNG, app, frame};

    #[test]
    fn a_wrong_field_drops_only_that_field() {
        let s: Scene = serde_json::from_value(json!({
            "sky": { "top": "#ff0000", "horizon": "红", "sunSize": "大", "diffuse": 0.5 },
            "fog": { "start": 5, "end": "远", "color": "#00ff00", "height": 3 },
            "shadows": { "filter": "blur", "softness": 4, "maxLights": -1, "debugCascades": true },
            "lights": [{ "id": "a", "kind": "spot", "shadows": true, "shadowPriority": "高" }, { "id": "b", "kind": "point", "shadowPriority": 2 }]
        }))
        .unwrap();
        let sky = s.sky.unwrap();
        assert_eq!((sky.top.map(|c| c.0.x), sky.horizon, sky.sun_size, sky.diffuse), (Some(1.0), None, None, Some(0.5)));
        let fog = s.fog.unwrap();
        assert_eq!((fog.start, fog.end, fog.height), (Some(5.0), None, None));
        assert!(fog.color.is_some());
        let sh = s.shadows.unwrap();
        assert_eq!((sh.filter, sh.softness, sh.max_lights, sh.debug_cascades), (None, Some(4.0), None, Some(true)));
        assert_eq!((s.lights.len(), s.lights[0].shadow_priority, s.lights[1].shadow_priority), (2, None, Some(2)), "灯整盏留着");
        // 整项不是对象：当没写，别的照常
        let s: Scene = serde_json::from_value(json!({ "sky": 1, "fog": [], "shadows": "soft", "ambient": { "brightness": 0.3 } })).unwrap();
        assert_eq!((s.sky, s.fog, s.shadows, s.ambient.map(|a| a.brightness)), (None, None, None, Some(0.3)));
    }

    #[test]
    fn fog_mode_follows_what_is_written() {
        let parse = |v: Value| fog(&serde_json::from_value(v).unwrap()).mode;
        assert_eq!(parse(json!({ "end": 40 })), FogMode::Linear { start: 0.0, end: 40.0 });
        assert_eq!(parse(json!({ "density": 0.1 })), FogMode::Exp { density: 0.1 });
        assert_eq!(parse(json!({ "mode": "exp2" })), FogMode::Exp2 { density: 0.02 });
        assert_eq!(parse(json!({ "height": {} })), FogMode::None);
        let f = fog(&serde_json::from_value(json!({ "height": { "density": 0.2 }, "sky": 0.5, "max": 0.8 })).unwrap());
        assert_eq!((f.height, f.sky, f.max), (Some(HeightFog { base: 0.0, falloff: 0.5, density: 0.2 }), 0.5, 0.8));
    }

    #[test]
    fn scene_writes_sky_fog_and_shadows_only_when_declared_changes() {
        let mut app = app();
        app.world_mut().resource_mut::<Shadows>().size = 1024;
        let scene = json!({
            "sky": { "top": "#000000", "sunSize": 3 },
            "fog": { "start": 10, "end": 50, "color": "#ffffff" },
            "shadows": { "filter": "pcss", "blend": 0.2 },
            "lights": [{ "id": "sun", "kind": "directional", "direction": [1, -1, 0] }]
        });
        frame(&mut app, Some(scene.clone()));
        let sky = app.world().resource::<Sky>().clone();
        let SkySource::Gradient(g) = sky.source else { panic!("程序天空 {sky:?}") };
        assert_eq!((g.top, g.sun), (Vec3::ZERO, Vec3::new(1.0, -1.0, 0.0)), "太阳跟着方向光");
        assert!((g.sun_size - 3f32.to_radians()).abs() < 1e-6);
        assert_eq!(app.world().resource::<Fog>().mode, FogMode::Linear { start: 10.0, end: 50.0 });
        let s = *app.world().resource::<Shadows>();
        assert_eq!((s.filter, s.blend, s.size), (ShadowFilter::Pcss, 0.2, 1024), "没写的留着 Rust 设的");

        // 声明没变：不覆盖 Rust 改的
        app.world_mut().resource_mut::<Shadows>().blend = 0.4;
        app.world_mut().resource_mut::<Fog>().max = 0.5;
        frame(&mut app, Some(scene));
        assert_eq!((app.world().resource::<Shadows>().blend, app.world().resource::<Fog>().max), (0.4, 0.5));

        // 去掉的字段回到默认，去掉的节点回到默认
        frame(&mut app, Some(json!({ "shadows": { "filter": "pcss" } })));
        let s = *app.world().resource::<Shadows>();
        assert_eq!((s.filter, s.blend, s.size), (ShadowFilter::Pcss, Shadows::default().blend, 1024));
        assert_eq!(*app.world().resource::<Sky>(), Sky::default());
        assert_eq!(*app.world().resource::<Fog>(), Fog::default());
    }

    #[test]
    fn cube_sky_loads_faces_or_a_ktx2() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sky": { "cube": [PNG, PNG, PNG, PNG, PNG, "plain"], "background": false } })));
        let sky = app.world().resource::<Sky>().clone();
        let SkySource::Faces(faces) = sky.source else { panic!("6 张图 {sky:?}") };
        assert_eq!((faces[0], sky.background), (faces[1], false), "同一张图只读一次");
        assert_ne!(faces[5], faces[0], "注册过的纹理名也行");
        frame(&mut app, Some(json!({ "sky": { "cube": "asset:/@t/sky.ktx2" } })));
        assert!(matches!(app.world().resource::<Sky>().source, SkySource::Cube(_)));
    }

    #[test]
    fn shadow_priority_goes_on_the_light() {
        let mut app = app();
        frame(&mut app, Some(json!({ "lights": [{ "id": "a", "kind": "spot", "shadows": true, "shadowPriority": 5 }] })));
        let world = app.world_mut();
        assert_eq!(world.query_filtered::<&ShadowPriority, With<SpotLight>>().single(world).unwrap().0, 5);
        frame(&mut app, Some(json!({ "lights": [{ "id": "a", "kind": "spot", "shadows": true }] })));
        let world = app.world_mut();
        assert_eq!(world.query::<&ShadowPriority>().iter(world).count(), 0, "去掉了就没有");
    }

    #[test]
    fn rust_sky_and_fog_survive_while_the_scene_never_declares_them() {
        let mut app = app();
        app.world_mut().insert_resource(Fog { mode: FogMode::Exp { density: 0.1 }, ..Default::default() });
        app.world_mut().insert_resource(Sky { source: SkySource::Gradient(SkyGradient::default()), ..Default::default() });
        frame(&mut app, Some(json!({ "sprites": [] })));
        assert_eq!(app.world().resource::<Fog>().mode, FogMode::Exp { density: 0.1 }, "场景没写 fog，留着 Rust 设的");
        assert!(matches!(app.world().resource::<Sky>().source, SkySource::Gradient(_)), "场景没写 sky，留着 Rust 设的");
        // 写过再去掉：回到默认
        frame(&mut app, Some(json!({ "fog": { "density": 0.3 }, "sky": {} })));
        frame(&mut app, Some(json!({ "sprites": [] })));
        assert_eq!((app.world().resource::<Fog>().mode, app.world().resource::<Sky>().source.clone()), (FogMode::None, SkySource::None));
    }

    #[test]
    fn a_bad_cube_drops_the_whole_sky() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sky": { "cube": ["a", "b"], "top": "#ff0000" } })));
        assert_eq!(app.world().resource::<Sky>().source, SkySource::None, "cube 写错（2 张）不退成程序天空");
        let s: Scene = serde_json::from_value(json!({ "sky": { "cube": 3 } })).unwrap();
        assert_eq!(s.sky, None);
        // 6 张里有一张找不到：整个天空当没写
        frame(&mut app, Some(json!({ "sky": { "cube": [PNG, PNG, PNG, PNG, PNG, "没有这张"] } })));
        assert_eq!(app.world().resource::<Sky>().source, SkySource::None);
    }

    #[test]
    fn unknown_fields_are_warned_once() {
        let _: Scene = serde_json::from_value(json!({ "fog": { "dennsity": 0.1 }, "shadows": { "maxLight": 2 }, "sky": { "tops": "#000000" } })).unwrap();
        for (node, key) in [("fog", "dennsity"), ("shadows", "maxLight"), ("sky", "tops")] {
            assert!(!crate::lenient::warn_once(format!("场景的 {node} 里没有 {key:?} 这个字段，忽略")), "{node}.{key} 已经 warn 过");
        }
    }
}
