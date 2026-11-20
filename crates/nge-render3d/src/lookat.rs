//! 3D 程序化骨骼注视（BoneLookAt）：在骨骼动画 / 状态机算完 `Pose` 之后、骨骼挂点同步之前，
//! 将指定骨骼（及其所有子孙骨骼）平滑转向目标世界坐标，支持权重混合与最大偏转角限幅。

use bevy_ecs::prelude::*;
use glam::{Mat4, Quat, Vec3};
use nge_ecs::Time;

use crate::animator::{Pose, padded};
use crate::assets::Assets3d;
use crate::camera::Transform3d;
use crate::renderer::Model3d;

/// 程序化骨骼注视已叠加进 `Pose` 的 `PostUpdate` 系统集（位于动画之后、`SocketsUpdated` 之前）。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct LookAtApplied;

/// 挂在 3D 模型实体上，让指定骨骼平滑朝向 `target` 世界坐标。
#[derive(Component, Debug, Clone, PartialEq)]
#[require(Pose)]
pub struct BoneLookAt {
    /// glTF 骨骼/节点名（如 `"head"`、`"body"`、`"armR"`）。
    pub bone: String,
    /// 注视的世界坐标目标。
    pub target: Vec3,
    /// 骨骼本地空间里的"正前方"单位向量（默认 `-Z`，符合 glTF / nge 角色朝向）。
    pub forward: Vec3,
    /// 混合权重 `0..=1`（默认 1.0）。
    pub weight: f32,
    /// 最大偏转角（弧度，默认 60°）。
    pub max_angle: f32,
    /// 转向平滑速度（弧度 / 秒，默认 8.0；`<= 0` 时瞬间对准）。
    pub speed: f32,
    /// 上一帧平滑后的本地叠加旋转。
    pub(crate) current: Quat,
}

impl BoneLookAt {
    pub fn new(bone: impl Into<String>, target: Vec3) -> Self {
        Self {
            bone: bone.into(),
            target,
            forward: Vec3::NEG_Z,
            weight: 1.0,
            max_angle: std::f32::consts::FRAC_PI_3,
            speed: 8.0,
            current: Quat::IDENTITY,
        }
    }
}

/// 判断 `node` 是否是 `ancestor` 的子孙节点。
fn is_descendant(nodes: &[crate::animation::Node], mut node: usize, ancestor: usize) -> bool {
    let mut hops = 0;
    while let Some(parent) = nodes.get(node).and_then(|n| n.parent) {
        let parent = parent as usize;
        if parent == ancestor {
            return true;
        }
        node = parent;
        hops += 1;
        if hops > nodes.len() {
            break;
        }
    }
    false
}

/// `PostUpdate`（在 `animate` / `animate_graphs` 之后、`SocketsUpdated` 之前）：
/// 在当前动画姿势上叠加骨骼注视旋转，并传播给该骨骼的所有子孙节点。
pub(crate) fn apply_bone_look_at(
    time: Res<Time>,
    assets: Res<Assets3d>,
    mut models: Query<(&Model3d, &Transform3d, &mut BoneLookAt, &mut Pose, Has<crate::Animator>, Has<nge_anim::AnimGraph>)>,
) {
    let dt = time.delta.min(0.1);
    for (model_id, entity_t, mut look, mut pose, has_anim, has_graph) in &mut models {
        let Some(model) = assets.model(model_id.0) else { continue };
        let skeleton = &model.skeleton;
        let Some(idx) = skeleton.names.iter().position(|n| n == &look.bone) else { continue };

        // 没有动画系统驱动时，每帧从 rest pose 开始叠加，避免旋转逐帧累积
        if !(has_anim || has_graph) || pose.nodes.len() != skeleton.nodes.len() {
            pose.nodes = skeleton.rest();
        }
        if pose.weights.len() != skeleton.nodes.len() {
            pose.weights.clear();
            pose.weights.resize(skeleton.nodes.len(), [0.0; crate::mesh::MAX_MORPH_TARGETS]);
            for part in &model.parts {
                if let Some(w) = part.node.and_then(|n| pose.weights.get_mut(n as usize)) {
                    *w = padded(&part.weights);
                }
            }
        }

        let old_local = pose.nodes[idx];
        let bone_world = entity_t.matrix() * old_local;
        let inv_world = bone_world.inverse();
        let desired = if look.target.is_finite() && inv_world.is_finite() {
            let local_dir = inv_world.transform_point3(look.target).try_normalize();
            let fwd = look.forward.try_normalize().unwrap_or(Vec3::NEG_Z);
            match local_dir {
                Some(dir) => {
                    let mut rot = Quat::from_rotation_arc(fwd, dir);
                    let max_a = look.max_angle.clamp(0.0, std::f32::consts::PI);
                    let angle = Quat::IDENTITY.angle_between(rot);
                    if angle > max_a && angle > 1e-5 {
                        rot = Quat::IDENTITY.slerp(rot, max_a / angle);
                    }
                    let w = look.weight.clamp(0.0, 1.0);
                    Quat::IDENTITY.slerp(rot, w)
                }
                None => Quat::IDENTITY,
            }
        } else {
            Quat::IDENTITY
        };

        let alpha = if look.speed > 0.0 && dt > 0.0 { 1.0 - (-look.speed * dt).exp() } else { 1.0 };
        look.current = look.current.slerp(desired, alpha).normalize();

        let delta = Mat4::from_quat(look.current);
        let new_local = old_local * delta;
        pose.nodes[idx] = new_local;

        // 把叠加旋转传播到所有子孙节点，保证头盔/手臂/挂点联动
        let inv_old = old_local.inverse();
        if inv_old.is_finite() {
            for c in 0..skeleton.nodes.len() {
                if c != idx && is_descendant(&skeleton.nodes, c, idx) {
                    pose.nodes[c] = new_local * (inv_old * pose.nodes[c]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_4;

    use super::*;
    use crate::animation::{Node, Skeleton, Trs};
    use crate::{BoneSocket, MaterialId, Mesh, Model, Part, Render3dPlugin};

    #[test]
    fn bone_look_at_rotates_bone_toward_target_and_clamps_max_angle() {
        let mut app = nge_ecs::App::new();
        app.add_plugins(Render3dPlugin);
        let mesh = app.world_mut().resource_mut::<Assets3d>().add_mesh(Mesh::cube(1.0)).unwrap();
        let model = Model {
            parts: vec![Part { node: Some(0), ..Part::new(mesh, MaterialId::DEFAULT, Mat4::IDENTITY) }],
            skeleton: Skeleton {
                nodes: vec![
                    Node { parent: None, rest: Trs::default() },
                    Node { parent: Some(0), rest: Trs { translation: Vec3::new(0.0, 0.0, -2.0), ..Default::default() } },
                ],
                skins: vec![],
                names: vec!["head".into(), "visor".into()],
            },
            animations: vec![],
        };
        let model_id = app.world_mut().resource_mut::<Assets3d>().add_model(model).unwrap();
        // 目标在正右方 +X（需要转 90°），但 max_angle 限制为 45°
        let hero = app
            .world_mut()
            .spawn((
                Model3d(model_id),
                Transform3d::default(),
                BoneLookAt { max_angle: FRAC_PI_4, speed: 0.0, ..BoneLookAt::new("head", Vec3::new(10.0, 0.0, 0.0)) },
            ))
            .id();
        let hat = app.world_mut().spawn((Transform3d::default(), BoneSocket::new(hero, "visor"))).id();

        app.tick(nge_core::Viewport { size: [10.0, 10.0], pixels: [10, 10] }, 0.016);
        let pose = app.world().get::<Pose>(hero).unwrap();
        let (_, head_rot, _) = pose.nodes[0].to_scale_rotation_translation();
        let angle = Quat::IDENTITY.angle_between(head_rot);
        assert!((angle - FRAC_PI_4).abs() < 1e-3, "应被 max_angle 限制在 45°：{angle}");
        // 子骨骼 visor（原本在 (0, 0, -2)）向右转 45° 后 x 应为正（+sqrt(2)）
        let hat_t = *app.world().get::<Transform3d>(hat).unwrap();
        assert!(hat_t.translation.x > 1.3 && hat_t.translation.z < -1.3, "子骨骼与挂点应同步跟随旋转：{:?}", hat_t.translation);
    }
}
