//! `scene` prop 的 JSON 协议（和 `ts/nge.tsx` 的类型一一对应）。

use bevy_ecs::prelude::Resource;
use glam::Vec4;
use nge_core::{EventKey, PropKey};
use serde::{Deserialize, Serialize};

pub const SCENE: PropKey<Scene> = PropKey::new("scene");
pub const TAP: EventKey<Tap> = EventKey::new("tap");

#[derive(Resource, Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Scene {
    pub background: Option<Color>,
    pub camera: Option<CameraNode>,
    pub sprites: Vec<SpriteNode>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct CameraNode {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

impl Default for CameraNode {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, zoom: 1.0 }
    }
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
    pub texture: Option<String>,
    #[serde(default)]
    pub frame: u32,
    #[serde(default)]
    pub spin: f32,
    #[serde(default)]
    pub tap: bool,
}

fn one() -> f32 {
    1.0
}

/// 点中了可点击的精灵。x / y 是世界坐标。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tap {
    pub id: String,
    pub x: f32,
    pub y: f32,
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
        assert!(SCENE.decode(&json!({ "background": "nope" })).is_err());
    }
}
