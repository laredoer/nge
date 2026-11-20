//! 3D 弹簧臂防穿墙跟随相机（`camera3d.follow`）与 3D 震屏（复用 `shake` 命令）。

use bevy_ecs::prelude::*;
use glam::Vec3;
use nge_ecs::Time;
use nge_physics3d::{Physics3d, QueryFilter, QueryShape};
use nge_render3d::{Camera3d, Transform3d};

use crate::SceneCamera;
use crate::camera::Shake;
use crate::nodes::{Camera3dNode, ModelNode};
use crate::sync::{Declared, SceneId};

/// 3D 相机跟随与震屏已写入的 `PostUpdate` 系统集；自定义 Rust 相机排在它之前即可自动叠加 3D 震屏。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Camera3dRigUpdated;

#[derive(Debug, Clone, Copy)]
struct Shaking3d {
    strength: f32,
    duration: f32,
    left: f32,
}

impl Shaking3d {
    fn amplitude(&self) -> f32 {
        self.strength * (self.left / self.duration).clamp(0.0, 1.0)
    }
}

/// 3D 相机跟随与震屏状态；不跟随时只叠加震屏偏移，绝不覆盖 Rust 自驱动的 3D 相机。
#[derive(Resource, Debug, Default)]
pub(crate) struct Camera3dRig {
    node: Camera3dNode,
    tracking: bool,
    current_pos: Vec3,
    current_target: Vec3,
    applied_shake: Vec3,
    written_pos: Vec3,
    shake: Option<Shaking3d>,
}

impl Camera3dRig {
    pub(crate) fn set_node(&mut self, node: Camera3dNode) {
        if node.follow != self.node.follow {
            self.tracking = false;
        }
        self.node = node;
    }

    fn unshaken(&self, pos: Vec3) -> Vec3 {
        if pos == self.written_pos { pos - self.applied_shake } else { pos }
    }
}

fn shake_offset_3d(t: f32, amplitude: f32) -> Vec3 {
    let x = (t * 47.0).sin() + 0.5 * (t * 91.0 + 1.3).sin();
    let y = (t * 53.0 + 0.7).sin() + 0.5 * (t * 79.0).sin();
    let z = (t * 41.0 + 2.1).sin() + 0.5 * (t * 67.0).sin();
    (Vec3::new(x, y, z) / 1.5).clamp_length_max(1.0) * amplitude
}

/// `PostUpdate`（在 3D 物理与挂点之后）：执行 3D 弹簧臂平滑跟随、防穿墙球体投射与 3D 震屏。
pub(crate) fn camera3d_rig(
    time: Res<Time>,
    mut rig: ResMut<Camera3dRig>,
    mut shakes: MessageReader<Shake>,
    physics: Option<Res<Physics3d>>,
    models: Query<(Entity, &SceneId, &Transform3d), (With<Declared<ModelNode>>, Without<SceneCamera>)>,
    mut camera: Query<&mut Transform3d, (With<SceneCamera>, With<Camera3d>)>,
) {
    let rig = &mut *rig;
    for s in shakes.read() {
        let valid = |v: f32| v.is_finite() && v > 0.0;
        if !(valid(s.strength) && valid(s.duration)) {
            continue;
        }
        let new = Shaking3d { strength: s.strength, duration: s.duration, left: s.duration };
        if rig.shake.is_none_or(|old| new.amplitude() >= old.amplitude()) {
            rig.shake = Some(new);
        }
    }

    let Ok(mut cam_t) = camera.single_mut() else { return };
    let dt = time.delta.min(0.1);
    let mut base_pos = rig.unshaken(cam_t.translation);
    let mut moved = rig.applied_shake != Vec3::ZERO;

    if !rig.node.match2d {
        if let Some(target_id) = rig.node.follow.as_deref() {
            if let Some((target_entity, _, target_t)) = models.iter().find(|(_, id, _)| id.0 == target_id) {
                let pivot = target_t.translation + Vec3::Y * rig.node.lift;
                let yaw = match rig.node.yaw {
                    Some(deg) => deg.to_radians(),
                    None => {
                        let fwd = target_t.rotation * Vec3::NEG_Z;
                        if fwd.x * fwd.x + fwd.z * fwd.z > 1e-6 { (-fwd.x).atan2(-fwd.z) } else { 0.0 }
                    }
                };
                let pitch = rig.node.pitch.clamp(-85.0, 85.0).to_radians();
                let dir = Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos()).normalize();
                let mut dist = rig.node.distance.max(1.0);

                // 弹簧臂球体投射防穿墙：从注视点往相机方向扫，撞到地形/墙体时自动拉近
                if rig.node.collision {
                    if let Some(phys) = physics.as_deref() {
                        let filter = QueryFilter::default().exclude(target_entity);
                        if let Some(hit) = phys.cast_shape(QueryShape::Ball(4.0), pivot, glam::Quat::IDENTITY, dir, dist, filter) {
                            dist = (hit.distance - 2.0).max(8.0);
                        }
                    }
                }

                let want_pos = pivot + dir * dist;
                let k = if rig.tracking && rig.node.smooth > 0.0 { 1.0 - (-dt / rig.node.smooth).exp() } else { 1.0 };
                rig.current_pos = if rig.tracking { rig.current_pos.lerp(want_pos, k) } else { want_pos };
                rig.current_target = if rig.tracking { rig.current_target.lerp(pivot, k) } else { pivot };
                rig.tracking = true;
                base_pos = rig.current_pos;
                let up = if (rig.current_target - base_pos).normalize_or_zero().cross(Vec3::Y).length_squared() < 1e-8 { Vec3::NEG_Z } else { Vec3::Y };
                *cam_t = Transform3d::from_xyz(base_pos.x, base_pos.y, base_pos.z).looking_at(rig.current_target, up);
                moved = true;
            } else {
                rig.tracking = false;
            }
        } else {
            rig.tracking = false;
        }
    }

    let mut shake_off = Vec3::ZERO;
    if let Some(s) = &mut rig.shake {
        moved = true;
        shake_off = shake_offset_3d((time.elapsed % 1000.0) as f32, s.amplitude());
        s.left -= dt;
        if s.left <= 0.0 {
            rig.shake = None;
        }
    }

    if moved {
        cam_t.translation = base_pos + shake_off;
        rig.applied_shake = shake_off;
        rig.written_pos = cam_t.translation;
    }
}

#[cfg(all(test, feature = "physics3d"))]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::{VIEWPORT, app, frame};

    fn cam_pos(app: &mut nge_ecs::App) -> Vec3 {
        let world = app.world_mut();
        world.query_filtered::<&Transform3d, (With<SceneCamera>, With<Camera3d>)>().single(world).unwrap().translation
    }

    #[test]
    fn spring_arm_follow_tracks_target_and_avoids_wall_collision() {
        let mut app = app();
        // 目标在原点，背后 z = 40 处有一堵厚墙；相机要求距离 100、俯仰 0°、偏航 0°（正后方 +Z）
        let scene = json!({
            "camera3d": { "follow": "hero", "distance": 100, "pitch": 0, "yaw": 0, "lift": 0, "smooth": 0, "collision": true },
            "models": [
                { "id": "hero", "model": "asset:/@t/crate.glb", "x": 0, "y": 0, "z": 0 },
                { "id": "wall", "model": "asset:/@t/crate.glb", "x": 0, "y": 0, "z": 40, "body": { "kind": "fixed", "shape": "box", "size": [100, 100, 10] } },
            ]
        });
        frame(&mut app, Some(scene));
        frame(&mut app, None);
        let p = cam_pos(&mut app);
        assert!(p.z > 8.0 && p.z < 36.0, "防穿墙弹簧臂应把相机拉近到墙前：{p:?}");
    }

    #[test]
    fn shake_command_shakes_3d_camera_and_restores_position() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera3d": { "x": 0, "y": 10, "z": 50 } })));
        let base = cam_pos(&mut app);
        app.command("shake", &json!({ "strength": 10, "duration": 0.2 }));
        app.tick(VIEWPORT, 0.05);
        assert!(cam_pos(&mut app).distance(base) > 0.1, "3D 相机应在震屏中产生偏移");
        for _ in 0..8 {
            app.tick(VIEWPORT, 0.05);
        }
        assert!(cam_pos(&mut app).distance(base) < 1e-4, "震屏结束后应精确回到原位：{:?} vs {base:?}", cam_pos(&mut app));
    }
}
