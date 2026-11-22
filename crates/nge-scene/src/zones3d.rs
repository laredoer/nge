//! 3D 区域触发器与体积感应（`scene.zones3d` + `zone3d` 事件）：
//! 支持球形（`sphere`）与轴对齐盒形（`box`）3D 触发区，零物理碰撞体开销，
//! 支持按 `watch` 监听指定 3D 模型进出并派发 `zone3d` 进入/离开事件，可选 `Gizmos3d` 边界可视化。

use std::collections::HashSet;

use bevy_ecs::prelude::*;
use glam::{Vec3, Vec4};
use nge_core::EventKey;
use nge_ecs::Outbox;
use nge_render3d::{Gizmos3d, Transform3d};
use serde::{Deserialize, Serialize};

use crate::nodes::ModelNode;
use crate::protocol::Scene;
use crate::sync::{Declared, SceneId};

pub const ZONE3D: EventKey<Zone3dEvent> = EventKey::new("zone3d");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Zone3dShape {
    #[default]
    Sphere,
    Box,
}

/// `scene.zones3d` 的一项：3D 球形或盒形触发区。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Zone3dNode {
    pub id: String,
    #[serde(default)]
    pub shape: Zone3dShape,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub z: f32,
    /// 球形触发区半径（默认 80）。
    #[serde(default = "default_radius")]
    pub r: f32,
    /// 盒形触发区全尺寸 `[w, h, d]`（默认 `[100, 100, 100]`）。
    #[serde(default = "default_size")]
    pub size: [f32; 3],
    /// 仅监听这些 3D 模型 id；不写时监听所有已声明的 3D 模型。
    pub watch: Option<Vec<String>>,
    /// 是否用 `Gizmos3d` 绘制触发区边界线框（默认 false）。
    #[serde(default)]
    pub debug: bool,
}

fn default_radius() -> f32 {
    80.0
}

fn default_size() -> [f32; 3] {
    [100.0, 100.0, 100.0]
}

/// 3D 模型进入或离开触发区时派发的事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Zone3dEvent {
    pub zone: String,
    pub model: String,
    pub entered: bool,
}

/// 记录当前位于各 3D 触发区内的 `(zone_id, model_id)` 集合。
#[derive(Resource, Debug, Default)]
pub(crate) struct Zones3dState {
    inside: HashSet<(String, String)>,
}

fn contains(zone: &Zone3dNode, pos: Vec3) -> bool {
    let center = Vec3::new(zone.x, zone.y, zone.z);
    if !center.is_finite() || !pos.is_finite() {
        return false;
    }
    match zone.shape {
        Zone3dShape::Sphere => zone.r.is_finite() && zone.r > 0.0 && (pos - center).length_squared() <= zone.r * zone.r,
        Zone3dShape::Box => {
            let half = Vec3::from(zone.size) * 0.5;
            half.is_finite() && half.cmpgt(Vec3::ZERO).all() && (pos - center).abs().cmple(half).all()
        }
    }
}

fn draw_debug_zone(gizmos: &mut Gizmos3d, zone: &Zone3dNode, active: bool) {
    let center = Vec3::new(zone.x, zone.y, zone.z);
    let color = if active { Vec4::new(0.25, 0.96, 1.0, 0.9) } else { Vec4::new(1.0, 0.82, 0.32, 0.55) };
    let g = gizmos.on_top();
    match zone.shape {
        Zone3dShape::Sphere => {
            if zone.r.is_finite() && zone.r > 0.0 {
                g.circle(center + Vec3::Y * 4.0, Vec3::Y, zone.r, color);
            }
        }
        Zone3dShape::Box => {
            let h = Vec3::from(zone.size) * 0.5;
            if !(h.is_finite() && h.cmpgt(Vec3::ZERO).all()) {
                return;
            }
            let c = |sx: f32, sy: f32, sz: f32| center + Vec3::new(sx * h.x, sy * h.y, sz * h.z);
            let b = [c(-1.0, -1.0, -1.0), c(1.0, -1.0, -1.0), c(1.0, -1.0, 1.0), c(-1.0, -1.0, 1.0)];
            let t = [c(-1.0, 1.0, -1.0), c(1.0, 1.0, -1.0), c(1.0, 1.0, 1.0), c(-1.0, 1.0, 1.0)];
            for i in 0..4 {
                let j = (i + 1) % 4;
                g.line(b[i], b[j], color);
                g.line(t[i], t[j], color);
                g.line(b[i], t[i], color);
            }
        }
    }
}

/// `PostUpdate`（在物理、挂点之后）：检测 3D 模型进出触发区并派发 `zone3d` 事件。
pub(crate) fn check_zones3d(
    scene: Res<Scene>,
    mut state: ResMut<Zones3dState>,
    mut gizmos: ResMut<Gizmos3d>,
    models: Query<(&SceneId, &Transform3d), With<Declared<ModelNode>>>,
    mut outbox: ResMut<Outbox>,
) {
    if scene.zones3d.is_empty() && state.inside.is_empty() {
        return;
    }

    let mut next = HashSet::new();
    for zone in &scene.zones3d {
        let mut any_inside = false;
        for (model_id, transform) in &models {
            if let Some(watch) = &zone.watch {
                if !watch.iter().any(|w| w == &model_id.0) {
                    continue;
                }
            }
            if contains(zone, transform.translation) {
                any_inside = true;
                next.insert((zone.id.clone(), model_id.0.clone()));
            }
        }
        if zone.debug {
            draw_debug_zone(&mut gizmos, zone, any_inside);
        }
    }

    let mut entered: Vec<_> = next.difference(&state.inside).cloned().collect();
    entered.sort();
    for (zone, model) in entered {
        outbox.send(ZONE3D, &Zone3dEvent { zone, model, entered: true });
    }

    let mut exited: Vec<_> = state.inside.difference(&next).cloned().collect();
    exited.sort();
    for (zone, model) in exited {
        outbox.send(ZONE3D, &Zone3dEvent { zone, model, entered: false });
    }

    state.inside = next;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::{app, frame};

    fn zone_events(events: &[(String, serde_json::Value)]) -> Vec<Zone3dEvent> {
        events.iter().filter(|(n, _)| n == "zone3d").map(|(_, d)| ZONE3D.decode(d).unwrap()).collect()
    }

    #[test]
    fn zones3d_emit_enter_and_exit_events_for_sphere_and_box() {
        let mut app = app();
        let scene = |hx: f32| {
            json!({
                "models": [
                    { "id": "hero", "model": "asset:/@t/crate.glb", "x": hx, "y": 0, "z": 0 },
                    { "id": "npc", "model": "asset:/@t/crate.glb", "x": 0, "y": 0, "z": 0 },
                ],
                "zones3d": [
                    { "id": "camp", "shape": "sphere", "x": 0, "y": 0, "z": 0, "r": 50, "watch": ["hero"], "debug": true },
                    { "id": "gate", "shape": "box", "x": 100, "y": 0, "z": 0, "size": [40, 40, 40], "watch": ["hero"] },
                ]
            })
        };

        // 初始在 camp 内：仅监听 hero，不触发 npc
        let e1 = zone_events(&frame(&mut app, Some(scene(10.0))));
        assert_eq!(e1, [Zone3dEvent { zone: "camp".into(), model: "hero".into(), entered: true }]);

        // 停留在原地不重复触发
        assert!(zone_events(&frame(&mut app, None)).is_empty());

        // 移动到 gate 盒形区内：同时触发离开 camp 与进入 gate
        let e2 = zone_events(&frame(&mut app, Some(scene(100.0))));
        assert_eq!(
            e2,
            [
                Zone3dEvent { zone: "gate".into(), model: "hero".into(), entered: true },
                Zone3dEvent { zone: "camp".into(), model: "hero".into(), entered: false },
            ]
        );
    }
}
