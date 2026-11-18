//! `query3d` command / 事件：TSX 做 3D 空间查询（射线、形状投射、重叠、点重叠、最近点），按 `id` 回结果（见 docs/gameplay3d.md）。

use bevy_ecs::prelude::*;
use glam::{EulerRot, Quat, Vec3};
use nge_core::{CommandKey, EventKey};
use nge_ecs::Outbox;
use nge_physics3d::{Physics3d, QueryFilter, QueryShape};
use serde::{Deserialize, Serialize};

use crate::nodes::ModelNode;
use crate::sync::{Declared, SceneId};

/// 收到的是原样的 JSON：解析不了也能按 `id` 马上回一个带原因的空结果，不让 TSX 干等到超时。
pub const QUERY3D: CommandKey<Query3dCommand> = CommandKey::new("query3d");
pub const QUERY3D_RESULT: EventKey<Query3dResult> = EventKey::new("query3d");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Query3dKind {
    /// 射线：`from` 沿 `dir` 最远 `distance`。
    Ray,
    /// 形状投射：`shape` 放在 `from`，沿 `dir` 扫。
    Cast,
    /// 和 `shape`（放在 `from`）重叠的。
    Overlap,
    /// 包含 `from` 这个点的。
    Point,
    /// `distance` 以内离 `from` 最近的。
    Closest,
}

/// 查询的形状：`{ ball: r }`、`{ capsule: [中间高, 半径] }`、`{ box: [x, y, z] }`。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum Shape3dQuery {
    Ball(f32),
    Capsule([f32; 2]),
    Box([f32; 3]),
}

#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
#[serde(transparent)]
pub struct Query3dCommand(pub serde_json::Value);

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query3d {
    pub id: u64,
    pub kind: Query3dKind,
    pub from: [f32; 3],
    #[serde(default)]
    pub dir: Option<[f32; 3]>,
    /// 射线、投射默认 10000；最近点默认不限。
    #[serde(default)]
    pub distance: Option<f32>,
    #[serde(default)]
    pub shape: Option<Shape3dQuery>,
    /// 形状的角度（度，和模型的 `rotation` 一样）。
    #[serde(default)]
    pub rotation: Option<[f32; 3]>,
    /// 不算这个模型。
    #[serde(default)]
    pub exclude: Option<String>,
    /// sensor 也算。
    #[serde(default)]
    pub sensors: bool,
    /// 像这一组的碰撞体一样查：跳过同一组的（0 … 31；收宽的类型，超出范围回错误，不是整个解析失败）。
    #[serde(default)]
    pub group: i64,
    /// 只查这些分组（0 是不分组的，0 … 31）；不写时都查。
    #[serde(default)]
    pub only: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Hit3d {
    /// 模型的 id；不是模型的碰撞体（游戏在 Rust 里加的）是 `null`。
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub point: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub normal: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub distance: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub inside: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Query3dResult {
    pub id: u64,
    pub hits: Vec<Hit3d>,
    /// 查询本身不对（字段写错、分组超出 0 … 31、没写 shape 等）时的原因；这时 `hits` 是空的。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

const FAR: f32 = 10_000.0;

/// 物理这一步之后、调试视图画之前回答这一帧收到的查询（查询在同一帧画出来）。
pub(crate) fn answer(mut reader: MessageReader<Query3dCommand>, physics: Res<Physics3d>, models: Query<(Entity, &SceneId), With<Declared<ModelNode>>>, mut outbox: ResMut<Outbox>) {
    for raw in reader.read() {
        let Some(id) = raw.0.get("id").and_then(serde_json::Value::as_u64) else {
            log::warn!("query3d 没有 id（要是非负整数），回不了：{}", raw.0);
            continue;
        };
        let result = serde_json::from_value::<Query3d>(raw.0.clone()).map_err(|e| e.to_string()).and_then(|q| run(&q, &physics, &models));
        let reply = match result {
            Ok(hits) => Query3dResult { id, hits, error: None },
            Err(why) => {
                log::warn!("query3d {id}：{why}");
                Query3dResult { id, hits: Vec::new(), error: Some(why) }
            }
        };
        outbox.send(QUERY3D_RESULT, &reply);
    }
}

/// 分组要在 0 … 31。
fn group(g: i64) -> Result<u8, String> {
    u8::try_from(g).ok().filter(|g| *g < 32).ok_or_else(|| format!("分组 {g} 超出 0 … 31"))
}

fn run(q: &Query3d, physics: &Physics3d, models: &Query<(Entity, &SceneId), With<Declared<ModelNode>>>) -> Result<Vec<Hit3d>, String> {
    let id = |e: Entity| models.get(e).ok().map(|(_, s)| s.0.clone());
    let mut filter = QueryFilter::default().group(group(q.group)?);
    if let Some(name) = &q.exclude {
        let (e, _) = models.iter().find(|(_, s)| &s.0 == name).ok_or_else(|| format!("没有 id 为 {name:?} 的模型"))?;
        filter = filter.exclude(e);
    }
    if q.sensors {
        filter = filter.with_sensors();
    }
    if let Some(only) = &q.only {
        let mut mask = 0u32;
        for &g in only {
            mask |= 1 << group(g)?;
        }
        filter = filter.only(mask);
    }
    let from = Vec3::from(q.from);
    let dir = || q.dir.map(Vec3::from).ok_or("要写 dir");
    let shape = || {
        q.shape
            .map(|s| match s {
                Shape3dQuery::Ball(r) => QueryShape::Ball(r),
                Shape3dQuery::Capsule([h, r]) => QueryShape::Capsule { half_height: h / 2.0, radius: r },
                Shape3dQuery::Box(size) => QueryShape::Box(Vec3::from(size)),
            })
            .ok_or("要写 shape")
    };
    let [rx, ry, rz] = q.rotation.unwrap_or_default().map(f32::to_radians);
    let rotation = Quat::from_euler(EulerRot::YXZ, ry, rx, rz);
    let hit = |entity, point: Vec3, normal: Option<Vec3>, distance, inside| Hit3d { id: id(entity), point: Some(point.to_array()), normal: normal.map(|n| n.to_array()), distance: Some(distance), inside };
    Ok(match q.kind {
        Query3dKind::Ray => physics.raycast(from, dir()?, q.distance.unwrap_or(FAR), filter).map(|h| hit(h.entity, h.point, Some(h.normal), h.distance, None)).into_iter().collect(),
        Query3dKind::Cast => physics.cast_shape(shape()?, from, rotation, dir()?, q.distance.unwrap_or(FAR), filter).map(|h| hit(h.entity, h.point, Some(h.normal), h.distance, None)).into_iter().collect(),
        Query3dKind::Overlap => physics.overlap(shape()?, from, rotation, filter).into_iter().map(|e| Hit3d { id: id(e), ..Default::default() }).collect(),
        Query3dKind::Point => physics.overlap_point(from, filter).into_iter().map(|e| Hit3d { id: id(e), ..Default::default() }).collect(),
        Query3dKind::Closest => physics.closest_point(from, q.distance.unwrap_or(f32::MAX), filter).map(|h| hit(h.entity, h.point, None, h.distance, Some(h.inside))).into_iter().collect(),
    })
}

#[cfg(all(test, feature = "physics3d"))]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::testing::{app, frame};

    fn scene() -> Value {
        json!({ "physics3d": { "gravity": [0, 0, 0] }, "models": [
            { "id": "floor", "model": "asset:/@g/a.glb", "y": -10, "body": { "kind": "fixed", "size": [400, 20, 400] } },
            { "id": "crate", "model": "asset:/@g/a.glb", "x": 100, "y": 10, "scale": 20, "body": { "kind": "fixed", "group": 2 } },
            { "id": "zone", "model": "asset:/@g/a.glb", "x": -100, "y": 10, "scale": 20, "body": { "kind": "fixed", "sensor": true } },
        ] })
    }

    fn ask(app: &mut nge_ecs::App, q: Value) -> Query3dResult {
        app.command("query3d", &q);
        let events = frame(app, None);
        let (_, data) = events.iter().find(|(n, _)| n == "query3d").expect("回了结果");
        QUERY3D_RESULT.decode(data).unwrap()
    }

    fn ids(r: &Query3dResult) -> Vec<Option<&str>> {
        r.hits.iter().map(|h| h.id.as_deref()).collect()
    }

    #[test]
    fn rays_and_casts_hit_models_by_id() {
        let mut app = app();
        frame(&mut app, Some(scene()));
        let r = ask(&mut app, json!({ "id": 7, "kind": "ray", "from": [100, 100, 0], "dir": [0, -1, 0] }));
        assert_eq!(r.id, 7);
        assert_eq!(ids(&r), [Some("crate")]);
        let h = &r.hits[0];
        assert_eq!((h.point, h.normal, h.distance), (Some([100.0, 20.0, 0.0]), Some([0.0, 1.0, 0.0]), Some(80.0)));
        let r = ask(&mut app, json!({ "id": 8, "kind": "ray", "from": [100, 100, 0], "dir": [0, -1, 0], "exclude": "crate" }));
        assert_eq!(ids(&r), [Some("floor")], "跳过 exclude 的");
        let r = ask(&mut app, json!({ "id": 9, "kind": "ray", "from": [100, 100, 0], "dir": [0, -1, 0], "group": 2 }));
        assert_eq!(ids(&r), [Some("floor")], "同一组的跳过");
        let r = ask(&mut app, json!({ "id": 10, "kind": "cast", "from": [100, 100, 0], "dir": [0, -1, 0], "shape": { "ball": 10 } }));
        assert!((r.hits[0].distance.unwrap() - 70.0).abs() < 0.1, "{r:?}");
        let r = ask(&mut app, json!({ "id": 11, "kind": "ray", "from": [0, 100, 0], "dir": [0, 1, 0], "distance": 50 }));
        assert!(r.hits.is_empty());
    }

    #[test]
    fn overlaps_points_and_closest() {
        let mut app = app();
        frame(&mut app, Some(scene()));
        let r = ask(&mut app, json!({ "id": 1, "kind": "overlap", "from": [0, 10, 0], "shape": { "box": [300, 30, 10] }, "sensors": true }));
        let mut found = ids(&r);
        found.sort();
        assert_eq!(found, [Some("crate"), Some("floor"), Some("zone")]);
        let r = ask(&mut app, json!({ "id": 2, "kind": "overlap", "from": [0, 10, 0], "shape": { "capsule": [300, 5] }, "rotation": [0, 0, 90], "only": [2] }));
        assert_eq!(ids(&r), [Some("crate")], "只查分组 2；横过来的胶囊");
        let r = ask(&mut app, json!({ "id": 3, "kind": "point", "from": [-100, 10, 0] }));
        assert!(r.hits.is_empty(), "默认不算 sensor");
        let r = ask(&mut app, json!({ "id": 4, "kind": "closest", "from": [100, 50, 0], "only": [2] }));
        assert_eq!((r.hits[0].id.as_deref(), r.hits[0].point, r.hits[0].inside), (Some("crate"), Some([100.0, 20.0, 0.0]), Some(false)));
    }

    #[test]
    fn bad_queries_answer_empty_with_the_reason_right_away() {
        let mut app = app();
        frame(&mut app, Some(scene()));
        let bad = [
            json!({ "id": 1, "kind": "cast", "from": [0, 10, 0], "dir": [0, -1, 0] }),
            json!({ "id": 2, "kind": "ray", "from": [0, 10, 0], "dir": [0, -1, 0], "exclude": "nobody" }),
            json!({ "id": 3, "kind": "ray", "from": [0, 0, 0], "dirr": [0, 1, 0] }),
            json!({ "id": 4, "kind": "raycast", "from": [0, 0, 0], "dir": [0, 1, 0] }),
            json!({ "id": 5, "kind": "ray", "from": [0, 0, 0], "dir": [0, 1, 0], "group": 40 }),
            json!({ "id": 6, "kind": "ray", "from": [0, 0, 0], "dir": [0, 1, 0], "group": -1 }),
            json!({ "id": 7, "kind": "ray", "from": [0, 0, 0], "dir": [0, 1, 0], "only": [0, 300] }),
        ];
        for q in bad {
            let id = q["id"].as_u64().unwrap();
            // 同一帧就回：不让 TSX 干等到超时
            let r = ask(&mut app, q);
            assert_eq!(r.id, id);
            assert!(r.hits.is_empty() && r.error.as_deref().is_some_and(|e| !e.is_empty()), "查询 {id} 回了空结果和原因 {r:?}");
        }
        let ok = ask(&mut app, json!({ "id": 8, "kind": "ray", "from": [100, 100, 0], "dir": [0, -1, 0], "group": 31, "only": [0, 31] }));
        assert_eq!(ok.error, None, "0 … 31 的分组都行");
        // 没有 id 的回不了，丢掉
        app.command("query3d", &json!({ "kind": "ray" }));
        assert!(!frame(&mut app, None).iter().any(|(n, _)| n == "query3d"));
    }

    #[test]
    fn queries_are_drawn_by_the_debug_view_in_the_same_frame() {
        let mut app = app();
        frame(&mut app, Some(scene()));
        app.world_mut().resource_mut::<nge_physics3d::Physics3dDebug>().enabled = true;
        frame(&mut app, None);
        let lines = |app: &nge_ecs::App| app.world().resource::<nge_render3d::Gizmos3d>().depth_tested().len();
        frame(&mut app, None);
        let before = lines(&app);
        ask(&mut app, json!({ "id": 1, "kind": "ray", "from": [0, 100, 0], "dir": [0, -1, 0] }));
        assert!(lines(&app) > before, "回答查询的那一帧就画出射线 {before} → {}", lines(&app));
    }
}
