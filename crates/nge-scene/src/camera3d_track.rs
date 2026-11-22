//! 3D 过场运镜轨道（`camera3dTrack` 命令 + `camera3dTrackEnd` 事件）：
//! 支持多段 3D 关键帧（位置、注视点、可选 FOV 与缓动曲线），在 `Camera3dRigUpdated` 内接管 3D 相机，
//! 播完后可按 `returnDuration` 平滑归位到当前实时玩法相机，避免硬切跳变。

use bevy_ecs::prelude::*;
use glam::Vec3;
use nge_core::{CommandKey, EventKey};
use nge_ecs::{Outbox, Time};
use nge_render3d::{Camera3d, Transform3d};
use serde::{Deserialize, Serialize};

use crate::SceneCamera;

pub const CAMERA3D_TRACK: CommandKey<Camera3dTrack> = CommandKey::new("camera3dTrack");
pub const CAMERA3D_TRACK_END: EventKey<Camera3dTrackEnd> = EventKey::new("camera3dTrackEnd");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackEase {
    Linear,
    EaseIn,
    EaseOut,
    #[default]
    EaseInOut,
}

impl TrackEase {
    fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EaseIn => t * t,
            Self::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Self::EaseInOut => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// 3D 运镜轨道的单个关键帧。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Camera3dKeyframe {
    /// 相机世界坐标 `[x, y, z]`。
    pub pos: [f32; 3],
    /// 相机注视点 `[x, y, z]`。
    pub target: [f32; 3],
    /// 从上一个状态过渡到本关键帧耗费的秒数（第一帧写 0 表示直接切入，>0 表示从当前相机平滑过渡过去）。
    #[serde(default)]
    pub duration: f32,
    /// 可选垂直视角（度），不写沿用上一帧 / 当前相机视角。
    pub fov: Option<f32>,
    /// 缓动曲线，默认 `"easeInOut"`。
    #[serde(default)]
    pub ease: TrackEase,
}

/// `camera3dTrack` 命令：播放一段 3D 过场运镜，或用 `stop: true` 提前终止。
#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Camera3dTrack {
    #[serde(default)]
    pub keyframes: Vec<Camera3dKeyframe>,
    /// 原样带回 `camera3dTrackEnd` 事件。
    pub tag: Option<String>,
    /// 播完关键帧后平滑归位到玩法相机的秒数（默认 0.6；0 表示立刻归还控制权）。
    #[serde(default = "default_return")]
    pub return_duration: f32,
    /// 设为 true 时立即停止当前运镜。
    #[serde(default)]
    pub stop: bool,
}

fn default_return() -> f32 {
    0.6
}

/// `camera3dTrackEnd` 事件：运镜播完或被提前取消时触发。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Camera3dTrackEnd {
    pub tag: Option<String>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Copy)]
struct Pose {
    pos: Vec3,
    target: Vec3,
    fov_y: f32,
}

impl Pose {
    fn to_transform(self) -> Transform3d {
        let dir = (self.target - self.pos).normalize_or_zero();
        let up = if dir.cross(Vec3::Y).length_squared() < 1e-8 { Vec3::NEG_Z } else { Vec3::Y };
        Transform3d::from_xyz(self.pos.x, self.pos.y, self.pos.z).looking_at(self.target, up)
    }
}

#[derive(Debug, Clone)]
enum Phase {
    Playing {
        index: usize,
        from: Pose,
        elapsed: f32,
    },
    Returning {
        from: Pose,
        elapsed: f32,
        duration: f32,
    },
}

#[derive(Debug, Clone)]
struct ActiveTrack {
    tag: Option<String>,
    keyframes: Vec<Camera3dKeyframe>,
    return_duration: f32,
    phase: Phase,
    last_pose: Pose,
}

/// 当前正在播放的 3D 过场运镜状态。
#[derive(Resource, Debug, Default)]
pub(crate) struct Camera3dTrackState {
    active: Option<ActiveTrack>,
}

fn keyframe_pose(kf: &Camera3dKeyframe, fallback_fov: f32) -> Pose {
    let fov_y = kf.fov.filter(|v| v.is_finite() && *v > 1.0 && *v < 175.0).map(f32::to_radians).unwrap_or(fallback_fov);
    Pose { pos: Vec3::from(kf.pos), target: Vec3::from(kf.target), fov_y }
}

fn current_pose(cam: &Camera3d, cam_t: &Transform3d) -> Pose {
    let fwd = (cam_t.rotation * Vec3::NEG_Z).normalize_or_zero();
    let dir = if fwd == Vec3::ZERO { Vec3::NEG_Z } else { fwd };
    Pose { pos: cam_t.translation, target: cam_t.translation + dir * 120.0, fov_y: cam.fov_y }
}

/// `PostUpdate`（在 `Camera3dRigUpdated` 内、`camera3d_rig` 之后）：驱动 3D 过场运镜与平滑归位。
pub(crate) fn step_camera3d_track(
    time: Res<Time>,
    mut state: ResMut<Camera3dTrackState>,
    mut commands: MessageReader<Camera3dTrack>,
    mut outbox: ResMut<Outbox>,
    mut camera: Query<(&mut Camera3d, &mut Transform3d), With<SceneCamera>>,
) {
    let Ok((mut cam, mut cam_t)) = camera.single_mut() else { return };

    for cmd in commands.read() {
        if cmd.stop {
            if let Some(old) = state.active.take() {
                outbox.send(CAMERA3D_TRACK_END, &Camera3dTrackEnd { tag: old.tag, cancelled: true });
            }
            continue;
        }
        if cmd.keyframes.is_empty() {
            continue;
        }
        let start_pose = match state.active.take() {
            Some(old) => {
                outbox.send(CAMERA3D_TRACK_END, &Camera3dTrackEnd { tag: old.tag, cancelled: true });
                old.last_pose
            }
            None => current_pose(&cam, &cam_t),
        };
        let return_duration = if cmd.return_duration.is_finite() && cmd.return_duration > 0.0 { cmd.return_duration } else { 0.0 };
        state.active = Some(ActiveTrack {
            tag: cmd.tag.clone(),
            keyframes: cmd.keyframes.clone(),
            return_duration,
            phase: Phase::Playing { index: 0, from: start_pose, elapsed: 0.0 },
            last_pose: start_pose,
        });
    }

    let Some(active) = state.active.as_mut() else { return };
    let dt = time.delta.max(0.0).min(0.1);
    let mut finished = false;
    let mut just_entered_returning = false;

    match &mut active.phase {
        Phase::Playing { index, from, elapsed } => {
            *elapsed += dt;
            loop {
                let kf = &active.keyframes[*index];
                let dur = if kf.duration.is_finite() && kf.duration > 0.0 { kf.duration } else { 0.0 };
                let to = keyframe_pose(kf, from.fov_y);
                if dur <= 0.0 || *elapsed >= dur {
                    let overflow = (*elapsed - dur).max(0.0);
                    active.last_pose = to;
                    if *index + 1 < active.keyframes.len() {
                        *index += 1;
                        *from = to;
                        *elapsed = overflow;
                        continue;
                    }
                    // 所有关键帧播放完毕：进入平滑归位或直接结束
                    if active.return_duration > 0.0 {
                        active.phase = Phase::Returning { from: to, elapsed: overflow, duration: active.return_duration };
                        just_entered_returning = true;
                    } else {
                        *cam_t = to.to_transform();
                        cam.fov_y = to.fov_y;
                        finished = true;
                    }
                    break;
                }
                let e = kf.ease.apply(*elapsed / dur);
                let pose = Pose {
                    pos: from.pos.lerp(to.pos, e),
                    target: from.target.lerp(to.target, e),
                    fov_y: from.fov_y + (to.fov_y - from.fov_y) * e,
                };
                active.last_pose = pose;
                *cam_t = pose.to_transform();
                cam.fov_y = pose.fov_y;
                break;
            }
        }
        Phase::Returning { from, elapsed, duration } => {
            *elapsed += dt;
            let live_pos = cam_t.translation;
            let live_rot = cam_t.rotation;
            let live_fov = cam.fov_y;
            if *elapsed >= *duration {
                finished = true;
            } else {
                let e = TrackEase::EaseInOut.apply(*elapsed / *duration);
                let from_t = from.to_transform();
                cam_t.translation = from_t.translation.lerp(live_pos, e);
                cam_t.rotation = from_t.rotation.slerp(live_rot, e);
                cam.fov_y = from.fov_y + (live_fov - from.fov_y) * e;
                active.last_pose = current_pose(&cam, &cam_t);
            }
        }
    }

    // 若本帧刚从 Playing 切入 Returning，立刻按当前进度写入过渡姿态（避免跳帧）
    if just_entered_returning && !finished {
        if let Phase::Returning { from, elapsed, duration } = &active.phase {
            if *elapsed < *duration {
                let e = TrackEase::EaseInOut.apply(*elapsed / *duration);
                let from_t = from.to_transform();
                let live_pos = cam_t.translation;
                let live_rot = cam_t.rotation;
                let live_fov = cam.fov_y;
                cam_t.translation = from_t.translation.lerp(live_pos, e);
                cam_t.rotation = from_t.rotation.slerp(live_rot, e);
                cam.fov_y = from.fov_y + (live_fov - from.fov_y) * e;
                active.last_pose = current_pose(&cam, &cam_t);
            } else {
                finished = true;
            }
        }
    }

    if finished {
        if let Some(done) = state.active.take() {
            outbox.send(CAMERA3D_TRACK_END, &Camera3dTrackEnd { tag: done.tag, cancelled: false });
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::{VIEWPORT, app, frame};

    fn cam_pos(app: &mut nge_ecs::App) -> Vec3 {
        let world = app.world_mut();
        world.query_filtered::<&Transform3d, (With<SceneCamera>, With<Camera3d>)>().single(world).unwrap().translation
    }

    fn ends(events: &[(String, serde_json::Value)]) -> Vec<Camera3dTrackEnd> {
        events.iter().filter(|(n, _)| n == "camera3dTrackEnd").map(|(_, d)| CAMERA3D_TRACK_END.decode(d).unwrap()).collect()
    }

    #[test]
    fn camera3d_track_interpolates_keyframes_and_emits_end() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera3d": { "x": 0, "y": 10, "z": 50 } })));
        app.command("camera3dTrack", &json!({
            "tag": "intro",
            "returnDuration": 0.0,
            "keyframes": [
                { "pos": [100, 20, 50], "target": [0, 0, 0], "duration": 0.0 },
                { "pos": [200, 20, 50], "target": [0, 0, 0], "duration": 0.2, "ease": "linear" }
            ]
        }));
        app.tick(VIEWPORT, 0.1);
        let mid = cam_pos(&mut app);
        assert!((mid.x - 150.0).abs() < 1.0, "线性插值到一半应在 x=150：{mid:?}");

        let events = app.tick(VIEWPORT, 0.12);
        let done = ends(&events);
        assert_eq!(done, [Camera3dTrackEnd { tag: Some("intro".into()), cancelled: false }]);
    }

    #[test]
    fn camera3d_track_stop_cancels_active_track() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera3d": { "x": 0, "y": 10, "z": 50 } })));
        app.command("camera3dTrack", &json!({
            "tag": "shrine",
            "keyframes": [{ "pos": [50, 30, 50], "target": [0, 0, 0], "duration": 1.0 }]
        }));
        app.tick(VIEWPORT, 0.1);
        app.command("camera3dTrack", &json!({ "stop": true }));
        let done = ends(&app.tick(VIEWPORT, 0.05));
        assert_eq!(done, [Camera3dTrackEnd { tag: Some("shrine".into()), cancelled: true }]);
    }
}
