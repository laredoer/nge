//! 模型节点的 `attach`：3D 模型跟着一个精灵（通常是 2D 刚体）走和转（见 docs/joints.md）。

use bevy_ecs::prelude::*;
use glam::{Quat, Vec2, Vec3};
use nge_render2d::{DrawAt, Transform2d};
use nge_render3d::Transform3d;

use crate::protocol::SpriteNode;
use crate::sync::Declared;

/// 模型挂在精灵上：`offset` 是精灵坐标里的偏移（x、y 跟着精灵转，z 不变），`rotation` 是在精灵角度之上再转多少。
#[derive(Component, Debug, Clone, PartialEq)]
pub(crate) struct Attach {
    pub sprite: String,
    pub offset: Vec3,
    pub rotation: Quat,
}

/// 场景层声明的 3D 骨骼挂点（按目标模型 id 解析为 [`nge_render3d::BoneSocket`]）。
#[derive(Component, Debug, Clone, PartialEq)]
pub(crate) struct SceneSocket {
    pub target: String,
    pub bone: String,
    pub offset: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

/// 物理一步之后，按精灵画在哪（有刚体时是插值过的 `DrawAt`）摆模型。精灵不存在时模型留在原地。
pub(crate) fn attach(mut models: Query<(&Attach, &mut Transform3d)>, sprites: Query<(&Declared<SpriteNode>, &Transform2d, Option<&DrawAt>)>) {
    if models.is_empty() {
        return;
    }
    for (attach, mut t) in &mut models {
        let Some((_, sprite, drawn)) = sprites.iter().find(|(d, ..)| d.0.id == attach.sprite) else { continue };
        let (center, angle) = drawn.map_or((sprite.translation, sprite.rotation), |d| (d.translation, d.rotation));
        let xy = center + Vec2::from_angle(angle).rotate(attach.offset.truncate());
        let translation = xy.extend(attach.offset.z);
        let rotation = Quat::from_rotation_z(angle) * attach.rotation;
        if (t.translation, t.rotation) != (translation, rotation) {
            (t.translation, t.rotation) = (translation, rotation);
        }
    }
}

/// `Update`：按目标模型的 `SceneId` 把 [`SceneSocket`] 解析成 [`nge_render3d::BoneSocket`]，供 `PostUpdate` 的骨骼挂点系统同步。
pub(crate) fn resolve_sockets(
    mut commands: Commands,
    sockets: Query<(Entity, &SceneSocket, Option<&nge_render3d::BoneSocket>)>,
    models: Query<(Entity, &crate::sync::SceneId), With<Declared<crate::nodes::ModelNode>>>,
) {
    if sockets.is_empty() {
        return;
    }
    for (entity, socket, current) in &sockets {
        let target = models.iter().find(|(_, id)| id.0 == socket.target).map(|(e, _)| e);
        match target {
            Some(target) if target != entity => {
                let want = nge_render3d::BoneSocket {
                    target,
                    bone: socket.bone.clone(),
                    offset: socket.offset,
                    rotation: socket.rotation,
                    scale: socket.scale,
                };
                if current != Some(&want) {
                    commands.entity(entity).insert(want);
                }
            }
            _ => {
                if current.is_some() {
                    commands.entity(entity).remove::<nge_render3d::BoneSocket>();
                }
            }
        }
    }
}

/// 场景层声明的 3D 骨骼注视（按目标模型 id 或固定世界坐标解析为 [`nge_render3d::BoneLookAt`]）。
#[derive(Component, Debug, Clone, PartialEq)]
pub(crate) struct SceneLookAt {
    pub bone: String,
    pub target: Option<String>,
    pub point: Option<Vec3>,
    pub weight: f32,
    pub max_angle: f32,
    pub speed: f32,
}

/// `Update`：按目标模型的 `SceneId` 把 [`SceneLookAt`] 同步到 [`nge_render3d::BoneLookAt`]，保留平滑插值状态。
pub(crate) fn resolve_look_at(
    mut commands: Commands,
    mut lookers: Query<(Entity, &SceneLookAt, Option<&mut nge_render3d::BoneLookAt>)>,
    models: Query<(Entity, &crate::sync::SceneId, &Transform3d), With<Declared<crate::nodes::ModelNode>>>,
) {
    if lookers.is_empty() {
        return;
    }
    for (entity, cfg, current) in &mut lookers {
        let target_pos = cfg
            .target
            .as_deref()
            .and_then(|name| models.iter().find(|(e, id, _)| *e != entity && id.0 == name).map(|(_, _, t)| t.translation))
            .or(cfg.point);
        let Some(pos) = target_pos else { continue };
        match current {
            Some(mut old) => {
                if old.bone != cfg.bone {
                    old.bone = cfg.bone.clone();
                }
                old.target = pos;
                old.weight = cfg.weight;
                old.max_angle = cfg.max_angle;
                old.speed = cfg.speed;
            }
            None => {
                let mut comp = nge_render3d::BoneLookAt::new(cfg.bone.clone(), pos);
                comp.weight = cfg.weight;
                comp.max_angle = cfg.max_angle;
                comp.speed = cfg.speed;
                commands.entity(entity).insert(comp);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::sync::SceneId;
    use crate::testing::{app, frame};

    fn model(app: &mut nge_ecs::App, id: &str) -> Transform3d {
        let world = app.world_mut();
        world.query::<(&SceneId, &Transform3d, Has<Attach>)>().iter(world).find(|(s, _, a)| s.0 == id && *a).map(|(_, t, _)| *t).unwrap()
    }

    fn sprite(app: &mut nge_ecs::App, id: &str) -> (Vec2, f32) {
        let world = app.world_mut();
        world.query::<(&SceneId, &DrawAt)>().iter(world).find(|(s, _)| s.0 == id).map(|(_, d)| (d.translation, d.rotation)).unwrap()
    }

    /// 掉下去、被撞得转起来的箱子：模型跟着它画的位置和角度走，偏移跟着转，z 不变。
    #[test]
    fn model_follows_a_falling_spinning_body() {
        let mut app = app();
        let scene = json!({
            "sprites": [{ "id": "box", "x": 0, "y": 100, "w": 20, "body": { "angularVelocity": 90 } }],
            "models": [{ "id": "crate", "model": "asset:/@t/crate.glb", "attach": "box", "x": 10, "y": 0, "z": -5, "rotation": [30, 0, 0], "scale": 20 }],
        });
        frame(&mut app, Some(scene));
        frame(&mut app, None);
        // 半步：物理不走，精灵画在两步之间（DrawAt 和 Transform2d 不一样），模型跟着画的位置
        app.tick(crate::testing::VIEWPORT, nge_physics2d::STEP / 2.0);
        let (center, angle) = sprite(&mut app, "box");
        let world = app.world_mut();
        let body = world.query::<(&SceneId, &Transform2d)>().iter(world).find(|(s, _)| s.0 == "box").map(|(_, t)| t.translation).unwrap();
        assert_ne!(body, center);
        assert!(center.y < 100.0 && angle > 0.1, "箱子在掉、在转 {center} {angle}");
        let t = model(&mut app, "crate");
        let expect = center + Vec2::from_angle(angle) * 10.0;
        assert!((t.translation.truncate() - expect).length() < 1e-3, "{} vs {expect}", t.translation);
        assert_eq!(t.translation.z, -5.0);
        let want = Quat::from_rotation_z(angle) * Quat::from_rotation_x(30f32.to_radians());
        assert!(t.rotation.dot(want).abs() > 1.0 - 1e-6, "先按声明转，再跟着精灵转 {} {want}", t.rotation);
        assert_eq!(t.scale, Vec3::splat(20.0));
    }

    #[test]
    fn missing_sprite_leaves_the_model_and_detaching_restores_it() {
        let mut app = app();
        let crate_at = |attach: Option<&str>| json!({ "models": [{ "id": "crate", "model": "asset:/@t/crate.glb", "attach": attach, "x": 3, "y": 4 }] });
        frame(&mut app, Some(crate_at(Some("nobody"))));
        assert_eq!(model(&mut app, "crate").translation, Vec3::new(3.0, 4.0, 0.0), "没有这个精灵：留在声明的位置");
        frame(&mut app, Some(crate_at(None)));
        let world = app.world_mut();
        assert_eq!(world.query::<&Attach>().iter(world).count(), 0, "去掉 attach 就不跟了");
    }

    #[test]
    fn model_socket_binds_to_target_bone_and_detaches() {
        let mut app = app();
        let scene = json!({
            "models": [
                { "id": "hero", "model": "asset:/@t/crate.glb", "x": 10, "y": 2, "z": 3 },
                { "id": "staff", "model": "asset:/@t/crate.glb", "socket": { "target": "hero" }, "x": 1, "y": 2, "z": 0 },
            ]
        });
        frame(&mut app, Some(scene));
        let world = app.world_mut();
        let (t, has_socket) = world
            .query::<(&SceneId, &Transform3d, Has<nge_render3d::BoneSocket>)>()
            .iter(world)
            .find(|(id, ..)| id.0 == "staff")
            .map(|(_, t, s)| (*t, s))
            .unwrap();
        assert!(has_socket, "应解析并挂上 BoneSocket");
        assert!((t.translation - Vec3::new(11.0, 4.0, 3.0)).length() < 1e-4, "挂在根节点上：{:?}", t.translation);

        frame(&mut app, Some(json!({ "models": [{ "id": "hero", "model": "asset:/@t/crate.glb" }, { "id": "staff", "model": "asset:/@t/crate.glb", "x": 5 }] })));
        let world = app.world_mut();
        assert_eq!(world.query::<&nge_render3d::BoneSocket>().iter(world).count(), 0, "去掉 socket 后解除绑定");
    }
}
