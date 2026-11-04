//! 场景 → ECS 实体的增量同步。

use std::collections::{HashMap, HashSet};

use bevy_ecs::prelude::*;
use bevy_ecs::system::EntityCommands;
use glam::{Vec2, Vec4};
use nge_core::wgpu;
use nge_render2d::{Camera2d, ClearColor, Sprite, TextureId, Transform2d};

use crate::behave::{Spin, Tappable};
use crate::protocol::{CameraNode, Color, Scene, SpriteNode};
use crate::{NamedTexture, SceneCamera, SceneTextures};

/// 节点 id。Rust 系统用它找到 TSX 声明的实体。
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneId(pub String);

/// 上一次同步时的声明，用来判断节点有没有变。
#[derive(Component)]
pub(crate) struct Declared(SpriteNode);

pub(crate) fn sync(
    mut commands: Commands,
    scene: Res<Scene>,
    textures: Res<SceneTextures>,
    nodes: Query<(Entity, &SceneId, &Declared)>,
    mut camera: Query<&mut Camera2d, With<SceneCamera>>,
    mut clear: ResMut<ClearColor>,
    mut last: Local<Option<(Option<CameraNode>, Option<Color>)>>,
) {
    if !scene.is_changed() {
        return;
    }
    let mut existing: HashMap<&str, (Entity, &SpriteNode)> = nodes.iter().map(|(e, id, d)| (id.0.as_str(), (e, &d.0))).collect();
    let mut seen = HashSet::new();
    for node in &scene.sprites {
        if !seen.insert(node.id.as_str()) {
            log::warn!("场景里有重复的 id {:?}，只保留第一个", node.id);
            continue;
        }
        match existing.remove(node.id.as_str()) {
            Some((_, old)) if old == node => {}
            Some((entity, _)) => apply(commands.entity(entity), node, &textures),
            None => apply(commands.spawn(SceneId(node.id.clone())), node, &textures),
        }
    }
    for (entity, _) in existing.into_values() {
        commands.entity(entity).despawn();
    }

    // 和精灵一样：声明没变就不碰，Rust 系统可以自己驱动相机
    let declared = (scene.camera.clone(), scene.background);
    if last.as_ref() == Some(&declared) {
        return;
    }
    let cam = declared.0.clone().unwrap_or_default();
    for mut camera in &mut camera {
        *camera = Camera2d { position: Vec2::new(cam.x, cam.y), zoom: cam.zoom, ..*camera };
    }
    *clear = declared.1.map_or_else(ClearColor::default, |c| {
        let c = c.0.as_dvec4();
        ClearColor(wgpu::Color { r: c.x, g: c.y, b: c.z, a: c.w })
    });
    *last = Some(declared);
}

fn apply(mut entity: EntityCommands, node: &SpriteNode, textures: &SceneTextures) {
    let image = match node.texture.as_deref().map(|name| (name, textures.0.get(name))) {
        None => Sprite::from_texture(TextureId::WHITE),
        Some((_, Some(NamedTexture::Texture(t)))) => Sprite::from_texture(*t),
        Some((_, Some(NamedTexture::Atlas(a)))) => Sprite::from_atlas(*a, node.frame),
        Some((name, None)) => {
            log::warn!("节点 {:?} 引用了未注册的纹理 {name:?}", node.id);
            Sprite::from_texture(TextureId::WHITE)
        }
    };
    // 只给 w 或 h 时按正方形；都不给时纯色块按 1×1、纹理按像素大小
    let size = match (node.w, node.h) {
        (Some(w), Some(h)) => Some(Vec2::new(w, h)),
        (Some(s), None) | (None, Some(s)) => Some(Vec2::splat(s)),
        (None, None) => None,
    };
    let sprite = Sprite { size, color: node.color.map_or(Vec4::ONE, |c| c.0), ..image };
    let transform = Transform2d {
        translation: Vec2::new(node.x, node.y),
        rotation: node.rotation.to_radians(),
        scale: Vec2::splat(node.scale),
        z: node.z,
    };
    entity.remove::<(Spin, Tappable)>().insert((sprite, transform, Declared(node.clone())));
    if node.spin != 0.0 {
        entity.insert(Spin(node.spin.to_radians()));
    }
    if node.tap {
        entity.insert(Tappable);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use glam::{Vec2, Vec4};
    use nge_render2d::SpriteImage;
    use serde_json::json;

    use super::*;
    use crate::testing::{app, frame};

    fn sprites(app: &mut nge_ecs::App) -> HashMap<String, (Transform2d, Sprite)> {
        let world = app.world_mut();
        world.query::<(&SceneId, &Transform2d, &Sprite)>().iter(world).map(|(id, t, s)| (id.0.clone(), (*t, *s))).collect()
    }

    #[test]
    fn spawns_with_declared_fields() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [
            { "id": "a", "x": 1, "y": 2, "z": 3, "rotation": 90, "scale": 2, "w": 10, "color": "#ff0000" },
        ] })));
        let (t, s) = sprites(&mut app)["a"];
        assert_eq!((t.translation, t.z, t.scale), (Vec2::new(1.0, 2.0), 3.0, Vec2::splat(2.0)));
        assert!((t.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(s.size, Some(Vec2::splat(10.0)), "只给 w 时按正方形");
        assert_eq!(s.color, Vec4::new(1.0, 0.0, 0.0, 1.0));
        assert!(matches!(s.image, SpriteImage::Texture(t) if t == nge_render2d::TextureId::WHITE));
    }

    #[test]
    fn named_textures_and_atlas_frames() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [
            { "id": "p", "texture": "plain" },
            { "id": "g", "texture": "grid", "frame": 5 },
            { "id": "x", "texture": "missing" },
        ] })));
        let all = sprites(&mut app);
        let (images, atlases) = (app.world().resource::<nge_render2d::Assets<nge_render2d::Image>>(), app.world().resource::<nge_render2d::Atlases>());
        assert_eq!(atlases.sprite_size(images, &all["p"].1), Vec2::new(32.0, 16.0));
        assert!(matches!(all["g"].1.image, SpriteImage::Atlas(_, 5)));
        assert_eq!(atlases.sprite_size(images, &all["g"].1), Vec2::splat(8.0));
        assert!(matches!(all["x"].1.image, SpriteImage::Texture(t) if t == nge_render2d::TextureId::WHITE));
    }

    #[test]
    fn updates_changed_and_removes_missing() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a" }, { "id": "b" }] })));
        let before: HashMap<_, _> = {
            let world = app.world_mut();
            world.query::<(Entity, &SceneId)>().iter(world).map(|(e, id)| (id.0.clone(), e)).collect()
        };
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "x": 5 }, { "id": "c" }] })));
        let all = sprites(&mut app);
        let mut ids: Vec<_> = all.keys().cloned().collect();
        ids.sort();
        assert_eq!(ids, ["a", "c"]);
        assert_eq!(all["a"].0.translation.x, 5.0);
        let world = app.world_mut();
        let a = world.query::<(Entity, &SceneId)>().iter(world).find(|(_, id)| id.0 == "a").unwrap().0;
        assert_eq!(a, before["a"], "同一个 id 复用实体");
    }

    #[test]
    fn unchanged_nodes_keep_runtime_state() {
        let mut app = app();
        let scene = json!({ "sprites": [{ "id": "a" }, { "id": "b" }] });
        frame(&mut app, Some(scene.clone()));
        {
            let world = app.world_mut();
            let mut q = world.query::<(&SceneId, &mut Transform2d)>();
            for (_, mut t) in q.iter_mut(world) {
                t.translation.x = 42.0;
            }
        }
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a" }, { "id": "b", "y": 1 }] })));
        let all = sprites(&mut app);
        assert_eq!(all["a"].0.translation.x, 42.0, "没变的节点不覆盖");
        assert_eq!(all["b"].0.translation, Vec2::new(0.0, 1.0), "变了的节点整体覆盖");
    }

    #[test]
    fn duplicate_ids_keep_first() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "x": 1 }, { "id": "a", "x": 2 }] })));
        let all = sprites(&mut app);
        assert_eq!(all.len(), 1);
        assert_eq!(all["a"].0.translation.x, 1.0);
    }

    #[test]
    fn camera_and_background() {
        let mut app = app();
        frame(&mut app, Some(json!({ "background": "#ffffff", "camera": { "x": 3, "zoom": 2 }, "sprites": [] })));
        {
            let world = app.world_mut();
            let cams: Vec<Camera2d> = world.query::<&Camera2d>().iter(world).copied().collect();
            assert_eq!(cams.len(), 1);
            assert_eq!((cams[0].position, cams[0].zoom), (Vec2::new(3.0, 0.0), 2.0));
            assert_eq!(world.resource::<ClearColor>().0.r, 1.0);
        }
        frame(&mut app, Some(json!({ "sprites": [] })));
        let world = app.world_mut();
        assert_eq!(world.query::<&Camera2d>().single(world).unwrap().zoom, 1.0);
        assert_eq!(world.resource::<ClearColor>().0, ClearColor::default().0);
    }

    #[test]
    fn camera_untouched_when_only_sprites_change() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a" }] })));
        {
            let world = app.world_mut();
            world.query::<&mut Camera2d>().single_mut(world).unwrap().position.x = 77.0;
            world.resource_mut::<ClearColor>().0.r = 0.5;
        }
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "y": 1 }] })));
        let world = app.world_mut();
        assert_eq!(world.query::<&Camera2d>().single(world).unwrap().position.x, 77.0, "Rust 驱动的相机不被重置");
        assert_eq!(world.resource::<ClearColor>().0.r, 0.5);
    }

    #[test]
    fn removing_prop_clears_scene() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a" }] })));
        app.prop("scene", None);
        frame(&mut app, None);
        assert!(sprites(&mut app).is_empty());
    }
}
