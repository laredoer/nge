//! 场景 → ECS 实体的增量同步：每种节点按 id 对比上一次的声明，没变的不碰，变了整体覆盖，没了就删。

use std::collections::{HashMap, HashSet};

use bevy_ecs::prelude::*;
use glam::{Vec2, Vec4};
use nge_core::wgpu;
use nge_render2d::{Camera2d, ClearColor, Sprite, SpriteImage, Transform2d};

use crate::anim::FrameAnimation;
use crate::behave::{Spin, Tappable};
use crate::physics::{apply_body, square};
use crate::protocol::{CameraNode, Color, Scene, SpriteNode};
use crate::resolve::SceneAssets;
use crate::SceneCamera;

/// 节点 id。Rust 系统用它找到 TSX 声明的实体。不同种类的节点各用各的 id 空间。
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneId(pub String);

pub(crate) trait Node: Clone + PartialEq + Send + Sync + 'static {
    fn id(&self) -> &str;
}

/// 上一次同步时的声明，用来判断节点有没有变。
#[derive(Component)]
pub(crate) struct Declared<N: Node>(pub N);

/// 按 id 对比 `nodes` 和已有实体：新的 spawn，变了的调 `apply`（带上旧声明），没了的删掉。
pub(crate) fn sync_list<N: Node>(
    commands: &mut Commands,
    nodes: &[N],
    existing: &Query<(Entity, &Declared<N>)>,
    mut apply: impl FnMut(&mut Commands, Entity, Option<&N>, &N),
) {
    let mut old: HashMap<&str, (Entity, &N)> = existing.iter().map(|(e, d)| (d.0.id(), (e, &d.0))).collect();
    let mut seen = HashSet::new();
    for node in nodes {
        if !seen.insert(node.id()) {
            log::warn!("场景里有重复的 id {:?}，只保留第一个", node.id());
            continue;
        }
        let (entity, before) = match old.remove(node.id()) {
            Some((_, before)) if before == node => continue,
            Some((entity, before)) => (entity, Some(before)),
            None => (commands.spawn(SceneId(node.id().to_owned())).id(), None),
        };
        apply(commands, entity, before, node);
        commands.entity(entity).insert(Declared(node.clone()));
    }
    for (entity, _) in old.into_values() {
        commands.entity(entity).despawn();
    }
}

impl Node for SpriteNode {
    fn id(&self) -> &str {
        &self.id
    }
}

pub(crate) fn sync_sprites(mut commands: Commands, scene: Res<Scene>, mut assets: SceneAssets, nodes: Query<(Entity, &Declared<SpriteNode>)>) {
    sync_list(&mut commands, &scene.sprites, &nodes, |commands, entity, before, node| {
        let image = assets.texture(&node.id, node.texture.as_deref(), node.grid, node.frame);
        // 只给 w 或 h 时按正方形；都不给时纯色块按 1×1、纹理按像素大小
        let size = square(node.w, node.h);
        let sprite = Sprite { image, size, color: node.color.map_or(Vec4::ONE, |c| c.0) };
        let transform = Transform2d {
            translation: Vec2::new(node.x, node.y),
            rotation: node.rotation.to_radians(),
            scale: Vec2::splat(node.scale),
            z: node.z,
        };
        let mut entity = commands.entity(entity);
        entity.remove::<(Spin, Tappable)>().insert(sprite);
        // 有刚体时位置归物理管：声明的位置没变就只更新缩放和 z，不把它拉回去
        let placed = before.is_some_and(|b| (b.x, b.y, b.rotation) == (node.x, node.y, node.rotation));
        if node.body.is_some() && placed {
            entity.queue(move |mut e: EntityWorldMut| {
                if let Some(mut t) = e.get_mut::<Transform2d>() {
                    (t.scale, t.z) = (transform.scale, transform.z);
                }
            });
        } else {
            entity.insert(transform);
        }
        apply_body(&mut entity, before, node, size);
        if node.spin != 0.0 && node.body.is_some() {
            log::warn!("节点 {:?} 有 body，spin 不生效（位置和旋转归物理管）", node.id);
        } else if node.spin != 0.0 {
            entity.insert(Spin(node.spin.to_radians()));
        }
        if node.tap {
            entity.insert(Tappable);
        }
        // anim 没变时接着播，不从头开始
        if before.map(|b| &b.anim) != Some(&node.anim) {
            let anim = node.anim.as_ref().and_then(|a| FrameAnimation::new(a.frames.clone(), a.fps, a.looped));
            if node.anim.is_some() && anim.is_none() {
                log::warn!("节点 {:?} 的 anim 没有帧或 fps 不是正数，不播", node.id);
            }
            if anim.is_some() && matches!(image, SpriteImage::Texture(_)) {
                log::warn!("节点 {:?} 不是图集精灵，anim 不起作用", node.id);
            }
            match anim {
                Some(anim) => entity.insert(anim),
                None => entity.remove::<FrameAnimation>(),
            };
        }
    });
}

/// 2D 相机和背景色。和节点一样：声明没变就不碰，Rust 系统可以自己驱动相机。
pub(crate) fn sync_camera(
    scene: Res<Scene>,
    mut camera: Query<&mut Camera2d, With<SceneCamera>>,
    mut clear: ResMut<ClearColor>,
    mut last: Local<Option<(Option<CameraNode>, Option<Color>)>>,
) {
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
