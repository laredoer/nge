//! `scene` prop 的 JSON 协议（和 `ts/nge.tsx` 的类型一一对应）。

use bevy_ecs::prelude::Resource;
use glam::Vec4;
use nge_core::{EventKey, PropKey};
use serde::{Deserialize, Serialize};

use crate::anim::AnimNode;
use crate::audio::AudioNode;
use crate::effects::{LightingNode, MaskedNode, PostNode};
use crate::joints::JointNode;
use crate::nodes::{AmbientNode, Camera3dNode, EmitterNode, LightNode, ModelNode, TextNode};
use crate::physics::{BodyNode, PhysicsNode};
use crate::tilemap::TilemapNode;

pub const SCENE: PropKey<Scene> = PropKey::new("scene");
pub const TAP: EventKey<Tap> = EventKey::new("tap");

#[derive(Resource, Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
/// 每一项单独解析：写错的只丢那一项（见 `lenient`），不让整个场景失败。
pub struct Scene {
    #[serde(deserialize_with = "crate::lenient::background")]
    pub background: Option<Color>,
    #[serde(deserialize_with = "crate::lenient::camera")]
    pub camera: Option<CameraNode>,
    #[serde(deserialize_with = "crate::lenient::sprites")]
    pub sprites: Vec<SpriteNode>,
    #[serde(deserialize_with = "crate::lenient::tilemaps")]
    pub tilemaps: Vec<TilemapNode>,
    #[serde(deserialize_with = "crate::lenient::texts")]
    pub texts: Vec<TextNode>,
    #[serde(deserialize_with = "crate::lenient::emitters")]
    pub emitters: Vec<EmitterNode>,
    #[serde(deserialize_with = "crate::lenient::models")]
    pub models: Vec<ModelNode>,
    /// 3D 粒子发射器（见 docs/particles3d.md）。
    #[serde(deserialize_with = "crate::lenient::emitters3d")]
    pub emitters3d: Vec<crate::particles3d::Emitter3dNode>,
    /// 3D 贴地光环与贴花（古代法阵、范围指示器、脚底阴影）。
    #[serde(deserialize_with = "crate::lenient::decals3d")]
    pub decals3d: Vec<crate::decals3d::Decal3dNode>,
    #[serde(deserialize_with = "crate::lenient::camera3d")]
    pub camera3d: Option<Camera3dNode>,
    #[serde(deserialize_with = "crate::lenient::lights")]
    pub lights: Vec<LightNode>,
    #[serde(deserialize_with = "crate::lenient::ambient")]
    pub ambient: Option<AmbientNode>,
    /// 天空和环境光照（见 docs/render3d-quality.md）。
    #[serde(deserialize_with = "crate::lenient::sky")]
    pub sky: Option<crate::quality3d::SkyNode>,
    /// 雾。
    #[serde(deserialize_with = "crate::lenient::fog")]
    pub fog: Option<crate::quality3d::FogNode>,
    /// 阴影的过滤、偏移、投影灯数和级联过渡。
    #[serde(deserialize_with = "crate::lenient::shadows")]
    pub shadows: Option<crate::quality3d::ShadowsNode>,
    /// 3D 真实线性 HDR 色调映射（ACES / AgX / Reinhard）。
    #[serde(deserialize_with = "crate::lenient::tonemap")]
    pub tonemap: Option<crate::quality3d::ToneMapNode>,
    /// 3D 屏幕空间环境光遮蔽（SSAO）。
    #[serde(deserialize_with = "crate::lenient::ssao")]
    pub ssao: Option<crate::quality3d::SsaoNode>,
    #[serde(deserialize_with = "crate::lenient::physics")]
    pub physics: Option<PhysicsNode>,
    /// 3D 物理（见 docs/physics3d.md）。
    #[serde(deserialize_with = "crate::lenient::physics3d")]
    pub physics3d: Option<crate::physics3d::Physics3dNode>,
    /// 3D 导航网格（从地形模型与静态碰撞体自动构建）。
    #[serde(deserialize_with = "crate::lenient::navmesh3d")]
    pub navmesh3d: Option<crate::nav3d::NavMesh3dNode>,
    /// 进度条（见 docs/world-ui.md）。
    #[serde(deserialize_with = "crate::lenient::bars")]
    pub bars: Vec<crate::world_ui::BarNode>,
    /// 气泡。
    #[serde(deserialize_with = "crate::lenient::bubbles")]
    pub bubbles: Vec<crate::world_ui::BubbleNode>,
    /// 关节（见 docs/joints.md）。
    #[serde(deserialize_with = "crate::lenient::joints")]
    pub joints: Vec<JointNode>,
    /// 3D 关节（见 docs/gameplay3d.md）。
    #[serde(deserialize_with = "crate::lenient::joints3d")]
    pub joints3d: Vec<crate::joints3d::Joint3dNode>,
    /// 游戏写的着色器：名字 → WGSL（见 docs/effects2d.md）。
    #[serde(deserialize_with = "crate::lenient::shaders")]
    pub shaders: std::collections::HashMap<String, String>,
    /// 游戏写的 3D 材质着色器：名字 → WGSL（定义 `fn shade3d(s: Surface) -> vec4f`）。
    #[serde(deserialize_with = "crate::lenient::shaders")]
    pub shaders3d: std::collections::HashMap<String, String>,
    /// 2D 光照，写了就开。
    #[serde(deserialize_with = "crate::lenient::lighting")]
    pub lighting: Option<LightingNode>,
    /// 后处理，按顺序。
    #[serde(deserialize_with = "crate::lenient::post")]
    pub post: Vec<PostNode>,
    #[serde(deserialize_with = "crate::lenient::audio")]
    pub audio: Option<AudioNode>,
    /// 时间缩放，1 正常、0 暂停。
    #[serde(rename = "timeScale")]
    #[serde(deserialize_with = "crate::lenient::time_scale")]
    pub time_scale: Option<f32>,
    /// 左上角的性能面板、曲线、`perf` 事件（见 docs/perf-overlay.md、docs/big-scene-perf.md）。
    #[serde(deserialize_with = "crate::lenient::perf")]
    pub perf: Option<crate::scene_perf::PerfNode>,
    /// 关卡：换了值就整个重建场景（见 docs/levels.md）。数字和字符串都行。
    #[serde(deserialize_with = "crate::level::level_id")]
    pub level: Option<String>,
    /// 联网（见 docs/networking.md）：写了就连中继。
    #[serde(deserialize_with = "crate::lenient::net")]
    pub net: Option<crate::net::NetNode>,
    /// 预制体：名字 → 节点组（见 docs/scene-flow.md）。
    #[serde(deserialize_with = "crate::lenient::prefabs")]
    pub prefabs: std::collections::HashMap<String, serde_json::Value>,
    /// 预制体实例，展开后接在各列表后面。
    #[serde(deserialize_with = "crate::lenient::instances")]
    pub instances: Vec<crate::prefab::InstanceNode>,
    /// 换关过场。
    #[serde(deserialize_with = "crate::lenient::transition")]
    pub transition: Option<crate::transition::TransitionNode>,
    /// 按纹理名或 `asset:/` 路径设置的贴图选项。
    #[serde(deserialize_with = "crate::lenient::textures")]
    pub textures: std::collections::HashMap<String, TextureOptions>,
    /// 实例已经展开进各列表了。
    #[serde(skip)]
    pub(crate) expanded: bool,
}

/// `scene.textures` 的一项。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextureOptions {
    /// 不写按图片自己的（PNG / JPEG 生成，程序生成和压缩贴图不生成）。
    pub mipmaps: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct CameraNode {
    /// 跟随时是相对目标的偏移。
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
    /// 跟随的精灵 id。
    pub follow: Option<String>,
    /// 跟随的平滑时间（秒），0 是每帧对准。
    pub smooth: f32,
    #[serde(deserialize_with = "crate::lenient::bounds")]
    pub bounds: Option<Bounds>,
}

impl Default for CameraNode {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, zoom: 1.0, follow: None, smooth: 0.0, bounds: None }
    }
}

/// 相机能拍到的世界范围。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bounds {
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
    pub top: f32,
}

/// 一个精灵。角度用度（逆时针），`spin` 是度 / 秒。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SpriteNode {
    pub id: String,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub z: f32,
    #[serde(default)]
    pub rotation: f32,
    #[serde(default = "one")]
    pub scale: f32,
    pub w: Option<f32>,
    pub h: Option<f32>,
    pub color: Option<Color>,
    /// 注册过的纹理名，或 `asset:/` 路径（配 `grid` 当图集）。
    pub texture: Option<String>,
    pub grid: Option<[u32; 2]>,
    #[serde(default)]
    pub frame: u32,
    #[serde(default)]
    pub spin: f32,
    /// 帧动画；有它时 `frame` 不起作用。
    pub anim: Option<AnimNode>,
    /// 动画状态机（见 docs/anim-graph.md）；有它时 `anim`、`frame` 不起作用。
    #[serde(rename = "animGraph")]
    pub anim_graph: Option<crate::anim_graph::GraphJson>,
    /// 状态机的参数，和上一次声明不一样时才写进去。
    #[serde(rename = "animParams")]
    pub anim_params: Option<crate::anim_graph::AnimParams>,
    #[serde(default)]
    pub tap: bool,
    pub body: Option<BodyNode>,
    /// 视差：相机移动时跟着挪的比例，不填是 1（见 docs/parallax.md）。
    pub parallax: Option<ParallaxNode>,
    /// 沿哪个轴一个挨一个重复铺满视野（见 docs/repeat.md）。
    pub repeat: Option<RepeatNode>,
    /// 用着色器库里的哪个着色器画，`params` 交给它（不写用着色器的默认值）。
    pub shader: Option<String>,
    pub params: Option<[f32; 4]>,
    /// 当遮罩：不画出来，只记下不透明的地方。
    #[serde(default)]
    pub mask: bool,
    /// 只画在遮罩里或者遮罩外。
    pub masked: Option<MaskedNode>,
    /// 开着 2D 光照时不受光。
    #[serde(default)]
    pub unlit: bool,
    /// 法线图（和贴图同样排布），受光时按表面朝向明暗（见 docs/render2d-more.md）。
    pub normal: Option<String>,
    /// 挡住开了 shadows 的灯：`true` 是精灵的矩形，或者本地坐标的多边形。
    pub occluder: Option<crate::effects::OccluderNode>,
    /// 追某个节点（见 docs/ai.md）。
    pub chase: Option<crate::ai::ChaseNode>,
    /// 行为树（见 docs/gameplay2d.md）。写错时只丢这一项。
    #[serde(default, deserialize_with = "crate::lenient::behavior")]
    pub behavior: Option<crate::behavior::BehaviorNode>,
}

/// `repeat`：`"x"`、`"y"` 或 `"xy"`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatNode {
    X,
    Y,
    Xy,
}

impl From<RepeatNode> for nge_render2d::Repeat {
    fn from(node: RepeatNode) -> Self {
        Self { x: node != RepeatNode::Y, y: node != RepeatNode::X }
    }
}

/// `parallax`：一个数（x、y 一样），或者 `[x, y]`。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ParallaxNode {
    Both(f32),
    Axes([f32; 2]),
}

impl ParallaxNode {
    /// 转成组件；有坏值（NaN、无穷）时 warn 并按 1 处理（不加组件）。
    pub(crate) fn resolve(node: Option<Self>, id: &str) -> Option<nge_render2d::Parallax> {
        let v = match node? {
            Self::Both(p) => glam::Vec2::splat(p),
            Self::Axes(p) => glam::Vec2::from(p),
        };
        if !v.is_finite() {
            log::warn!("节点 {id:?} 的 parallax 不是有限的数，按 1 处理");
            return None;
        }
        Some(nge_render2d::Parallax(v))
    }
}

fn one() -> f32 {
    1.0
}

/// 点中了可点击的精灵或模型。x / y 是指针的 2D 世界坐标；点中模型时 `point` 是 3D 命中点。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tap {
    pub id: String,
    pub x: f32,
    pub y: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub point: Option<[f32; 3]>,
    /// 点中瓦片地图时：（列，行）和那一格的帧号（见 docs/gameplay2d.md）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell: Option<[i32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<i32>,
}

/// `"#rrggbb"` / `"#rrggbbaa"`（sRGB），解析后存线性 RGBA。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub struct Color(pub Vec4);

impl TryFrom<String> for Color {
    type Error = String;

    fn try_from(hex: String) -> Result<Self, String> {
        let bad = || format!("颜色 {hex:?} 不合法，要写成 #rrggbb 或 #rrggbbaa");
        let digits = hex.strip_prefix('#').filter(|d| matches!(d.len(), 6 | 8) && d.bytes().all(|b| b.is_ascii_hexdigit())).ok_or_else(bad)?;
        let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).map(|b| f32::from(b) / 255.0).map_err(|_| bad());
        let alpha = if digits.len() == 8 { byte(6)? } else { 1.0 };
        Ok(Self(Vec4::new(linear(byte(0)?), linear(byte(2)?), linear(byte(4)?), alpha)))
    }
}

fn linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn sprite_defaults() {
        let s: SpriteNode = serde_json::from_value(json!({ "id": "a" })).unwrap();
        assert_eq!((s.x, s.y, s.z, s.rotation, s.scale, s.frame, s.spin, s.tap), (0.0, 0.0, 0.0, 0.0, 1.0, 0, 0.0, false));
        assert_eq!((s.w, s.h, s.color, s.texture), (None, None, None, None));
    }

    #[test]
    fn sprite_requires_id() {
        assert!(serde_json::from_value::<SpriteNode>(json!({ "x": 1 })).is_err());
    }

    #[test]
    fn empty_scene() {
        let scene = SCENE.decode(&json!({})).unwrap();
        assert_eq!(scene, Scene::default());
    }

    #[test]
    fn hex_colors_are_linearized() {
        let c = Color::try_from("#ff000080".to_owned()).unwrap().0;
        assert_eq!((c.x, c.y, c.z), (1.0, 0.0, 0.0));
        assert!((c.w - 128.0 / 255.0).abs() < 1e-6, "alpha 不做 gamma");
        let gray = Color::try_from("#808080".to_owned()).unwrap().0;
        assert!((gray.x - 0.2158).abs() < 1e-3, "sRGB 0.5 → 线性约 0.216，实际 {}", gray.x);
        assert_eq!(gray.w, 1.0);
    }

    #[test]
    fn bad_colors_rejected() {
        for bad in ["red", "#12345", "#gggggg", "ff0000", "#ff00000", "#+f+f+f"] {
            assert!(Color::try_from(bad.to_owned()).is_err(), "{bad}");
        }
        let scene = SCENE.decode(&json!({ "background": "nope", "sprites": [{ "id": "a" }] })).expect("颜色写错只丢背景");
        assert_eq!((scene.background, scene.sprites.len()), (None, 1));
    }

    #[test]
    fn a_bad_item_only_drops_that_item() {
        let scene = SCENE
            .decode(&json!({
                "sprites": [{ "id": "a" }, { "x": 1 }, { "id": "c", "x": "far" }, { "id": "d" }],
                "models": [{ "id": "m", "model": "asset:/@g/a.glb" }, { "id": "bad", "model": "asset:/@g/a.glb", "body": { "kind": "rubber" } }],
                "joints": [{ "id": "j", "a": "a", "b": "d", "kind": "weld", "typo": 1 }],
                "texts": 5,
                "camera": { "x": 3, "bounds": { "left": 0, "right": 1, "bottom": 0, "top": 1, "front": 2 } },
                "lighting": { "lights": [{ "radius": 10 }, { "radius": 20, "glow": true }] },
                "post": [{ "shader": "s", "params": [1, 2] }, { "shader": "t" }],
                "shaders": { "s": "fn x() {}", "t": 3 },
                "timeScale": "fast",
                "physics": { "gravity": "down" },
            }))
            .expect("写错的项不让整个场景失败");
        let ids: Vec<&str> = scene.sprites.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["a", "d"], "没有 id、x 不是数的两个精灵跳过");
        assert_eq!(scene.models.len(), 1);
        assert!(scene.joints.is_empty() && scene.texts.is_empty());
        let camera = scene.camera.expect("相机还在");
        assert_eq!((camera.x, camera.bounds), (3.0, None), "只丢写错的 bounds");
        assert_eq!(scene.lighting.expect("光照还开着").lights.len(), 1);
        assert_eq!(scene.post.len(), 1);
        assert_eq!(scene.shaders.keys().collect::<Vec<_>>(), ["s"]);
        assert_eq!((scene.time_scale, scene.physics), (None, None));
    }

    #[test]
    fn the_same_warning_is_said_once() {
        assert!(crate::lenient::warn_once("场景测试：只说一次".into()));
        assert!(!crate::lenient::warn_once("场景测试：只说一次".into()));
    }
}
