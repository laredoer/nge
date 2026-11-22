//! 3D 屏幕拾取（`scene.pick3d` + `pick3d` 事件）：把鼠标/触控点击通过 `Camera3d::screen_to_ray` 转成世界射线，
//! 用 `Physics3d::raycast` 精确命中地形网格与 3D 刚体，返回模型 `id`、世界命中点、表面法线与距离。

use bevy_ecs::prelude::*;
use nge_core::EventKey;
use nge_ecs::{Input, Outbox, Screen};
use nge_physics3d::{Physics3d, QueryFilter};
use nge_render3d::{Camera3d, Transform3d};
use serde::{Deserialize, Serialize};

use crate::nodes::ModelNode;
use crate::protocol::Scene;
use crate::sync::{Declared, SceneId};
use crate::SceneCamera;

pub const PICK3D: EventKey<Pick3d> = EventKey::new("pick3d");

/// `scene.pick3d` 配置：可直接写 `true` 开启，也可写对象过滤排除模型、最大距离与分组。
#[derive(Debug, Clone, PartialEq)]
pub struct Pick3dNode {
    pub enabled: bool,
    pub max_distance: f32,
    pub exclude: Option<String>,
    pub sensors: bool,
    pub only: Option<Vec<i64>>,
}

impl Default for Pick3dNode {
    fn default() -> Self {
        Self { enabled: true, max_distance: 4000.0, exclude: None, sensors: false, only: None }
    }
}

fn yes() -> bool {
    true
}

fn default_max_distance() -> f32 {
    4000.0
}

impl<'de> Deserialize<'de> for Pick3dNode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Config {
            #[serde(default = "yes")]
            enabled: bool,
            #[serde(default = "default_max_distance")]
            max_distance: f32,
            #[serde(default)]
            exclude: Option<String>,
            #[serde(default)]
            sensors: bool,
            #[serde(default)]
            only: Option<Vec<i64>>,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Flag(bool),
            Config(Config),
        }
        match Raw::deserialize(d)? {
            Raw::Flag(enabled) => Ok(Self { enabled, ..Default::default() }),
            Raw::Config(c) => Ok(Self {
                enabled: c.enabled,
                max_distance: c.max_distance,
                exclude: c.exclude,
                sensors: c.sensors,
                only: c.only,
            }),
        }
    }
}

/// 3D 屏幕拾取命中事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pick3d {
    /// 命中的 3D 模型 id（若是非场景模型碰撞体则为 `null`）。
    pub id: Option<String>,
    /// 精确的 3D 世界命中坐标。
    pub point: [f32; 3],
    /// 命中点的世界表面法线。
    pub normal: [f32; 3],
    /// 射线从近平面到命中点的距离。
    pub distance: f32,
    /// 触发拾取的屏幕坐标（左上角原点，y 朝下）。
    pub screen: [f32; 2],
}

/// `PostUpdate`（在 `Physics3dStepped` 与 `Camera3dRigUpdated` 之后）：按当帧最终 3D 相机做射线拾取。
#[cfg_attr(not(feature = "physics3d"), allow(dead_code))]
pub(crate) fn pick3d(
    scene: Res<Scene>,
    input: Res<Input>,
    screen: Res<Screen>,
    physics: Res<Physics3d>,
    camera: Query<(&Camera3d, &Transform3d), With<SceneCamera>>,
    models: Query<(Entity, &SceneId), With<Declared<ModelNode>>>,
    mut outbox: ResMut<Outbox>,
) {
    let Some(cfg) = scene.pick3d.as_ref().filter(|c| c.enabled) else { return };
    if !(cfg.max_distance.is_finite() && cfg.max_distance > 0.0) {
        return;
    }
    let Some((cam, cam_t)) = camera.iter().next() else { return };

    let mut filter = QueryFilter::default();
    if let Some(name) = cfg.exclude.as_deref() {
        if let Some((e, _)) = models.iter().find(|(_, id)| id.0 == name) {
            filter = filter.exclude(e);
        }
    }
    if cfg.sensors {
        filter = filter.with_sensors();
    }
    if let Some(only) = &cfg.only {
        let mut mask = 0u32;
        for &g in only {
            if let Ok(bit) = u8::try_from(g) {
                if bit < 32 {
                    mask |= 1 << bit;
                }
            }
        }
        filter = filter.only(mask);
    }

    for p in input.pointers_pressed() {
        let Some((origin, dir)) = cam.screen_to_ray(cam_t, p.screen, screen.size) else { continue };
        let Some(hit) = physics.raycast(origin, dir, cfg.max_distance, filter) else { continue };
        let id = models.get(hit.entity).ok().map(|(_, s)| s.0.clone());
        outbox.send(
            PICK3D,
            &Pick3d {
                id,
                point: hit.point.to_array(),
                normal: hit.normal.to_array(),
                distance: hit.distance,
                screen: p.screen.to_array(),
            },
        );
    }
}

#[cfg(all(test, feature = "physics3d"))]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::{app, frame};

    fn picks(events: &[(String, serde_json::Value)]) -> Vec<Pick3d> {
        events.iter().filter(|(n, _)| n == "pick3d").map(|(_, d)| PICK3D.decode(d).unwrap()).collect()
    }

    #[test]
    fn pick3d_raycasts_colliders_and_respects_exclude() {
        let mut app = app();
        let scene = |pick: serde_json::Value| {
            json!({
                "pick3d": pick,
                "physics3d": { "gravity": [0, 0, 0] },
                "camera3d": { "x": 0, "y": 20, "z": 100, "target": [0, 0, 0], "fov": 60 },
                "models": [
                    { "id": "hero", "model": "asset:/@t/crate.glb", "x": 0, "y": 10, "z": 50, "body": { "kind": "fixed", "size": [20, 20, 20] } },
                    { "id": "ground", "model": "asset:/@t/crate.glb", "x": 0, "y": -10, "z": 0, "body": { "kind": "fixed", "size": [400, 20, 400] } },
                ]
            })
        };
        frame(&mut app, Some(scene(json!(true))));
        // 点击视口正中：射线先命中挡在前面的 hero
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 50, "y": 50 }));
        let hits = picks(&frame(&mut app, None));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id.as_deref(), Some("hero"));
        assert!(hits[0].distance > 0.0);

        // 排除 hero 后再点同一位置，应穿透命中下面的 ground
        frame(&mut app, Some(scene(json!({ "exclude": "hero", "maxDistance": 500 }))));
        app.command("touch", &json!({ "phase": "end", "id": 1, "x": 50, "y": 50 }));
        app.command("touch", &json!({ "phase": "start", "id": 2, "x": 50, "y": 50 }));
        let hits = picks(&frame(&mut app, None));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id.as_deref(), Some("ground"));
        assert!((hits[0].normal[1] - 1.0).abs() < 1e-2, "地面法线应朝上 {:?}", hits[0].normal);
    }
}
