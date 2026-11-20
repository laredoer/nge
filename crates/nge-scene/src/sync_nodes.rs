//! 文字、粒子、3D 模型、灯光、3D 相机和环境光的同步。

use bevy_ecs::prelude::*;
use glam::{EulerRot, Quat, Vec2, Vec3, Vec4};
use nge_particles::ParticleEmitter;
use nge_render2d::Transform2d;
use nge_ecs::Interleave;
use nge_render3d::{AmbientLight, Animator, BoneSocket, Camera3d, DirectionalLight, GltfModel, MaterialShader3d, MaterialStyleOverride, Model3d, PointLight, Pose, ShadowPriority, Shadows, SpotLight, Transform3d};
use nge_text::{ScreenText, Text2d};

use crate::attach::{Attach, SceneSocket};
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
            visible: None,
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

pub(crate) fn sync_models(
    mut commands: Commands,
    scene: Res<Scene>,
    mut assets: SceneAssets,
    nodes: Query<(Entity, &Declared<ModelNode>)>,
    mut animators: Query<&mut Animator>,
    mut graphs: Local<crate::anim_graph::GraphCache>,
) {
    sync_list(&mut commands, &scene.models, &nodes, |commands, entity, before, node| {
        let [rx, ry, rz] = node.rotation.map(f32::to_radians);
        let transform = Transform3d {
            translation: Vec3::new(node.x, node.y, node.z),
            rotation: Quat::from_euler(EulerRot::YXZ, ry, rx, rz),
            scale: Vec3::splat(node.scale),
        };
        let mut entity = commands.entity(entity);
        entity.remove::<(Spin, Tappable, Attach, SceneSocket, BoneSocket)>();
        // 有刚体或 3D 导航代理的：声明的位置没变就不拉回去；变了就是瞬移
        let keeps_place = crate::physics3d::keeps_place(before, node) || crate::nav3d::keeps_place(before, node);
        if !keeps_place {
            entity.insert(transform);
        }
        let teleported = node.body.is_some() && before.is_some() && !keeps_place;
        crate::physics3d::apply_body(&mut entity, before, node);
        // 模型没换时保留已经展开的 Model3d，不闪
        let same_model = before.is_some_and(|b| b.model == node.model);
        if !same_model {
            entity.remove::<Model3d>().insert(GltfModel(assets.model(&node.model)));
        }
        // 状态机接管时不用 animation（见 docs/anim-graph.md）
        let graph = crate::anim_graph::apply_graph(&mut entity, &node.id, before.map(|b| (&b.anim_graph, &b.anim_params)), &node.anim_graph, &node.anim_params, &mut graphs);
        if graph && node.animation.is_some() {
            log::warn!("模型 {:?} 同时写了 animation 和 animGraph，以 animGraph 为准", node.id);
        }
        match &node.animation {
            _ if graph => {
                entity.remove::<Animator>();
            }
            None => {
                entity.remove::<(Animator, Pose)>();
            }
            Some(a) => {
                // 同一个模型：换动画按 fade 淡过去，只改速度和循环时接着播
                match animators.get_mut(entity.id()).ok().filter(|_| same_model) {
                    Some(mut old) => {
                        old.play(a.clip.clone(), a.fade);
                        old.speed = a.speed;
                        old.looping = a.looping;
                    }
                    None => {
                        entity.insert(Animator { speed: a.speed, looping: a.looping, ..Animator::new(a.clip.clone()) });
                    }
                }
            }
        }
        if node.body.is_some() {
            if node.attach.is_some() || node.socket.is_some() || node.spin != 0.0 {
                log::warn!("模型 {:?} 有刚体，attach / socket / spin 不生效", node.id);
            }
        } else if let Some(socket) = &node.socket {
            entity.insert(SceneSocket {
                target: socket.target.clone(),
                bone: socket.bone.clone(),
                offset: transform.translation,
                rotation: transform.rotation,
                scale: transform.scale,
            });
            if node.attach.is_some() || node.spin != 0.0 {
                log::warn!("模型 {:?} 挂在骨骼上，attach / spin 不生效", node.id);
            }
        } else if let Some(sprite) = &node.attach {
            entity.insert(Attach { sprite: sprite.clone(), offset: transform.translation, rotation: transform.rotation });
            if node.spin != 0.0 {
                log::warn!("模型 {:?} 跟着精灵走，spin 不生效", node.id);
            }
        } else if node.spin != 0.0 {
            entity.insert(Spin(node.spin.to_radians()));
        }
        if node.tap {
            entity.insert(Tappable);
        }
        match node.style {
            Some(style) => { entity.insert(MaterialStyleOverride(style.into())); }
            None => { entity.remove::<MaterialStyleOverride>(); }
        }
        match &node.shader {
            Some(name) => {
                entity.insert(MaterialShader3d {
                    name: name.clone(),
                    params: node.params.map_or(Vec4::ZERO, Vec4::from),
                });
            }
            None => {
                if node.params.is_some() {
                    log::warn!("模型 {:?} 写了 params 没写 shader：params 不起作用", node.id);
                }
                entity.remove::<MaterialShader3d>();
            }
        }
        crate::nav3d::apply_nav(&mut entity, before, node);
        crate::particles3d::apply_trail(&mut entity, &node.id, node.trail.as_ref(), teleported, &mut assets);
    });
}

pub(crate) fn sync_lights(mut commands: Commands, scene: Res<Scene>, nodes: Query<(Entity, &Declared<LightNode>)>) {
    sync_list(&mut commands, &scene.lights, &nodes, |commands, entity, _, node| {
        let mut entity = commands.entity(entity);
        entity.remove::<(DirectionalLight, PointLight, SpotLight, Transform3d, ShadowPriority)>();
        if let Some(p) = node.shadow_priority {
            entity.insert(ShadowPriority(p));
        }
        match node.kind {
            LightKind::Directional => {
                let d = DirectionalLight::default();
                entity.insert(DirectionalLight {
                    direction: Vec3::from(node.direction),
                    color: rgb(node.color).unwrap_or(d.color),
                    illuminance: node.intensity.unwrap_or(d.illuminance),
                    shadows: node.shadows,
                });
            }
            LightKind::Point => {
                let d = PointLight::default();
                entity.insert((
                    PointLight { color: rgb(node.color).unwrap_or(d.color), intensity: node.intensity.unwrap_or(d.intensity), range: node.range.unwrap_or(d.range), shadows: node.shadows },
                    Transform3d::from_xyz(node.x, node.y, node.z),
                ));
            }
            LightKind::Spot => {
                let d = SpotLight::default();
                let outer = node.angle.map_or(d.outer_angle, f32::to_radians);
                let spot = SpotLight {
                    color: rgb(node.color).unwrap_or(d.color),
                    intensity: node.intensity.unwrap_or(d.intensity),
                    range: node.range.unwrap_or(d.range),
                    inner_angle: node.inner_angle.map_or(outer * 0.8, f32::to_radians),
                    outer_angle: outer,
                    shadows: node.shadows,
                };
                // 引擎里聚光灯照向 rotation × -Z
                let aim = Vec3::from(node.direction).try_normalize().unwrap_or(Vec3::NEG_Y);
                let transform = Transform3d { rotation: Quat::from_rotation_arc(Vec3::NEG_Z, aim), ..Transform3d::from_xyz(node.x, node.y, node.z) };
                entity.insert((spot, transform));
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
    mut interleave: ResMut<Interleave>,
    mut shadows: ResMut<Shadows>,
    mut rig3d: ResMut<crate::camera3d_rig::Camera3dRig>,
    mut last_camera: Local<Option<Option<Camera3dNode>>>,
    mut last_ambient: Local<Option<Option<AmbientNode>>>,
) {
    if last_camera.as_ref() != Some(&scene.camera3d) {
        let cam = scene.camera3d.clone().unwrap_or_default();
        rig3d.set_node(cam.clone());
        interleave.0 = cam.interleave;
        // 声明了的写进去；上次声明了、这次去掉的回到默认；其余留着 Rust 设的
        let before = last_camera.clone().flatten().unwrap_or_default();
        let d = Shadows::default();
        match (cam.shadow_cascades, before.shadow_cascades) {
            (Some(n), _) => shadows.cascades = n,
            (None, Some(_)) => shadows.cascades = d.cascades,
            (None, None) => {}
        }
        match (cam.shadow_distance, before.shadow_distance) {
            (Some(v), _) => shadows.distance = v,
            (None, Some(_)) => shadows.distance = d.distance,
            (None, None) => {}
        }
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
    use nge_render3d::Fade;
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
    fn model_animation_keeps_playing_when_only_speed_changes() {
        let mut app = app();
        let declare = |app: &mut nge_ecs::App, model: &str, animation: serde_json::Value| {
            frame(app, Some(json!({ "models": [{ "id": "flag", "model": model, "animation": animation }] })));
        };
        declare(&mut app, "asset:/@t/a.glb", json!({ "clip": "wave" }));
        let world = app.world_mut();
        let entity = world.query_filtered::<Entity, With<Declared<ModelNode>>>().single(world).unwrap();
        let animator = |app: &nge_ecs::App| app.world().get::<Animator>(entity).cloned();
        assert_eq!(animator(&app), Some(Animator::new("wave")), "默认原速、循环");
        app.world_mut().get_mut::<Animator>(entity).unwrap().time = 0.5;

        declare(&mut app, "asset:/@t/a.glb", json!({ "clip": "wave", "speed": 2, "loop": false }));
        assert_eq!(animator(&app), Some(Animator { speed: 2.0, looping: false, time: 0.5, ..Animator::new("wave") }), "只改速度和循环：接着播");
        declare(&mut app, "asset:/@t/a.glb", json!({ "clip": "idle" }));
        assert_eq!(animator(&app).map(|a| (a.clip, a.time)), Some(("idle".into(), 0.0)), "换了动画从头播");
        app.world_mut().get_mut::<Animator>(entity).unwrap().time = 0.5;
        declare(&mut app, "asset:/@t/b.glb", json!({ "clip": "idle" }));
        assert_eq!(animator(&app).map(|a| a.time), Some(0.0), "换了模型从头播");
        frame(&mut app, Some(json!({ "models": [{ "id": "flag", "model": "asset:/@t/b.glb" }] })));
        assert!(animator(&app).is_none() && app.world().get::<Pose>(entity).is_none(), "不播了去掉");
    }

    #[test]
    fn model_animation_fades_into_a_new_clip() {
        let mut app = app();
        let declare = |app: &mut nge_ecs::App, animation: serde_json::Value| {
            frame(app, Some(json!({ "models": [{ "id": "jelly", "model": "asset:/@t/a.glb", "animation": animation }] })));
        };
        declare(&mut app, json!({ "clip": "idle", "fade": 0.3 }));
        let world = app.world_mut();
        let entity = world.query_filtered::<Entity, With<Declared<ModelNode>>>().single(world).unwrap();
        assert_eq!(app.world().get::<Animator>(entity).unwrap().fade, None, "第一次播没东西可淡");
        app.world_mut().get_mut::<Animator>(entity).unwrap().time = 0.5;

        declare(&mut app, json!({ "clip": "squish", "fade": 0.3, "speed": 2 }));
        let a = app.world().get::<Animator>(entity).cloned().unwrap();
        assert_eq!((a.clip.as_str(), a.time, a.speed), ("squish", 0.0, 2.0));
        assert_eq!(a.fade, Some(Fade { clip: "idle".into(), time: 0.5, speed: 1.0, looping: true, elapsed: 0.0, duration: 0.3 }), "旧动画按原来的速度淡出");

        app.world_mut().get_mut::<Animator>(entity).unwrap().fade = None;
        declare(&mut app, json!({ "clip": "squish", "fade": 0.5, "speed": 2 }));
        assert_eq!(app.world().get::<Animator>(entity).unwrap().fade, None, "只改 fade 不重新淡");
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

        assert!(!sun.shadows && !world.resource::<nge_ecs::Interleave>().0, "默认都关");
        frame(&mut app, Some(json!({ "lights": [{ "id": "sun", "kind": "directional", "shadows": true }], "camera3d": { "interleave": true } })));
        let world = app.world_mut();
        assert!(world.query::<&DirectionalLight>().single(world).unwrap().shadows);
        assert!(world.resource::<nge_ecs::Interleave>().0);

        frame(&mut app, Some(json!({ "lights": [{ "id": "sun", "kind": "point" }] })));
        let world = app.world_mut();
        assert_eq!(world.query::<&DirectionalLight>().iter(world).count(), 0, "换类型时去掉旧的灯");
        assert_eq!(world.query::<&PointLight>().iter(world).count(), 1);
        let t = *world.query_filtered::<&Transform3d, With<Camera3d>>().single(world).unwrap();
        assert_eq!(t.translation, Vec3::new(0.0, 2.0, 6.0), "不声明时回到默认相机");
        assert!(!world.resource::<nge_ecs::Interleave>().0, "不声明时关掉混排");
    }

    #[test]
    fn spot_lights_aim_along_direction_and_cast_shadows() {
        let mut app = app();
        frame(&mut app, Some(json!({ "lights": [{ "id": "spot", "kind": "spot", "x": 1, "y": 2, "z": 3, "direction": [1, 0, 0], "angle": 40, "shadows": true, "range": 50 }] })));
        let world = app.world_mut();
        let (spot, t) = world.query::<(&SpotLight, &Transform3d)>().single(world).map(|(l, t)| (*l, *t)).unwrap();
        assert!((spot.outer_angle - 40f32.to_radians()).abs() < 1e-6 && (spot.inner_angle - 32f32.to_radians()).abs() < 1e-6, "内锥默认外锥的 80%：{spot:?}");
        assert!(spot.shadows && spot.range == 50.0);
        assert_eq!(t.translation, Vec3::new(1.0, 2.0, 3.0));
        assert!((t.rotation * Vec3::NEG_Z - Vec3::X).length() < 1e-5, "照向 direction：{:?}", t.rotation * Vec3::NEG_Z);

        frame(&mut app, Some(json!({ "lights": [{ "id": "spot", "kind": "spot", "direction": [0, 1, 0], "innerAngle": 10 }] })));
        let world = app.world_mut();
        let (spot, t) = world.query::<(&SpotLight, &Transform3d)>().single(world).map(|(l, t)| (*l, *t)).unwrap();
        let d = SpotLight::default();
        assert!((spot.outer_angle - d.outer_angle).abs() < 1e-6 && (spot.inner_angle - 10f32.to_radians()).abs() < 1e-6 && !spot.shadows);
        assert!((t.rotation * Vec3::NEG_Z - Vec3::Y).length() < 1e-5, "朝上：{:?}", t.rotation * Vec3::NEG_Z);

        frame(&mut app, Some(json!({ "lights": [{ "id": "spot", "kind": "spot", "direction": [0, 0, 1] }] })));
        let world = app.world_mut();
        let t = *world.query_filtered::<&Transform3d, With<SpotLight>>().single(world).unwrap();
        assert!((t.rotation * Vec3::NEG_Z - Vec3::Z).length() < 1e-5, "正对 +Z 的反方向也能转过去：{:?}", t.rotation * Vec3::NEG_Z);

        frame(&mut app, Some(json!({ "lights": [{ "id": "spot", "kind": "point", "shadows": true }] })));
        let world = app.world_mut();
        assert_eq!(world.query::<&SpotLight>().iter(world).count(), 0, "换类型时去掉旧的灯");
        let (point, t) = world.query::<(&PointLight, &Transform3d)>().single(world).map(|(l, t)| (*l, *t)).unwrap();
        assert!(point.shadows, "点光源的影子跟着节点");
        assert_eq!(t.rotation, Quat::IDENTITY, "不带聚光灯的朝向");

        frame(&mut app, Some(json!({ "lights": [{ "id": "spot", "kind": "directional" }] })));
        let world = app.world_mut();
        assert_eq!(world.query::<(&SpotLight, &PointLight)>().iter(world).count(), 0);
        assert_eq!(world.query_filtered::<&Transform3d, With<Declared<LightNode>>>().iter(world).count(), 0, "方向光不带位置");
    }

    #[test]
    fn camera3d_sets_shadow_cascades_and_keeps_other_shadow_fields() {
        let mut app = app();
        app.world_mut().resource_mut::<Shadows>().size = 1024;
        frame(&mut app, Some(json!({ "camera3d": { "match2d": true, "shadowCascades": 3, "shadowDistance": 80 } })));
        let s = *app.world().resource::<Shadows>();
        assert_eq!((s.cascades, s.distance, s.size), (3, 80.0, 1024));
        app.world_mut().resource_mut::<Shadows>().local_size = 256;
        frame(&mut app, Some(json!({ "camera3d": { "match2d": true, "shadowCascades": 2 } })));
        let s = *app.world().resource::<Shadows>();
        assert_eq!((s.cascades, s.distance, s.size, s.local_size), (2, Shadows::default().distance, 1024, 256), "去掉的字段回到默认，别的留着");
        app.world_mut().resource_mut::<Shadows>().cascades = 4;
        frame(&mut app, Some(json!({ "camera3d": { "match2d": true, "shadowCascades": 2 }, "ambient": { "brightness": 0.3 } })));
        assert_eq!(app.world().resource::<Shadows>().cascades, 4, "相机声明没变时不覆盖 Rust 设的");
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
