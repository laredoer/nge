//! 3D 骨骼挂点（Bone Socket）：让任意 3D 实体（武器、盾牌、装饰、粒子发射器等）跟随目标模型的指定骨骼/节点运动。

use bevy_ecs::prelude::*;
use glam::{Mat4, Quat, Vec3};

use crate::animator::Pose;
use crate::assets::{Assets3d, Model};
use crate::camera::Transform3d;
use crate::renderer::Model3d;

/// 骨骼挂点变换已同步完成的系统集（位于 `PostUpdate` 的骨骼动画之后）。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SocketsUpdated;

/// 把当前 3D 实体挂到目标 3D 模型的指定骨骼/节点上；每帧动画姿势算完后自动更新自身的 [`Transform3d`]。
#[derive(Component, Debug, Clone, PartialEq)]
pub struct BoneSocket {
    /// 目标 3D 模型实体。
    pub target: Entity,
    /// glTF 节点名（如 `"armR"`、`"hand_r"`、`"head"`）；留空时直接跟在目标模型根节点上。
    pub bone: String,
    /// 相对骨骼局部坐标系的偏移。
    pub offset: Vec3,
    /// 相对骨骼局部坐标系的旋转。
    pub rotation: Quat,
    /// 相对骨骼局部坐标系的缩放。
    pub scale: Vec3,
}

impl BoneSocket {
    pub fn new(target: Entity, bone: impl Into<String>) -> Self {
        Self { target, bone: bone.into(), offset: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE }
    }
}

/// 计算目标模型上指定骨骼/节点这一帧的世界变换矩阵。
/// `bone` 为空字符串时返回目标实体的世界矩阵；找不到同名骨骼时返回 `None`。
pub fn bone_world_matrix(target_t: &Transform3d, model: &Model, pose: Option<&Pose>, bone: &str) -> Option<Mat4> {
    let base = target_t.matrix();
    if bone.is_empty() {
        return Some(base);
    }
    let idx = model.skeleton.names.iter().position(|n| n == bone)?;
    let local = match pose.filter(|p| p.nodes.len() == model.skeleton.nodes.len()) {
        Some(p) => p.nodes[idx],
        None => *model.skeleton.rest().get(idx)?,
    };
    Some(base * local)
}

/// `PostUpdate`（在 `animate` 与 `animate_graphs` 之后）：根据目标模型的当前骨骼姿势更新所有挂点实体的 [`Transform3d`]。
pub(crate) fn update_bone_sockets(
    assets: Res<Assets3d>,
    targets: Query<(&Transform3d, Option<&Model3d>, Option<&Pose>), Without<BoneSocket>>,
    mut sockets: Query<(&BoneSocket, &mut Transform3d)>,
) {
    if sockets.is_empty() {
        return;
    }
    for (socket, mut transform) in &mut sockets {
        let Ok((target_t, model_id, pose)) = targets.get(socket.target) else { continue };
        let bone_mat = if socket.bone.is_empty() {
            target_t.matrix()
        } else {
            let Some(model) = model_id.and_then(|m| assets.model(m.0)) else { continue };
            let Some(m) = bone_world_matrix(target_t, model, pose, &socket.bone) else { continue };
            m
        };
        let world = bone_mat * Mat4::from_scale_rotation_translation(socket.scale, socket.rotation, socket.offset);
        if world.determinant().abs() < 1e-12 || !world.is_finite() {
            continue;
        }
        let (scale, rotation, translation) = world.to_scale_rotation_translation();
        if !(scale.is_finite() && rotation.is_finite() && translation.is_finite()) {
            continue;
        }
        let next = Transform3d { translation, rotation, scale };
        if *transform != next {
            *transform = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use glam::{Mat4, Quat, Vec3, Vec4};

    use super::*;
    use crate::animation::{Animation, Channel, Interpolation, Node, Property, Skeleton, Trs};
    use crate::{Animator, MaterialId, Mesh, Part, Render3dPlugin};

    #[test]
    fn socket_follows_animated_bone_transform() {
        let mut app = nge_ecs::App::new();
        app.add_plugins(Render3dPlugin);
        let mesh = app.world_mut().resource_mut::<Assets3d>().add_mesh(Mesh::cube(1.0)).unwrap();
        let spin_arm = Animation {
            name: "swing".into(),
            duration: 1.0,
            channels: vec![Channel {
                node: 1,
                property: Property::Rotation,
                interpolation: Interpolation::Step,
                times: vec![0.0],
                values: vec![Vec4::from(Quat::from_rotation_y(FRAC_PI_2))],
            }],
        };
        let model = Model {
            parts: vec![Part { node: Some(0), ..Part::new(mesh, MaterialId::DEFAULT, Mat4::IDENTITY) }],
            skeleton: Skeleton {
                nodes: vec![
                    Node { parent: None, rest: Trs::default() },
                    Node { parent: Some(0), rest: Trs { translation: Vec3::new(2.0, 3.0, 0.0), ..Default::default() } },
                ],
                skins: vec![],
                names: vec!["root".into(), "armR".into()],
            },
            animations: vec![spin_arm],
        };
        let model_id = app.world_mut().resource_mut::<Assets3d>().add_model(model).unwrap();
        let hero = app.world_mut().spawn((Model3d(model_id), Transform3d::from_xyz(10.0, 0.0, 0.0), Animator::new("swing"))).id();
        let sword = app
            .world_mut()
            .spawn((
                Transform3d::default(),
                BoneSocket { target: hero, bone: "armR".into(), offset: Vec3::new(0.0, 0.0, -1.0), rotation: Quat::IDENTITY, scale: Vec3::ONE },
            ))
            .id();

        app.tick(nge_core::Viewport { size: [10.0, 10.0], pixels: [10, 10] }, 0.1);
        let t = *app.world().get::<Transform3d>(sword).unwrap();
        // hero at (10,0,0), armR at (+2,+3,0) rotated +90° around Y => local (0,0,-1) maps to (-1,0,0)
        assert!((t.translation - Vec3::new(11.0, 3.0, 0.0)).length() < 1e-4, "挂点位置不对：{:?}", t.translation);
        assert!(t.rotation.angle_between(Quat::from_rotation_y(FRAC_PI_2)) < 1e-3, "挂点朝向不对：{:?}", t.rotation);
    }

    #[test]
    fn socket_uses_rest_pose_when_not_animating() {
        let mut app = nge_ecs::App::new();
        app.add_plugins(Render3dPlugin);
        let mesh = app.world_mut().resource_mut::<Assets3d>().add_mesh(Mesh::cube(1.0)).unwrap();
        let model = Model {
            parts: vec![Part::new(mesh, MaterialId::DEFAULT, Mat4::IDENTITY)],
            skeleton: Skeleton {
                nodes: vec![Node { parent: None, rest: Trs { translation: Vec3::new(0.0, 5.0, 0.0), ..Default::default() } }],
                skins: vec![],
                names: vec!["head".into()],
            },
            animations: vec![],
        };
        let model_id = app.world_mut().resource_mut::<Assets3d>().add_model(model).unwrap();
        let hero = app.world_mut().spawn((Model3d(model_id), Transform3d::from_xyz(1.0, 2.0, 3.0))).id();
        let hat = app.world_mut().spawn((Transform3d::default(), BoneSocket::new(hero, "head"))).id();

        app.tick(nge_core::Viewport { size: [10.0, 10.0], pixels: [10, 10] }, 0.016);
        let t = *app.world().get::<Transform3d>(hat).unwrap();
        assert!((t.translation - Vec3::new(1.0, 7.0, 3.0)).length() < 1e-4, "{:?}", t.translation);
    }
}
