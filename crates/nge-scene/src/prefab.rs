//! 预制体：可复用的节点组，场景里按名字或 `asset:/` 路径实例化，平移、按子节点覆盖属性、嵌套（见 docs/scene-flow.md）。

use std::collections::{HashMap, HashSet};

use bevy_ecs::prelude::*;
use nge_assets::{Asset, AssetEvent, AssetServer, Assets, LoadContext, LoadState};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::lenient::{items, warn_once};
use crate::protocol::Scene;

/// 预制体里能放的列表，顺序固定（展开结果按这个顺序存）。
pub(crate) const LISTS: [&str; 7] = ["sprites", "tilemaps", "texts", "emitters", "bars", "bubbles", "joints"];
/// 嵌套最多几层。
const MAX_DEPTH: usize = 16;
/// 一个场景最多展开多少个实例和节点（合起来）。
const MAX_NODES: usize = 10_000;

/// 场景 / 预制体的 JSON 文件（`asset:/….json`）。
#[derive(Debug, Clone, PartialEq)]
pub struct SceneFile(pub Value);

impl Asset for SceneFile {
    fn decode(bytes: Vec<u8>, _ctx: &LoadContext) -> Result<Self, String> {
        serde_json::from_slice(&bytes).map(SceneFile).map_err(|e| format!("JSON 解析失败: {e}"))
    }
}

/// 一个实例：`prefab` 是 `scene.prefabs` 里的名字或 `asset:/` 路径；`set` 按子节点 id 覆盖字段。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceNode {
    pub id: String,
    pub prefab: String,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub z: f32,
    #[serde(default)]
    pub set: Map<String, Value>,
}

/// 按名字找预制体的结果。
pub(crate) enum Found {
    Ready(Value),
    /// 文件还在加载：先不展开，加载好了再来。
    Loading,
    Missing(String),
}

/// 展开所有实例，按 [`LISTS`] 的顺序给出每个列表新增的节点（还是 JSON，之后照常宽松解析）。
pub(crate) fn expand(instances: &[InstanceNode], find: &mut dyn FnMut(&str) -> Found) -> [Vec<Value>; 7] {
    let mut out: [Vec<Value>; 7] = Default::default();
    let mut budget = MAX_NODES;
    for instance in instances {
        instantiate(instance, find, &mut Vec::new(), &mut out, &mut budget);
    }
    out
}

fn instantiate(inst: &InstanceNode, find: &mut dyn FnMut(&str) -> Found, stack: &mut Vec<String>, out: &mut [Vec<Value>; 7], budget: &mut usize) {
    // 实例也算进上限：只有实例、没有节点的一层层扇出也停得住
    if *budget == 0 {
        warn_once(format!("预制体展开出的实例和节点超过 {MAX_NODES} 个，后面的不要了"));
        return;
    }
    *budget -= 1;
    if stack.contains(&inst.prefab) {
        warn_once(format!("预制体循环：{} → {}，跳过实例 {:?}", stack.join(" → "), inst.prefab, inst.id));
        return;
    }
    if stack.len() >= MAX_DEPTH {
        warn_once(format!("预制体嵌套超过 {MAX_DEPTH} 层（{}），跳过实例 {:?}", stack.join(" → "), inst.id));
        return;
    }
    let def = match find(&inst.prefab) {
        Found::Ready(Value::Object(def)) => def,
        Found::Ready(_) => {
            warn_once(format!("预制体 {} 不是对象，跳过实例 {:?}", inst.prefab, inst.id));
            return;
        }
        Found::Loading => return,
        Found::Missing(e) => {
            warn_once(format!("实例 {:?} 的预制体 {}：{e}，跳过", inst.id, inst.prefab));
            return;
        }
    };
    let mut local: [Vec<Value>; 7] = Default::default();
    for (list, name) in local.iter_mut().zip(LISTS) {
        let nodes = match def.get(name) {
            None | Some(Value::Null) => continue,
            Some(Value::Array(nodes)) => nodes,
            Some(other) => {
                warn_once(format!("预制体 {} 的 {name} 要是数组，收到 {other}，当没写", inst.prefab));
                continue;
            }
        };
        for node in nodes {
            if node.get("id").and_then(Value::as_str).is_none() {
                warn_once(format!("预制体 {} 的 {name} 里有一项没有字符串 id，跳过", inst.prefab));
            } else if *budget == 0 {
                warn_once(format!("预制体展开出的实例和节点超过 {MAX_NODES} 个，后面的不要了"));
                break;
            } else {
                *budget -= 1;
                list.push(node.clone());
            }
        }
    }
    stack.push(inst.prefab.clone());
    let nested: Vec<InstanceNode> = items(&format!("预制体 {} 的 instances", inst.prefab), def.get("instances").cloned().unwrap_or(Value::Null));
    for child in &nested {
        instantiate(child, find, stack, &mut local, budget);
    }
    stack.pop();
    override_fields(inst, &mut local);
    let inside: HashSet<String> = local.iter().flatten().filter_map(|n| n["id"].as_str().map(str::to_owned)).collect();
    for (list, name) in local.iter_mut().zip(LISTS) {
        for node in list.iter_mut() {
            place(name, node, inst, &inside);
        }
    }
    for (o, l) in out.iter_mut().zip(local) {
        o.extend(l);
    }
}

/// `set` 的覆盖：浅合并，在平移之前（坐标是预制体里的）。
fn override_fields(inst: &InstanceNode, local: &mut [Vec<Value>; 7]) {
    for (target, fields) in &inst.set {
        let Value::Object(fields) = fields else {
            warn_once(format!("实例 {:?} 的 set 里 {target:?} 要是对象，跳过", inst.id));
            continue;
        };
        let mut hit = false;
        for node in local.iter_mut().flatten().filter(|n| n["id"].as_str() == Some(target.as_str())) {
            hit = true;
            for (key, value) in fields {
                if key == "id" {
                    warn_once(format!("实例 {:?} 的 set 不能改 id，跳过", inst.id));
                } else {
                    node[key] = value.clone();
                }
            }
        }
        if !hit {
            warn_once(format!("实例 {:?} 的 set 里 {target:?} 不是预制体 {} 里的节点", inst.id, inst.prefab));
        }
    }
}

/// 改名（加实例 id 前缀，指向预制体里面的引用也跟着改）、平移。
fn place(list: &str, node: &mut Value, inst: &InstanceNode, inside: &HashSet<String>) {
    let prefix = |v: &mut Value| {
        if let Some(id) = v.as_str().filter(|id| inside.contains(*id)) {
            *v = Value::String(format!("{}/{id}", inst.id));
        }
    };
    let id = node["id"].as_str().unwrap_or_default().to_owned();
    node["id"] = Value::String(format!("{}/{id}", inst.id));
    let offset = |node: &mut Value, key: &str, by: f32| {
        if by == 0.0 {
            return;
        }
        match node.get(key) {
            None | Some(Value::Null) => node[key] = by.into(),
            Some(Value::Number(n)) => node[key] = (n.as_f64().unwrap_or(0.0) + f64::from(by)).into(),
            // 写错的留着，解析时只丢这一项
            Some(_) => {}
        }
    };
    match list {
        "sprites" | "tilemaps" | "emitters" => {
            if list == "sprites" {
                if let Some(target) = node.get_mut("chase").and_then(|c| c.get_mut("target")) {
                    prefix(target);
                }
                if let Some(tree) = node.get_mut("behavior").and_then(|b| b.get_mut("tree")) {
                    behavior(tree, &prefix, inst);
                }
            }
            offset(node, "x", inst.x);
            offset(node, "y", inst.y);
            offset(node, "z", inst.z);
        }
        "texts" | "bars" | "bubbles" => {
            let follows = node.get("follow").is_some_and(|f| !f.is_null());
            if let Some(follow) = node.get_mut("follow") {
                prefix(follow);
            }
            if !follows && node.get("screen") != Some(&Value::Bool(true)) {
                offset(node, "x", inst.x);
                offset(node, "y", inst.y);
            }
        }
        "joints" => {
            for end in ["a", "b"] {
                if let Some(v) = node.get_mut(end) {
                    prefix(v);
                }
            }
            for anchor in ["anchorA", "anchorB"] {
                if let Some(Value::Array(p)) = node.get_mut(anchor)
                    && let [Value::Number(x), Value::Number(y)] = p.as_slice()
                {
                    let moved = [x.as_f64().unwrap_or(0.0) + f64::from(inst.x), y.as_f64().unwrap_or(0.0) + f64::from(inst.y)];
                    *p = vec![moved[0].into(), moved[1].into()];
                }
            }
        }
        _ => {}
    }
}

/// 行为树里 see / near / chase 的目标跟着改名，goto 的点跟着平移。
fn behavior(node: &mut Value, prefix: &dyn Fn(&mut Value), inst: &InstanceNode) {
    match node.get("type").and_then(Value::as_str) {
        Some("see" | "near" | "chase") => {
            if let Some(target) = node.get_mut("target") {
                prefix(target);
            }
        }
        Some("goto") => {
            for (key, by) in [("x", inst.x), ("y", inst.y)] {
                if let Some(Value::Number(n)) = node.get(key) {
                    node[key] = (n.as_f64().unwrap_or(0.0) + f64::from(by)).into();
                }
            }
        }
        _ => {}
    }
    if let Some(Value::Array(children)) = node.get_mut("children") {
        for child in children {
            behavior(child, prefix, inst);
        }
    }
    if let Some(child) = node.get_mut("child") {
        behavior(child, prefix, inst);
    }
}

/// 名字找 `scene.prefabs`，`asset:/` 路径交给资源层（还没加载就开始加载）。
fn find(name: &str, inline: &HashMap<String, Value>, server: &mut AssetServer, files: &mut Assets<SceneFile>, used: &mut HashSet<u32>) -> Found {
    if !name.starts_with("asset:/") {
        return inline.get(name).cloned().map_or_else(|| Found::Missing("场景的 prefabs 里没有".into()), Found::Ready);
    }
    let handle = server.load(files, name);
    used.insert(handle.index());
    match files.state(handle) {
        LoadState::Loaded => files.get(handle).map_or(Found::Loading, |f| Found::Ready(f.0.clone())),
        LoadState::Loading => Found::Loading,
        LoadState::Failed(e) => Found::Missing(e),
    }
}

fn lens(scene: &Scene) -> [usize; 7] {
    [scene.sprites.len(), scene.tilemaps.len(), scene.texts.len(), scene.emitters.len(), scene.bars.len(), scene.bubbles.len(), scene.joints.len()]
}

/// 新场景有实例时展开，接在各列表后面；有预制体文件加载好了，截回原来的长度重新展开。
pub(crate) fn expand_prefabs(
    mut scene: ResMut<Scene>,
    mut server: ResMut<AssetServer>,
    mut files: ResMut<Assets<SceneFile>>,
    mut events: MessageReader<AssetEvent<SceneFile>>,
    mut base: Local<Option<[usize; 7]>>,
    mut used: Local<HashSet<u32>>,
) {
    // 只有这个场景用到的预制体文件加载好（或失败）了才重新展开
    let loaded = events.read().any(|e| match e {
        AssetEvent::Loaded(h) | AssetEvent::Failed(h, _) => used.contains(&h.index()),
    });
    if scene.is_changed() && !scene.expanded {
        *base = None;
        used.clear();
        if scene.instances.is_empty() {
            return;
        }
    } else if !loaded || base.is_none() {
        return;
    }
    let scene = &mut *scene;
    let lens = *base.get_or_insert_with(|| lens(scene));
    let used = &mut *used;
    let [sprites, tilemaps, texts, emitters, bars, bubbles, joints] = expand(&scene.instances, &mut |name| find(name, &scene.prefabs, &mut server, &mut files, used));
    scene.sprites.truncate(lens[0]);
    scene.sprites.extend(items::<crate::protocol::SpriteNode>("sprites", Value::Array(sprites)));
    scene.tilemaps.truncate(lens[1]);
    scene.tilemaps.extend(items::<crate::tilemap::TilemapNode>("tilemaps", Value::Array(tilemaps)));
    scene.texts.truncate(lens[2]);
    scene.texts.extend(items::<crate::nodes::TextNode>("texts", Value::Array(texts)));
    scene.emitters.truncate(lens[3]);
    scene.emitters.extend(items::<crate::nodes::EmitterNode>("emitters", Value::Array(emitters)));
    scene.bars.truncate(lens[4]);
    scene.bars.extend(items::<crate::world_ui::BarNode>("bars", Value::Array(bars)));
    scene.bubbles.truncate(lens[5]);
    scene.bubbles.extend(items::<crate::world_ui::BubbleNode>("bubbles", Value::Array(bubbles)));
    scene.joints.truncate(lens[6]);
    scene.joints.extend(items::<crate::joints::JointNode>("joints", Value::Array(joints)));
    scene.expanded = true;
    for dup in duplicate_ids(scene) {
        warn_once(format!("场景里有重复的 id（{dup}）：同一个列表里后面的盖掉前面的"));
    }
}

/// 重复的实例 id，和展开后同一个列表里重复的 id（`列表: id`）。
pub(crate) fn duplicate_ids(scene: &Scene) -> Vec<String> {
    let lists: [(&str, Vec<&str>); 8] = [
        ("instances", scene.instances.iter().map(|n| n.id.as_str()).collect()),
        ("sprites", scene.sprites.iter().map(|n| n.id.as_str()).collect()),
        ("tilemaps", scene.tilemaps.iter().map(|n| n.id.as_str()).collect()),
        ("texts", scene.texts.iter().map(|n| n.id.as_str()).collect()),
        ("emitters", scene.emitters.iter().map(|n| n.id.as_str()).collect()),
        ("bars", scene.bars.iter().map(|n| n.id.as_str()).collect()),
        ("bubbles", scene.bubbles.iter().map(|n| n.id.as_str()).collect()),
        ("joints", scene.joints.iter().map(|n| n.id.as_str()).collect()),
    ];
    let mut out = Vec::new();
    for (list, ids) in lists {
        let mut seen = HashSet::new();
        let mut reported = HashSet::new();
        for id in ids {
            if !seen.insert(id) && reported.insert(id) {
                out.push(format!("{list}: {id}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};


    use nge_render2d::Transform2d;
    use serde_json::{Value, json};

    use super::*;
    use crate::sync::SceneId;
    use crate::testing::{CYCLE_A, TREE, app, frame};

    fn instance(v: Value) -> InstanceNode {
        serde_json::from_value(v).unwrap()
    }

    fn defs(map: Value) -> impl FnMut(&str) -> Found {
        move |name: &str| map.get(name).cloned().map_or(Found::Missing("没有".into()), Found::Ready)
    }

    fn ids(nodes: &[Value]) -> Vec<&str> {
        nodes.iter().map(|n| n["id"].as_str().unwrap()).collect()
    }

    fn tree() -> Value {
        json!({ "sprites": [{ "id": "trunk", "y": 40, "w": 10 }, { "id": "crown", "x": 0, "y": 90, "rotation": 45 }] })
    }

    #[test]
    fn instances_prefix_ids_and_move_their_nodes() {
        let out = expand(&[instance(json!({ "id": "t1", "prefab": "tree", "x": 100, "y": -5, "z": 2 }))], &mut defs(json!({ "tree": tree() })));
        assert_eq!(ids(&out[0]), ["t1/trunk", "t1/crown"]);
        assert_eq!((out[0][0]["x"].as_f64(), out[0][0]["y"].as_f64(), out[0][0]["z"].as_f64()), (Some(100.0), Some(35.0), Some(2.0)), "没写的 x 当 0");
        assert_eq!(out[0][1]["rotation"], json!(45), "别的字段原样");
    }

    #[test]
    fn set_overrides_fields_by_child_id_before_moving() {
        let i = instance(json!({ "id": "t", "prefab": "tree", "x": 10, "set": { "crown": { "y": 104, "color": "#ff0000" }, "ghost": { "x": 1 } } }));
        let out = expand(&[i], &mut defs(json!({ "tree": tree() })));
        let crown = &out[0][1];
        assert_eq!((crown["y"].as_f64(), crown["x"].as_f64(), crown["color"].as_str()), (Some(104.0), Some(10.0), Some("#ff0000")), "覆盖的 y 是预制体里的坐标，x 照样平移");
        assert_eq!(out[0][0]["y"].as_f64(), Some(40.0), "别的子节点不动");
    }

    #[test]
    fn nested_prefabs_compose_and_outer_overrides_win() {
        let grove = json!({
            "sprites": [{ "id": "rock", "x": 5 }],
            "instances": [{ "id": "a", "prefab": "tree" }, { "id": "b", "prefab": "tree", "x": 50, "set": { "crown": { "y": 104 } } }],
        });
        let out = expand(
            &[instance(json!({ "id": "g", "prefab": "grove", "x": 1000, "set": { "b/crown": { "color": "#00ff00" }, "a/trunk": { "w": 12 } } }))],
            &mut defs(json!({ "tree": tree(), "grove": grove })),
        );
        assert_eq!(ids(&out[0]), ["g/rock", "g/a/trunk", "g/a/crown", "g/b/trunk", "g/b/crown"]);
        let b_crown = &out[0][4];
        assert_eq!((b_crown["x"].as_f64(), b_crown["y"].as_f64(), b_crown["color"].as_str()), (Some(1050.0), Some(104.0), Some("#00ff00")), "里层的覆盖和外层的都在，两层都平移");
        assert_eq!(out[0][1]["w"], json!(12), "外层用 a/trunk 改里层的");
    }

    #[test]
    fn references_inside_the_prefab_follow_the_rename() {
        let def = json!({
            "sprites": [{ "id": "body" }, { "id": "eye", "chase": { "target": "body" } }, { "id": "hunter", "chase": { "target": "player" } }],
            "joints": [{ "id": "j", "a": "body", "b": "eye", "kind": "weld", "anchorA": [1, 2] }],
            "bars": [{ "id": "hp", "follow": "body", "y": 20 }, { "id": "free", "x": 3 }],
            "texts": [{ "id": "label", "text": "hi", "screen": true, "x": 4 }],
        });
        let out = expand(&[instance(json!({ "id": "e", "prefab": "enemy", "x": 100, "y": 10 }))], &mut defs(json!({ "enemy": def })));
        assert_eq!(out[0][1]["chase"]["target"], json!("e/body"));
        assert_eq!(out[0][2]["chase"]["target"], json!("player"), "外面的不改");
        let j = &out[6][0];
        assert_eq!((j["a"].as_str(), j["b"].as_str(), j["anchorA"].clone()), (Some("e/body"), Some("e/eye"), json!([101.0, 12.0])));
        assert_eq!((out[4][0]["follow"].as_str(), out[4][0]["y"].as_f64()), (Some("e/body"), Some(20.0)), "跟着精灵的是相对偏移，不平移");
        assert_eq!(out[4][1]["x"].as_f64(), Some(103.0));
        assert_eq!(out[2][0]["x"].as_f64(), Some(4.0), "屏幕上的字不平移");
    }

    #[test]
    fn cycles_and_missing_prefabs_skip_only_that_instance() {
        let map = json!({
            "a": { "sprites": [{ "id": "x" }], "instances": [{ "id": "to-b", "prefab": "b" }] },
            "b": { "sprites": [{ "id": "y" }], "instances": [{ "id": "back", "prefab": "a" }] },
            "self": { "instances": [{ "id": "me", "prefab": "self" }] },
            "bad": 3,
        });
        let out = expand(
            &[
                instance(json!({ "id": "1", "prefab": "a" })),
                instance(json!({ "id": "2", "prefab": "self" })),
                instance(json!({ "id": "3", "prefab": "nope" })),
                instance(json!({ "id": "4", "prefab": "bad" })),
                instance(json!({ "id": "5", "prefab": "b" })),
            ],
            &mut defs(map),
        );
        assert_eq!(ids(&out[0]), ["1/x", "1/to-b/y", "5/y", "5/back/x"], "a → b → a 停在第二个 a，别的照常");
    }

    #[test]
    fn deep_or_huge_expansions_are_capped() {
        let mut map = serde_json::Map::new();
        for i in 0..40 {
            map.insert(format!("p{i}"), json!({ "sprites": [{ "id": "s" }], "instances": [{ "id": "n", "prefab": format!("p{}", i + 1) }] }));
        }
        let out = expand(&[instance(json!({ "id": "r", "prefab": "p0" }))], &mut defs(Value::Object(map)));
        assert_eq!(out[0].len(), MAX_DEPTH, "最多 {MAX_DEPTH} 层");
        let wide = json!({ "leaf": { "sprites": (0..100).map(|i| json!({ "id": format!("s{i}") })).collect::<Vec<_>>() } });
        let many: Vec<InstanceNode> = (0..200).map(|i| instance(json!({ "id": format!("i{i}"), "prefab": "leaf" }))).collect();
        let n = expand(&many, &mut defs(wide))[0].len();
        assert!(n <= MAX_NODES && n > MAX_NODES - 200, "实例也占上限：{n}");
    }

    #[test]
    fn loading_prefabs_are_left_out_for_now() {
        let out = expand(&[instance(json!({ "id": "t", "prefab": "asset:/x.json" }))], &mut |_| Found::Loading);
        assert!(out.iter().all(Vec::is_empty));
    }

    fn sprite_x(app: &mut nge_ecs::App, id: &str) -> Option<f32> {
        let world = app.world_mut();
        world.query::<(&SceneId, &Transform2d)>().iter(world).find(|(s, _)| s.0 == id).map(|(_, t)| t.translation.x)
    }

    /// 按墙钟等（后台线程在加载文件），最多 15 秒。
    fn wait_for(app: &mut nge_ecs::App, mut done: impl FnMut(&mut nge_ecs::App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !done(app) {
            assert!(Instant::now() < deadline, "15 秒还没好");
            frame(app, None);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn scene_instances_become_entities_and_file_prefabs_arrive_later() {
        let mut app = app();
        let scene = json!({
            "prefabs": { "tree": tree() },
            "instances": [{ "id": "t1", "prefab": "tree", "x": 100 }, { "id": "f", "prefab": TREE, "x": -50 }, { "id": "loop", "prefab": CYCLE_A }],
            "sprites": [{ "id": "player" }],
        });
        frame(&mut app, Some(scene.clone()));
        assert_eq!(sprite_x(&mut app, "t1/crown"), Some(100.0), "TSX 里的预制体当帧就有");
        assert_eq!(sprite_x(&mut app, "player"), Some(0.0), "手写的节点照常");
        wait_for(&mut app, |app| sprite_x(app, "f/leaf").is_some());
        assert_eq!(sprite_x(&mut app, "f/leaf"), Some(-48.0), "文件里的预制体加载好以后展开");
        assert_eq!(sprite_x(&mut app, "t1/crown"), Some(100.0));
        assert_eq!(app.world().resource::<Scene>().sprites.len(), 5, "重新展开不重复");
        let world = app.world_mut();
        let count = world.query::<&SceneId>().iter(world).count();
        assert_eq!(count, 1 + 2 + 2, "player、t1 两个、f 两个；循环的文件跳过");
        // 场景再变（同样的实例）不重复
        frame(&mut app, Some(scene));
        let world = app.world_mut();
        assert_eq!(world.query::<&SceneId>().iter(world).count(), 5);
    }

    #[test]
    fn a_bad_instance_drops_only_itself() {
        let scene = crate::protocol::SCENE.decode(&json!({ "instances": [{ "id": "a", "prefab": "p" }, { "id": "b" }, { "id": "c", "prefab": "p", "sett": {} }], "prefabs": { "p": {}, "q": 3 } })).unwrap();
        assert_eq!(scene.instances.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["a"]);
        assert_eq!(scene.prefabs.len(), 1, "不是对象的预制体丢掉");
    }

    #[test]
    fn wide_and_deep_chains_stop_in_time() {
        // 每层 8 个实例指向下一层，16 层：不设实例上限时要展开 8 的 16 次方次
        let mut map = serde_json::Map::new();
        for i in 0..16 {
            let kids: Vec<Value> = (0..8).map(|k| json!({ "id": format!("k{k}"), "prefab": format!("p{}", i + 1) })).collect();
            map.insert(format!("p{i}"), json!({ "instances": kids }));
        }
        map.insert("p16".into(), json!({ "sprites": [{ "id": "leaf" }] }));
        let defs = Value::Object(map);
        // 在别的线程里展开：停不下来时测试照样按时失败
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut calls = 0usize;
            let out = expand(&[instance(json!({ "id": "r", "prefab": "p0" }))], &mut |name: &str| {
                calls += 1;
                defs.get(name).cloned().map_or(Found::Missing("没有".into()), Found::Ready)
            });
            let _ = tx.send((calls, out[0].len()));
        });
        let (calls, nodes) = rx.recv_timeout(Duration::from_secs(5)).expect("5 秒内要展开完");
        assert!(calls <= MAX_NODES, "最多展开 {MAX_NODES} 个实例：{calls}");
        assert!(nodes <= MAX_NODES);
    }

    #[test]
    fn behavior_targets_follow_the_rename_and_goto_moves() {
        let def = json!({ "sprites": [
            { "id": "body" },
            { "id": "eye", "behavior": { "tree": { "type": "sequence", "children": [
                { "type": "see", "target": "body", "range": 10 },
                { "type": "near", "target": "player", "range": 10 },
                { "type": "invert", "child": { "type": "chase", "target": "body" } },
                { "type": "goto", "x": 1, "y": 2 },
            ] } } },
        ] });
        let out = expand(&[instance(json!({ "id": "e", "prefab": "enemy", "x": 100, "y": 10 }))], &mut defs(json!({ "enemy": def })));
        let tree = &out[0][1]["behavior"]["tree"]["children"];
        assert_eq!(tree[0]["target"], json!("e/body"));
        assert_eq!(tree[1]["target"], json!("player"), "外面的不改");
        assert_eq!(tree[2]["child"]["target"], json!("e/body"));
        assert_eq!((tree[3]["x"].as_f64(), tree[3]["y"].as_f64()), (Some(101.0), Some(12.0)));
    }

    #[test]
    fn unrelated_scene_files_do_not_reexpand() {
        let mut app = app();
        frame(&mut app, Some(json!({ "prefabs": { "tree": tree() }, "instances": [{ "id": "t", "prefab": "tree" }] })));
        frame(&mut app, None);
        let before = app.world().resource_ref::<Scene>().last_changed();
        app.command("preload", &json!({ "id": "x", "uris": [TREE] }));
        wait_for(&mut app, |app| {
            let files = app.world().resource::<Assets<SceneFile>>();
            files.iter().count() > 0
        });
        frame(&mut app, None);
        assert_eq!(app.world().resource_ref::<Scene>().last_changed(), before, "这个场景没用到这个文件，不重新展开");
    }

    #[test]
    fn duplicate_ids_are_found() {
        let mut app = app();
        frame(&mut app, Some(json!({
            "prefabs": { "tree": tree() },
            "instances": [{ "id": "t", "prefab": "tree" }, { "id": "t", "prefab": "tree", "x": 5 }, { "id": "u", "prefab": "tree" }],
            "sprites": [{ "id": "u/crown" }],
        })));
        let mut dup = duplicate_ids(app.world().resource::<Scene>());
        dup.sort();
        assert_eq!(dup, ["instances: t", "sprites: t/crown", "sprites: t/trunk", "sprites: u/crown"]);
    }
}
