//! `scene` 里精灵之外的节点：文字、粒子、3D 模型、相机、灯光，以及一次性命令（和 `ts/nge.tsx` 一一对应）。

use bevy_ecs::prelude::Message;
use nge_core::CommandKey;
use serde::Deserialize;

use crate::protocol::Color;

pub const BURST: CommandKey<Burst> = CommandKey::new("burst");
pub const FLOAT_TEXT: CommandKey<FloatText> = CommandKey::new("floatText");

/// 文字的哪一点对准 (x, y)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AnchorName {
    TopLeft,
    Top,
    TopRight,
    Left,
    #[default]
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl From<AnchorName> for nge_text::Anchor {
    fn from(a: AnchorName) -> Self {
        use nge_text::Anchor::*;
        match a {
            AnchorName::TopLeft => TopLeft,
            AnchorName::Top => Top,
            AnchorName::TopRight => TopRight,
            AnchorName::Left => Left,
            AnchorName::Center => Center,
            AnchorName::Right => Right,
            AnchorName::BottomLeft => BottomLeft,
            AnchorName::Bottom => Bottom,
            AnchorName::BottomRight => BottomRight,
        }
    }
}

/// 一段文字。`follow` 填模型 id 时跟着模型投影到屏幕的位置走，x / y 变成屏幕上的偏移（y 朝上）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextNode {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default = "default_text_size")]
    pub size: f32,
    pub color: Option<Color>,
    #[serde(default)]
    pub anchor: AnchorName,
    /// `asset:/` 路径；不填用系统字体。
    pub font: Option<String>,
    /// x / y 是屏幕坐标（左上角原点，y 朝下），不跟相机走。
    #[serde(default)]
    pub screen: bool,
    pub max_width: Option<f32>,
    pub follow: Option<String>,
    /// 跟随模型时往上抬的 3D 距离。
    #[serde(default)]
    pub lift: f32,
}

fn default_text_size() -> f32 {
    16.0
}

/// 粒子参数（持续发射的 `emitters[]` 和一次性的 `burst` 共用）。范围写成 `[最小, 最大]`，角度用度。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Particles {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub lifetime: [f32; 2],
    pub speed: [f32; 2],
    pub angle: [f32; 2],
    pub gravity: [f32; 2],
    pub size: [f32; 2],
    /// [出生, 死亡]；不填是白色淡出。
    pub color: Option<[Color; 2]>,
    /// `asset:/` 路径或注册过的纹理名；不填是方块。
    pub texture: Option<String>,
    pub max: Option<usize>,
}

impl Default for Particles {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            lifetime: [1.0, 1.0],
            speed: [50.0, 100.0],
            angle: [0.0, 360.0],
            gravity: [0.0, 0.0],
            size: [8.0, 0.0],
            color: None,
            texture: None,
            max: None,
        }
    }
}

/// 一直发射的粒子发射器。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EmitterNode {
    pub id: String,
    /// 每秒发射多少个。
    #[serde(default)]
    pub rate: f32,
    #[serde(flatten)]
    pub particles: Particles,
}

/// 一个 glTF 模型。`rotation` 是度，按 Y、X、Z 的顺序转；`spin` 绕 y 轴，度 / 秒。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelNode {
    pub id: String,
    pub model: String,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub z: f32,
    #[serde(default)]
    pub rotation: [f32; 3],
    #[serde(default = "one")]
    pub scale: f32,
    #[serde(default)]
    pub spin: f32,
    #[serde(default)]
    pub tap: bool,
}

fn one() -> f32 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Camera3dNode {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub target: [f32; 3],
    /// 垂直视角（度）。
    pub fov: f32,
}

impl Default for Camera3dNode {
    fn default() -> Self {
        Self { x: 0.0, y: 2.0, z: 6.0, target: [0.0; 3], fov: 60.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LightKind {
    Directional,
    Point,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LightNode {
    pub id: String,
    pub kind: LightKind,
    /// 方向光照射的方向。
    #[serde(default = "down")]
    pub direction: [f32; 3],
    /// 点光源的位置。
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub z: f32,
    pub color: Option<Color>,
    /// 方向光的照度 / 点光源的强度；不填用引擎默认值。
    pub intensity: Option<f32>,
    pub range: Option<f32>,
}

fn down() -> [f32; 3] {
    [0.0, -1.0, 0.0]
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct AmbientNode {
    pub color: Option<Color>,
    pub brightness: f32,
}

impl Default for AmbientNode {
    fn default() -> Self {
        Self { color: None, brightness: 0.1 }
    }
}

/// 命令 `burst`：炸一团粒子，死完自动删除。
#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
pub struct Burst {
    #[serde(default = "default_count")]
    pub count: u32,
    #[serde(flatten)]
    pub particles: Particles,
}

fn default_count() -> u32 {
    100
}

/// 命令 `floatText`：在世界坐标 (x, y) 飘出一行字，往上飘、渐隐。
#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct FloatText {
    pub x: f32,
    pub y: f32,
    pub text: String,
    pub size: f32,
    pub color: Option<Color>,
    pub font: Option<String>,
    /// 秒。
    pub duration: f32,
    /// 每秒往上飘多少 point。
    pub rise: f32,
}

impl Default for FloatText {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, text: String::new(), size: 24.0, color: None, font: None, duration: 1.0, rise: 60.0 }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn text_defaults_and_camel_case() {
        let t: TextNode = serde_json::from_value(json!({ "id": "a", "text": "hi", "maxWidth": 80, "anchor": "top-left" })).unwrap();
        assert_eq!((t.size, t.anchor, t.max_width, t.screen, t.lift), (16.0, AnchorName::TopLeft, Some(80.0), false, 0.0));
        assert!(serde_json::from_value::<TextNode>(json!({ "id": "a" })).is_err(), "text 必填");
    }

    #[test]
    fn emitter_flattens_particle_fields() {
        let e: EmitterNode = serde_json::from_value(json!({ "id": "smoke", "rate": 20, "x": 5, "angle": [80, 100], "color": ["#ffffff", "#00000000"] })).unwrap();
        assert_eq!((e.rate, e.particles.x, e.particles.angle, e.particles.lifetime), (20.0, 5.0, [80.0, 100.0], [1.0, 1.0]));
        assert_eq!(e.particles.color.unwrap()[1].0.w, 0.0);
    }

    #[test]
    fn models_lights_and_commands() {
        let m: ModelNode = serde_json::from_value(json!({ "id": "box", "model": "asset:/@g/a.glb", "rotation": [0, 90, 0] })).unwrap();
        assert_eq!((m.scale, m.rotation, m.tap), (1.0, [0.0, 90.0, 0.0], false));
        let l: LightNode = serde_json::from_value(json!({ "id": "sun", "kind": "directional" })).unwrap();
        assert_eq!((l.kind, l.direction), (LightKind::Directional, [0.0, -1.0, 0.0]));
        assert!(serde_json::from_value::<LightNode>(json!({ "id": "x", "kind": "spot" })).is_err());
        let b = BURST.decode(&json!({ "x": 3, "speed": [0, 0] })).unwrap();
        assert_eq!((b.count, b.particles.x, b.particles.speed), (100, 3.0, [0.0, 0.0]));
        let f = FLOAT_TEXT.decode(&json!({ "text": "+10" })).unwrap();
        assert_eq!((f.text.as_str(), f.duration, f.size), ("+10", 1.0, 24.0));
    }
}
