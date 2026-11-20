//! `scene.perf`：性能面板、曲线、每次汇总发给 TSX 的 `perf` 事件（见 docs/perf-overlay.md、docs/big-scene-perf.md）。

use bevy_ecs::prelude::*;
use nge_ecs::{EventKey, Outbox, Perf, PerfReport};
use nge_text::{PerfGraph, PerfOverlay};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::lenient::Fields;
use crate::protocol::Scene;

/// 每半秒一次的汇总（开了 `perf.events` 时）。
pub const PERF: EventKey<PerfEvent> = EventKey::new("perf");

/// `perf: true` 等于 `{ panel: true }`；对象里每个字段单独解析，写错的当没写。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PerfNode {
    pub panel: bool,
    pub graph: bool,
    pub events: bool,
}

impl<'de> Deserialize<'de> for PerfNode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::Bool(panel) => Ok(Self { panel, ..Self::default() }),
            other => {
                let mut f = Fields::new("perf", other).map_err(serde::de::Error::custom)?;
                let node = Self { panel: f.get("panel").unwrap_or(false), graph: f.get("graph").unwrap_or(false), events: f.get("events").unwrap_or(false) };
                f.finish();
                Ok(node)
            }
        }
    }
}

/// 资源：开着时每次汇总发 `perf` 事件。
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PerfEvents(pub bool);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfEvent {
    pub fps: f32,
    pub frame_ms: f32,
    pub worst_ms: f32,
    pub update_ms: f32,
    pub render_ms: f32,
    pub wait_ms: f32,
    /// 不支持、还没读回时是 `null`。
    pub gpu_ms: Option<f32>,
    pub gpu_supported: bool,
    /// pass 太多没量全：`gpuMs` 是下限。
    pub gpu_truncated: bool,
    pub draws: u32,
    pub instances: u32,
    pub entities: u32,
    pub particles: u32,
    /// `[画了, 一共]`。
    pub chunks: [u32; 2],
    pub tiles: u32,
    pub sprites: [u32; 2],
    pub meshes: [u32; 2],
}

impl From<&PerfReport> for PerfEvent {
    fn from(r: &PerfReport) -> Self {
        let c = r.cull;
        Self {
            fps: r.fps,
            frame_ms: r.frame_ms,
            worst_ms: r.worst_ms,
            update_ms: r.update_ms,
            render_ms: r.render_ms,
            wait_ms: r.wait_ms,
            gpu_ms: r.gpu_ms,
            gpu_supported: r.gpu_supported,
            gpu_truncated: r.gpu_truncated,
            draws: r.draws,
            instances: r.instances,
            entities: r.entities,
            particles: r.particles,
            chunks: c.chunks,
            tiles: c.tiles,
            sprites: c.sprites,
            meshes: c.meshes,
        }
    }
}

/// 声明变了才写开关，Rust 自己开关的不会被每帧覆盖；声明去掉时都关掉。一开始当作没写：
/// Rust 启动时打开的面板不会被第一帧没写 `perf` 的场景关掉。
pub(crate) fn sync_perf(scene: Res<Scene>, mut overlay: ResMut<PerfOverlay>, mut graph: ResMut<PerfGraph>, mut events: ResMut<PerfEvents>, mut last: Local<Option<PerfNode>>) {
    if *last == scene.perf {
        return;
    }
    *last = scene.perf;
    let node = scene.perf.unwrap_or_default();
    overlay.0 = node.panel;
    graph.0 = node.graph;
    events.0 = node.events;
}

/// 出了新的汇总就发一次。
pub(crate) fn send_perf(events: Res<PerfEvents>, perf: Res<Perf>, mut outbox: ResMut<Outbox>, mut sent: Local<Option<PerfReport>>) {
    let Some(report) = perf.report() else { return };
    if !events.0 || *sent == Some(*report) {
        return;
    }
    *sent = Some(*report);
    outbox.send(PERF, &PerfEvent::from(report));
}

#[cfg(test)]
mod tests {
    use nge_ecs::FrameTiming;
    use serde_json::json;

    use super::*;
    use crate::testing::{app, frame};

    #[test]
    fn perf_panel_follows_the_scene_only_when_the_declaration_changes() {
        let mut app = app();
        let on = |app: &nge_ecs::App| app.world().resource::<PerfOverlay>().0;
        frame(&mut app, Some(json!({ "sprites": [] })));
        assert!(!on(&app));
        frame(&mut app, Some(json!({ "perf": true })));
        assert!(on(&app));
        // Rust 关掉后，场景没改就不再打开
        app.world_mut().resource_mut::<PerfOverlay>().0 = false;
        frame(&mut app, Some(json!({ "perf": true })));
        assert!(!on(&app));
        frame(&mut app, Some(json!({ "perf": false })));
        frame(&mut app, Some(json!({ "perf": true })));
        assert!(on(&app));
        frame(&mut app, Some(json!({ "sprites": [] })));
        assert!(!on(&app), "去掉声明就关");
    }

    #[test]
    fn a_panel_rust_opened_survives_a_first_scene_without_perf() {
        let mut app = app();
        app.world_mut().resource_mut::<PerfOverlay>().0 = true;
        app.world_mut().resource_mut::<PerfGraph>().0 = true;
        frame(&mut app, Some(json!({ "sprites": [] })));
        assert!(app.world().resource::<PerfOverlay>().0 && app.world().resource::<PerfGraph>().0);
    }

    #[test]
    fn perf_objects_switch_each_part_and_bad_fields_are_dropped() {
        let mut app = app();
        let state = |app: &nge_ecs::App| (app.world().resource::<PerfOverlay>().0, app.world().resource::<PerfGraph>().0, app.world().resource::<PerfEvents>().0);
        frame(&mut app, Some(json!({ "perf": { "graph": true, "events": true } })));
        assert_eq!(state(&app), (false, true, true));
        frame(&mut app, Some(json!({ "perf": { "panel": true, "graph": "yes", "what": 1 } })));
        assert_eq!(state(&app), (true, false, false), "写错的字段当没写，别的照常");
        frame(&mut app, Some(json!({ "perf": 3 })));
        assert_eq!(state(&app), (false, false, false), "整个写错当没写");
    }

    #[test]
    fn the_perf_event_goes_out_once_per_report_while_events_are_on() {
        let mut app = app();
        frame(&mut app, Some(json!({ "perf": { "events": true } })));
        let timing = FrameTiming { frame: 0.6, update: 0.001, render: 0.002, wait: 0.5 };
        app.world_mut().resource_mut::<Perf>().record(timing, 3, 10, || 7);
        let events = frame(&mut app, None);
        let sent: Vec<&Value> = events.iter().filter(|(n, _)| n == "perf").map(|(_, d)| d).collect();
        assert_eq!(sent.len(), 1);
        assert_eq!((sent[0]["draws"].as_u64(), sent[0]["entities"].as_u64(), sent[0]["gpuSupported"].as_bool()), (Some(3), Some(7), Some(false)));
        assert!(sent[0]["gpuMs"].is_null() && sent[0]["chunks"] == json!([0, 0]));
        assert!(frame(&mut app, None).iter().all(|(n, _)| n != "perf"), "同一份汇总不重发");
        frame(&mut app, Some(json!({ "perf": { "events": false } })));
        app.world_mut().resource_mut::<Perf>().record(timing, 4, 10, || 7);
        assert!(frame(&mut app, None).iter().all(|(n, _)| n != "perf"));
    }
}
