//! 文字、粒子、3D 模型、灯光、3D 相机和环境光的同步。

use bevy_ecs::prelude::*;
use glam::{EulerRot, Quat, Vec2, Vec3, Vec4};
use nge_particles::ParticleEmitter;
use nge_render2d::Transform2d;
use nge_render3d::{AmbientLight, Camera3d, DirectionalLight, GltfModel, Model3d, PointLight, Transform3d};
use nge_text::{ScreenText, Text2d};

use crate::behave::{Follow, Spin, Tappable};
use crate::match2d::Match2d;
use crate::nodes::{AmbientNode, Camera3dNode, EmitterNode, LightKind, LightNode, ModelNode, Particles, TextNode};
use crate::protocol::{Color, Scene};
use crate::resolve::SceneAssets;
use crate::sync::{Declared, Node, sync_list};
use crate::SceneCamera;

macro_rules! node_id {
    ($($t:ty),*) => {$(
        impl Node for $t {
            fn id(&self) -> &str {
                &self.id
            }
        }
    )*};
}
node_id!(TextNode, EmitterNode, ModelNode, LightNode);

fn rgb(color: Option<Color>) -> Option<Vec3> {
    color.map(|c| c.0.truncate())
}

pub(crate) fn sync_texts(mut commands: Commands, scene: Res<Scene>, mut assets: SceneAssets, nodes: Query<(Entity, &Declared<TextNode>)>) {
    sync_list(&mut commands, &scene.texts, &nodes, |commands, entity, _, node| {
        let text = Text2d {
            text: node.text.clone(),
            font: assets.font(node.font.as_deref()),
            size: node.size,
            color: node.color.map_or(Vec4::ONE, |c| c.0),
            anchor: node.anchor.into(),
            max_width: node.max_width,
        };
        let mut entity = commands.entity(entity);
        entity.remove::<(ScreenText, Follow)>().insert((text, Transform2d::from_xy(node.x, node.y)));
        if let Some(model) = &node.follow {
            entity.insert((ScreenText, Follow { model: model.clone(), lift: node.lift, offset: Vec2::new(node.x, node.y) }));
        } else if node.screen {
            entity.insert(ScreenText);
        }
    });
}

/// 节点的粒子参数 → 发射器（角度从度换成弧度）。
pub(crate) fn emitter(particles: &Particles, assets: &mut SceneAssets, owner: &str) -> ParticleEmitter {
    let p = particles;
    let defaults = ParticleEmitter::default();
    ParticleEmitter {
        lifetime: (p.lifetime[0], p.lifetime[1]),
        speed: (p.speed[0], p.speed[1]),
        angle: (p.angle[0].to_radians(), p.angle[1].to_radians()),
        gravity: Vec2::from(p.gravity),
        size: (p.size[0], p.size[1]),
        color: p.color.map_or(defaults.color, |[a, b]| (a.0, b.0)),
        texture: assets.texture(owner, p.texture.as_deref(), None, 0),
        max: p.max.unwrap_or(defaults.max),
        ..defaults
    }
}

pub(crate) fn sync_emitters(mut commands: Commands, scene: Res<Scene>, mut assets: SceneAssets, nodes: Query<(Entity, &Declared<EmitterNode>)>) {
    sync_list(&mut commands, &scene.emitters, &nodes, |commands, entity, before, node| {
        let emitter = ParticleEmitter { rate: node.rate, ..emitter(&node.particles, &mut assets, &node.id) };
        let transform = Transform2d::from_xy(node.particles.x, node.particles.y).with_z(node.particles.z);
        let mut entity = commands.entity(entity);
        // 只挪了位置时保留已经发射出去的粒子
        if before.is_none_or(|b| b.particles.texture != node.particles.texture) {
            entity.remove::<nge_particles::Particles>();
        }
        entity.insert((emitter, transform));
    });
}

pub(crate) fn sync_models(mut commands: Commands, scene: Res<Scene>, mut assets: SceneAssets, nodes: Query<(Entity, &Declared<ModelNode>)>) {
    sync_list(&mut commands, &scene.models, &nodes, |commands, entity, before, node| {
        let [rx, ry, rz] = node.rotation.map(f32::to_radians);
        let transform = Transform3d {
            translation: Vec3::new(node.x, node.y, node.z),
            rotation: Quat::from_euler(EulerRot::YXZ, ry, rx, rz),
            scale: Vec3::splat(node.scale),
        };
        let mut entity = commands.entity(entity);
        entity.remove::<(Spin, Tappable)>().insert(transform);
        // 模型没换时保留已经展开的 Model3d，不闪
        if before.is_none_or(|b| b.model != node.model) {
            entity.remove::<Model3d>().insert(GltfModel(assets.model(&node.model)));
        }
        if node.spin != 0.0 {
            entity.insert(Spin(node.spin.to_radians()));
        }
        if node.tap {
            entity.insert(Tappable);
        }
    });
}

pub(crate) fn sync_lights(mut commands: Commands, scene: Res<Scene>, nodes: Query<(Entity, &Declared<LightNode>)>) {
    sync_list(&mut commands, &scene.lights, &nodes, |commands, entity, _, node| {
        let mut entity = commands.entity(entity);
        entity.remove::<(DirectionalLight, PointLight, Transform3d)>();
        match node.kind {
            LightKind::Directional => {
                let d = DirectionalLight::default();
                entity.insert(DirectionalLight {
                    direction: Vec3::from(node.direction),
                    color: rgb(node.color).unwrap_or(d.color),
                    illuminance: node.intensity.unwrap_or(d.illuminance),
                });
            }
            LightKind::Point => {
                let d = PointLight::default();
                entity.insert((
                    PointLight { color: rgb(node.color).unwrap_or(d.color), intensity: node.intensity.unwrap_or(d.intensity), range: node.range.unwrap_or(d.range) },
                    Transform3d::from_xyz(node.x, node.y, node.z),
                ));
            }
        }
    });
}

/// 3D 相机和环境光：各自的声明变了才覆盖。
pub(crate) fn sync_camera3d(
    scene: Res<Scene>,
    mut commands: Commands,
    mut camera: Query<(Entity, &mut Camera3d, &mut Transform3d, Has<Match2d>), With<SceneCamera>>,
    mut ambient: ResMut<AmbientLight>,
    mut last_camera: Local<Option<Option<Camera3dNode>>>,
    mut last_ambient: Local<Option<Option<AmbientNode>>>,
) {
    if last_camera.as_ref() != Some(&scene.camera3d) {
        let cam = scene.camera3d.clone().unwrap_or_default();
        let (eye, target) = (Vec3::new(cam.x, cam.y, cam.z.unwrap_or(6.0)), Vec3::from(cam.target));
        // 正上方 / 正下方看时 y 不能当 up，改用 -z（画面上方朝 -z）
        let up = if (target - eye).normalize_or_zero().cross(Vec3::Y).length_squared() < 1e-8 { Vec3::NEG_Z } else { Vec3::Y };
        for (entity, mut camera, mut transform, matched) in &mut camera {
            if cam.match2d {
                // 位置、投影由 match_2d 每帧设
                commands.entity(entity).insert(Match2d { z: cam.z.unwrap_or(1000.0) });
                continue;
            }
            // 刚关掉 match2d 时近远平面回到默认；否则留着 Rust 设的
            let base = if matched { Camera3d::default() } else { *camera };
            commands.entity(entity).remove::<Match2d>();
            *camera = Camera3d { fov_y: cam.fov.clamp(1.0, 179.0).to_radians(), ortho: cam.ortho, ..base };
            *transform = Transform3d::from_xyz(eye.x, eye.y, eye.z).looking_at(target, up);
        }
        *last_camera = Some(scene.camera3d.clone());
    }
    if last_ambient.as_ref() != Some(&scene.ambient) {
        let amb = scene.ambient.clone().unwrap_or_default();
        *ambient = AmbientLight { color: rgb(amb.color).unwrap_or(Vec3::ONE), brightness: amb.brightness };
        *last_ambient = Some(scene.ambient.clone());
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use nge_assets::{Assets, Image};
    use nge_render2d::{Atlases, Sprite, SpriteImage};
    use nge_text::Font;
    use serde_json::json;

    use super::*;
    use crate::sync::SceneId;
    use crate::testing::{app, frame};

    fn by_id<C: Component + Clone>(app: &mut nge_ecs::App) -> HashMap<String, C> {
        let world = app.world_mut();
        world.query::<(&SceneId, &C)>().iter(world).map(|(id, c)| (id.0.clone(), c.clone())).collect()
    }

    #[test]
    fn texts_screen_and_follow() {
        let mut app = app();
        frame(&mut app, Some(json!({ "texts": [
            { "id": "w", "text": "world", "x": 3, "size": 20, "anchor": "top" },
            { "id": "s", "text": "hud", "screen": true, "font": "asset:/@t/a.ttf" },
            { "id": "f", "text": "name", "follow": "box", "lift": 1.5, "y": 4 },
        ] })));
        let texts = by_id::<Text2d>(&mut app);
        assert_eq!((texts["w"].text.as_str(), texts["w"].size, texts["w"].anchor), ("world", 20.0, nge_text::Anchor::Top));
        assert!(texts["s"].font.is_some() && texts["w"].font.is_none());
        assert_eq!(app.world().resource::<Assets<Font>>().len(), 1, "字体交给资源层加载");
        let world = app.world_mut();
        let screen: Vec<String> = world.query_filtered::<&SceneId, With<ScreenText>>().iter(world).map(|id| id.0.clone()).collect();
        assert_eq!(screen.len(), 2, "screen 和 follow 的都是屏幕文字 {screen:?}");
        let follow = by_id::<Follow>(&mut app);
        assert_eq!(follow["f"], Follow { model: "box".into(), lift: 1.5, offset: Vec2::new(0.0, 4.0) });

        frame(&mut app, Some(json!({ "texts": [{ "id": "f", "text": "name" }] })));
        let world = app.world_mut();
        assert_eq!(world.query::<&Follow>().iter(world).count(), 0, "去掉 follow 后不再跟随");
        assert_eq!(world.query::<&Text2d>().iter(world).count(), 1);
    }

    #[test]
    fn emitters_keep_particles_when_only_moved() {
        let mut app = app();
        frame(&mut app, Some(json!({ "emitters": [{ "id": "smoke", "rate": 10, "angle": [90, 90], "lifetime": [5, 5] }] })));
        frame(&mut app, None);
        let emitter = by_id::<ParticleEmitter>(&mut app)["smoke"].clone();
        assert_eq!(emitter.rate, 10.0);
        assert!((emitter.angle.0 - std::f32::consts::FRAC_PI_2).abs() < 1e-6, "度换成弧度");
        assert!(!emitter.despawn_when_done, "场景里的发射器不自动删除");
        let alive = |app: &mut nge_ecs::App| {
            let world = app.world_mut();
            world.query::<&nge_particles::Particles>().iter(world).map(|p| p.len()).sum::<usize>()
        };
        let before = alive(&mut app);
        assert!(before > 0);
        frame(&mut app, Some(json!({ "emitters": [{ "id": "smoke", "rate": 10, "angle": [90, 90], "lifetime": [5, 5], "x": 30 }] })));
        assert!(alive(&mut app) >= before, "只挪位置时已发射的粒子还在");
        assert_eq!(by_id::<Transform2d>(&mut app)["smoke"].translation.x, 30.0);
        frame(&mut app, Some(json!({})));
        assert_eq!(alive(&mut app), 0);
    }

    #[test]
    fn sprite_asset_textures_and_uri_atlases() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [
            { "id": "a", "texture": "asset:/@t/a.png" },
            { "id": "b", "texture": "asset:/@t/a.png", "grid": [2, 2], "frame": 3 },
            { "id": "c", "texture": "asset:/@t/a.png", "grid": [2, 2], "frame": 1 },
        ] })));
        let sprites = by_id::<Sprite>(&mut app);
        let (SpriteImage::Atlas(b, 3), SpriteImage::Atlas(c, 1)) = (sprites["b"].image, sprites["c"].image) else { panic!("{sprites:?}") };
        assert_eq!(b, c, "同一张图同一个网格只建一个图集");
        assert!(matches!(sprites["a"].image, SpriteImage::Texture(_)));
        // 白色 + 测试注册的两张 + 这一张
        assert_eq!(app.world().resource::<Assets<Image>>().len(), 4);
        let _ = app.world().resource::<Atlases>();
    }

    #[test]
    fn models_transform_spin_and_reload() {
        let mut app = app();
        let node = json!({ "id": "box", "model": "asset:/@t/a.glb", "x": 1, "rotation": [0, 90, 0], "scale": 2, "spin": 90, "tap": true });
        frame(&mut app, Some(json!({ "models": [node] })));
        let t = by_id::<Transform3d>(&mut app)["box"];
        assert_eq!((t.translation, t.scale), (Vec3::new(1.0, 0.0, 0.0), Vec3::splat(2.0)));
        let world = app.world_mut();
        let entity = world.query_filtered::<Entity, (With<Spin>, With<Tappable>, With<Declared<ModelNode>>)>().single(world).unwrap();
        // 模拟已经展开好
        let model = crate::testing::cube_model(world);
        world.entity_mut(entity).insert(Model3d(model));
        frame(&mut app, Some(json!({ "models": [{ "id": "box", "model": "asset:/@t/a.glb", "x": 5 }] })));
        assert!(app.world().get::<Model3d>(entity).is_some(), "模型没换时不重新展开");
        assert!(app.world().get::<Spin>(entity).is_none());
        frame(&mut app, Some(json!({ "models": [{ "id": "box", "model": "asset:/@t/b.glb" }] })));
        assert!(app.world().get::<Model3d>(entity).is_none(), "换了模型要重新加载");
    }

    #[test]
    fn lights_camera3d_and_ambient() {
        let mut app = app();
        frame(&mut app, Some(json!({
            "lights": [{ "id": "sun", "kind": "directional", "direction": [0, -1, -1], "intensity": 3 }, { "id": "lamp", "kind": "point", "x": 2, "color": "#ff0000" }],
            "camera3d": { "x": 0, "y": 0, "z": 10, "fov": 45 },
            "ambient": { "brightness": 0.5 },
        })));
        let world = app.world_mut();
        let sun = *world.query::<&DirectionalLight>().single(world).unwrap();
        assert_eq!((sun.direction, sun.illuminance), (Vec3::new(0.0, -1.0, -1.0), 3.0));
        let (lamp, at) = world.query::<(&PointLight, &Transform3d)>().single(world).map(|(l, t)| (*l, *t)).unwrap();
        assert_eq!((lamp.color, at.translation), (Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0)));
        let (cam, t) = world.query::<(&Camera3d, &Transform3d)>().single(world).map(|(c, t)| (*c, *t)).unwrap();
        assert!((cam.fov_y - 45f32.to_radians()).abs() < 1e-6);
        assert_eq!(t.translation, Vec3::new(0.0, 0.0, 10.0));
        assert!((t.rotation * Vec3::NEG_Z - Vec3::NEG_Z).length() < 1e-5, "看向原点");
        assert_eq!(world.resource::<AmbientLight>().brightness, 0.5);

        frame(&mut app, Some(json!({ "lights": [{ "id": "sun", "kind": "point" }] })));
        let world = app.world_mut();
        assert_eq!(world.query::<&DirectionalLight>().iter(world).count(), 0, "换类型时去掉旧的灯");
        assert_eq!(world.query::<&PointLight>().iter(world).count(), 1);
        let t = *world.query_filtered::<&Transform3d, With<Camera3d>>().single(world).unwrap();
        assert_eq!(t.translation, Vec3::new(0.0, 2.0, 6.0), "不声明时回到默认相机");
    }

    #[test]
    fn top_down_camera_looks_down() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera3d": { "x": 0, "y": 10, "z": 0 } })));
        let world = app.world_mut();
        let t = *world.query_filtered::<&Transform3d, With<Camera3d>>().single(world).unwrap();
        assert!((t.rotation * Vec3::NEG_Z - Vec3::NEG_Y).length() < 1e-5, "{:?}", t.rotation * Vec3::NEG_Z);
    }

    #[test]
    fn changing_ambient_keeps_a_camera_moved_by_rust() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera3d": { "x": 0, "y": 0, "z": 10 }, "ambient": { "brightness": 0.5 } })));
        let world = app.world_mut();
        world.query_filtered::<&mut Transform3d, With<Camera3d>>().single_mut(world).unwrap().translation.x = 3.0;
        frame(&mut app, Some(json!({ "camera3d": { "x": 0, "y": 0, "z": 10 }, "ambient": { "brightness": 0.8 } })));
        let world = app.world_mut();
        assert_eq!(world.query_filtered::<&Transform3d, With<Camera3d>>().single(world).unwrap().translation.x, 3.0);
        assert_eq!(world.resource::<AmbientLight>().brightness, 0.8);
    }

    #[test]
    fn huge_grids_fall_back_to_the_whole_texture() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "texture": "asset:/@t/a.png", "grid": [65536, 65536], "frame": 1 }] })));
        assert!(matches!(by_id::<Sprite>(&mut app)["a"].image, SpriteImage::Texture(_)));
    }
}
