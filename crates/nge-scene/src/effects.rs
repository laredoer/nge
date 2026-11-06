//! 场景里的 2D 特效：着色器库、精灵的着色器和遮罩、2D 光照、后处理（见 docs/effects2d.md）。

use bevy_ecs::prelude::*;
use glam::{Vec2, Vec3, Vec4};
use nge_render2d::{Camera2d, DrawAt, Light2d, Parallax, Lighting2d, Masking, PostEffect, PostEffects, Shaders, SpriteShader, Transform2d, Unlit, drawn};
use serde::Deserialize;

use crate::protocol::{Color, Scene, SpriteNode};
use crate::sync::{Declared, SceneId};

/// 精灵的 `masked`：只画在遮罩里还是遮罩外。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaskedNode {
    Inside,
    Outside,
}

/// `scene.lighting`：写了就开 2D 光照。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightingNode {
    /// 环境光，默认 "#ffffff"（不变暗）。
    pub ambient: Option<Color>,
    #[serde(default)]
    pub lights: Vec<Light2dNode>,
}

/// 2D 点光源。`follow` 时 x、y 是相对那个精灵的偏移。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Light2dNode {
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    pub radius: f32,
    pub color: Option<Color>,
    #[serde(default = "one")]
    pub intensity: f32,
    pub follow: Option<String>,
}

/// `scene.post` 的一项。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostNode {
    pub shader: String,
    pub params: Option<[f32; 4]>,
}

fn one() -> f32 {
    1.0
}

/// 参数不写时用着色器的默认值。
fn params(shader: &str, params: Option<[f32; 4]>) -> Vec4 {
    params.map_or_else(|| Shaders::default_params(shader), Vec4::from)
}

/// 精灵节点上的 `shader`、`params`、`mask`、`masked`、`unlit`。
pub(crate) fn apply_sprite(entity: &mut EntityCommands, node: &SpriteNode) {
    match &node.shader {
        Some(name) => entity.insert(SpriteShader { name: name.clone(), params: params(name, node.params) }),
        None => {
            if node.params.is_some() {
                log::warn!("节点 {:?} 写了 params 没写 shader：params 不起作用", node.id);
            }
            entity.remove::<SpriteShader>()
        }
    };
    if node.mask && node.masked.is_some() {
        log::warn!("节点 {:?} 同时写了 mask 和 masked：当遮罩用，masked 不起作用", node.id);
    }
    let masking = match (node.mask, node.masked) {
        (true, _) => Some(Masking::Write),
        (false, Some(MaskedNode::Inside)) => Some(Masking::Inside),
        (false, Some(MaskedNode::Outside)) => Some(Masking::Outside),
        (false, None) => None,
    };
    match masking {
        Some(m) => entity.insert(m),
        None => entity.remove::<Masking>(),
    };
    if node.unlit {
        entity.insert(Unlit);
    } else {
        entity.remove::<Unlit>();
    }
}

/// `scene.shaders` 和 `scene.post`（场景变了才跑）。
pub(crate) fn sync_effects(scene: Res<Scene>, mut shaders: ResMut<Shaders>, mut post: ResMut<PostEffects>) {
    // 一样的不算改，不会重新编译
    shaders.set(scene.shaders.clone());
    let effects: Vec<PostEffect> = scene.post.iter().map(|p| PostEffect { shader: p.shader.clone(), params: params(&p.shader, p.params) }).collect();
    if post.0 != effects {
        post.0 = effects;
    }
}

/// 每帧按 `scene.lighting` 摆灯：跟着精灵的灯放在精灵画的位置（有刚体时是插值过的，带视差时是挪过的）。精灵不存在时用 x、y 当世界坐标。
#[allow(clippy::type_complexity)]
pub(crate) fn lighting(
    scene: Res<Scene>,
    cameras: Query<&Camera2d>,
    sprites: Query<(&SceneId, &Transform2d, Option<&DrawAt>, Option<&Parallax>), With<Declared<SpriteNode>>>,
    mut lighting: ResMut<Lighting2d>,
) {
    let Some(node) = &scene.lighting else {
        if *lighting != Lighting2d::default() {
            *lighting = Lighting2d::default();
        }
        return;
    };
    let camera = cameras.iter().next().copied().unwrap_or_default();
    let at = |id: &str| sprites.iter().find(|(s, ..)| s.0 == id).map(|(_, t, d, p)| drawn(t, d, p, &camera).translation);
    let lights = node
        .lights
        .iter()
        .map(|l| {
            let offset = Vec2::new(l.x, l.y);
            Light2d {
                position: l.follow.as_deref().and_then(at).map_or(offset, |p| p + offset),
                radius: l.radius,
                color: l.color.map_or(Vec3::ONE, |c| c.0.truncate()),
                intensity: l.intensity,
            }
        })
        .collect();
    let next = Lighting2d { enabled: true, ambient: node.ambient.map_or(Vec3::ONE, |c| c.0.truncate()), lights };
    if *lighting != next {
        *lighting = next;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::{app, frame};

    fn sprite_effects(app: &mut nge_ecs::App, id: &str) -> (Option<SpriteShader>, Option<Masking>, bool) {
        let world = app.world_mut();
        world
            .query::<(&SceneId, Option<&SpriteShader>, Option<&Masking>, Has<Unlit>)>()
            .iter(world)
            .find(|(s, ..)| s.0 == id)
            .map(|(_, s, m, u)| (s.cloned(), m.copied(), u))
            .unwrap()
    }

    #[test]
    fn sprite_shader_mask_and_unlit_are_declared_and_removed() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "w": 10, "shader": "flash", "mask": true, "unlit": true }] })));
        let flash = SpriteShader { name: "flash".into(), params: Shaders::default_params("flash") };
        assert_eq!(sprite_effects(&mut app, "a"), (Some(flash), Some(Masking::Write), true), "不写 params 用默认的");
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "w": 10, "shader": "mine", "params": [1, 2, 3, 4], "masked": "inside" }] })));
        let mine = SpriteShader { name: "mine".into(), params: Vec4::new(1.0, 2.0, 3.0, 4.0) };
        assert_eq!(sprite_effects(&mut app, "a"), (Some(mine), Some(Masking::Inside), false));
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "w": 10, "masked": "outside" }] })));
        assert_eq!(sprite_effects(&mut app, "a"), (None, Some(Masking::Outside), false));
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "w": 10, "mask": true, "masked": "outside" }] })));
        assert_eq!(sprite_effects(&mut app, "a").1, Some(Masking::Write), "两个都写时当遮罩");
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "w": 10 }] })));
        assert_eq!(sprite_effects(&mut app, "a"), (None, None, false), "去掉就回到原样");
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "w": 10, "masked": "inside" }] })));
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "w": 10, "masked": "middle", "unlit": true }] })));
        assert_eq!(sprite_effects(&mut app, "a"), (None, Some(Masking::Inside), false), "写错的值整份场景解析失败，保持上一份");
    }

    #[test]
    fn shaders_and_post_effects_follow_the_scene() {
        let mut app = app();
        let code = "fn effect(e: Effect) -> vec4f { return e.color; }";
        frame(&mut app, Some(json!({ "shaders": { "plain": code }, "post": [{ "shader": "vignette" }, { "shader": "plain", "params": [1, 0, 0, 0] }] })));
        let world = app.world();
        assert_eq!(world.resource::<Shaders>().code("plain"), Some(code));
        let generation = world.resource::<Shaders>().generation();
        assert_eq!(
            world.resource::<PostEffects>().0,
            vec![
                PostEffect { shader: "vignette".into(), params: Shaders::default_params("vignette") },
                PostEffect { shader: "plain".into(), params: Vec4::X },
            ]
        );
        // 别的东西变了、着色器没变：不算新一代（不重新编译）
        frame(&mut app, Some(json!({ "shaders": { "plain": code }, "sprites": [{ "id": "a", "w": 1 }] })));
        let world = app.world();
        assert_eq!(world.resource::<Shaders>().generation(), generation);
        assert!(world.resource::<PostEffects>().0.is_empty(), "去掉 post 就不做后处理");
        frame(&mut app, Some(json!({})));
        assert_eq!(app.world().resource::<Shaders>().code("plain"), None);
    }

    #[test]
    fn lights_follow_sprites_and_turn_off_with_the_scene() {
        let mut app = app();
        let scene = |follow: &str| {
            json!({
                "sprites": [{ "id": "hero", "x": 50, "y": 10, "w": 10 }],
                "lighting": { "ambient": "#000000", "lights": [
                    { "x": 0, "y": 20, "radius": 80, "color": "#ff0000", "intensity": 2, "follow": follow },
                    { "x": 5, "y": 6, "radius": 30 },
                ] },
            })
        };
        frame(&mut app, Some(scene("hero")));
        let lighting = app.world().resource::<Lighting2d>().clone();
        assert!(lighting.enabled);
        assert_eq!(lighting.ambient, Vec3::ZERO);
        assert_eq!(lighting.lights[0], Light2d { position: Vec2::new(50.0, 30.0), radius: 80.0, color: Vec3::X, intensity: 2.0 }, "跟着精灵，x、y 是偏移");
        assert_eq!(lighting.lights[1], Light2d { position: Vec2::new(5.0, 6.0), radius: 30.0, color: Vec3::ONE, intensity: 1.0 }, "默认白色、强度 1");
        // 精灵挪了，灯跟着
        app.world_mut().query::<&mut Transform2d>().iter_mut(app.world_mut()).next().unwrap().translation.x = 70.0;
        frame(&mut app, None);
        assert_eq!(app.world().resource::<Lighting2d>().lights[0].position, Vec2::new(70.0, 30.0));
        // 带视差的精灵：灯在它画的位置（相机在 x = 100，视差 0.5 的精灵往右挪 50）
        frame(&mut app, Some(json!({
            "camera": { "x": 100 },
            "sprites": [{ "id": "far", "x": 50, "w": 10, "parallax": 0.5 }],
            "lighting": { "lights": [{ "radius": 10, "follow": "far" }] },
        })));
        frame(&mut app, None);
        assert_eq!(app.world().resource::<Lighting2d>().lights[0].position, Vec2::new(100.0, 0.0), "按画的位置");
        frame(&mut app, Some(scene("nobody")));
        assert_eq!(app.world().resource::<Lighting2d>().lights[0].position, Vec2::new(0.0, 20.0), "精灵不存在时 x、y 当世界坐标");
        frame(&mut app, Some(json!({ "sprites": [{ "id": "hero", "w": 10 }] })));
        assert_eq!(*app.world().resource::<Lighting2d>(), Lighting2d::default(), "去掉 lighting 就关掉");
    }

    #[test]
    fn misspelled_light_fields_fail_the_scene() {
        let mut app = app();
        frame(&mut app, Some(json!({ "lighting": { "lights": [{ "radius": 10 }] } })));
        frame(&mut app, Some(json!({ "lighting": { "lights": [{ "radius": 20, "raduis": 3 }] } })));
        assert_eq!(app.world().resource::<Lighting2d>().lights[0].radius, 10.0, "拼错的字段整份场景解析失败，保持上一份");
    }
}
