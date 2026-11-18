//! 节点上的动画状态机（`animGraph` / `animParams`）、`animSet` command、2D 精灵按状态机选帧、动画事件转给 TSX（见 docs/anim-graph.md）。

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use bevy_ecs::prelude::*;
use nge_anim::{AnimEvent, AnimGraph, Graph, GraphDef, LayerOutput, OneOrMany, ParamValue};
use nge_core::{CommandKey, EventKey};
use nge_ecs::{Outbox, Time};
use nge_render2d::{Sprite, SpriteImage};
use nge_render3d::Model3d;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value as Json;

use crate::sync::SceneId;

pub const ANIM_SET: CommandKey<AnimSet> = CommandKey::new("animSet");
pub const ANIM_EVENT: EventKey<AnimEventOut> = EventKey::new("animEvent");

/// 节点上声明式的参数。
pub type AnimParams = BTreeMap<String, ParamValue>;

/// TSX 的 `animSet` command：设参数、发 trigger。字段写错时整条不认，warn 里有字段名。
#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AnimSet {
    pub id: String,
    pub params: AnimParams,
    pub trigger: OneOrMany,
}

impl Default for AnimSet {
    fn default() -> Self {
        Self { id: String::new(), params: AnimParams::new(), trigger: OneOrMany::Many(Vec::new()) }
    }
}

/// 发给 TSX 的 `animEvent`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimEventOut {
    pub id: String,
    pub layer: String,
    pub state: String,
    pub clip: String,
    pub name: String,
}

/// 2D：取最上面一层权重 ≥ 0.5 的覆盖层（都不够时取第 0 层）里最重的片段（和发事件同一个规则：实例权重 × 片段权重，一样重时后面的赢），按时间取帧。
pub fn sprite_frame(graph: &Graph, out: &[LayerOutput]) -> Option<u32> {
    let layer = out.iter().skip(1).rev().find(|l| !l.additive && l.weight >= 0.5 && !l.clips.is_empty()).or(out.first())?;
    let top = nge_anim::heaviest(&layer.clips)?;
    graph.clips()[top.clip].frame_at(top.time)
}

/// 节点上的 `animGraph`：先按任意 JSON 收（定义写错不连累整个场景），同步时再解析。带一个内容的哈希，同步时按它比，不深比较。
#[derive(Debug, Clone)]
pub struct GraphJson {
    pub value: Json,
    hash: u64,
}

impl GraphJson {
    pub fn new(value: Json) -> Self {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        hash_json(&value, &mut h);
        Self { value, hash: h.finish() }
    }
}

/// 按哈希比（64 位，撞上的概率可以不管）。
impl PartialEq for GraphJson {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl<'de> Deserialize<'de> for GraphJson {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Json::deserialize(d).map(Self::new)
    }
}

fn hash_json(v: &Json, h: &mut impl Hasher) {
    match v {
        Json::Null => 0u8.hash(h),
        Json::Bool(b) => (1u8, b).hash(h),
        Json::Number(n) => (2u8, n.as_f64().unwrap_or(f64::NAN).to_bits()).hash(h),
        Json::String(s) => (3u8, s).hash(h),
        Json::Array(a) => {
            (4u8, a.len()).hash(h);
            a.iter().for_each(|x| hash_json(x, h));
        }
        Json::Object(o) => {
            (5u8, o.len()).hash(h);
            for (k, x) in o {
                k.hash(h);
                hash_json(x, h);
            }
        }
    }
}

/// 定义（按哈希）→ 编译好的状态机：一样的定义只编译一次，各节点共用一个 `Arc<Graph>`。
#[derive(Default)]
pub(crate) struct GraphCache(HashMap<u64, Result<Arc<Graph>, String>>);

impl GraphCache {
    /// 存太多（定义一直在变）时清一次。
    const MAX: usize = 256;

    fn get(&mut self, def: &GraphJson) -> Result<Arc<Graph>, String> {
        if self.0.len() > Self::MAX && !self.0.contains_key(&def.hash) {
            self.0.clear();
        }
        self.0
            .entry(def.hash)
            .or_insert_with(|| {
                let parsed = GraphDef::deserialize(&def.value).map_err(|e| format!("定义不对：{e}"))?;
                Graph::compile(&parsed).map(Arc::new)
            })
            .clone()
    }
}

/// 同步节点的 `animGraph` / `animParams`：定义按哈希比，变了才重建；参数只写和上一次声明不一样的。
/// 返回这一帧以后实体上有没有状态机（定义不合法时没有）。
pub(crate) fn apply_graph(
    entity: &mut EntityCommands,
    id: &str,
    before: Option<(&Option<GraphJson>, &Option<AnimParams>)>,
    def: &Option<GraphJson>,
    params: &Option<AnimParams>,
    cache: &mut GraphCache,
) -> bool {
    let Some(def) = def else {
        entity.remove::<AnimGraph>();
        return false;
    };
    let empty = AnimParams::new();
    let wanted = params.as_ref().unwrap_or(&empty);
    let rebuild = before.is_none_or(|(d, _)| d.as_ref() != Some(def));
    let graph = match cache.get(def) {
        Ok(graph) => graph,
        Err(e) => {
            if rebuild {
                log::warn!("节点 {id:?} 的 animGraph 不对，不建状态机：{e}");
                entity.remove::<AnimGraph>();
            }
            return false;
        }
    };
    if rebuild {
        let mut graph = AnimGraph::new(graph);
        set_params(&mut graph, id, wanted);
        entity.insert(graph);
        return true;
    }
    let old = before.and_then(|(_, p)| p.as_ref()).unwrap_or(&empty);
    let changed: AnimParams = wanted.iter().filter(|(k, v)| old.get(*k) != Some(*v)).map(|(k, v)| (k.clone(), *v)).collect();
    if !changed.is_empty() {
        let id = id.to_owned();
        entity.queue(move |mut e: EntityWorldMut| {
            if let Some(mut graph) = e.get_mut::<AnimGraph>() {
                set_params(&mut graph, &id, &changed);
            }
        });
    }
    true
}

fn set_params(graph: &mut AnimGraph, id: &str, params: &AnimParams) {
    for (name, value) in params {
        if !graph.set(name, *value) {
            log::warn!("节点 {id:?} 的状态机没有参数 {name:?}，或类型不对（{value:?}）");
        }
    }
}

pub(crate) fn animate_sprite_graphs(time: Res<Time>, mut sprites: Query<(Entity, &mut AnimGraph, &mut Sprite), Without<Model3d>>, mut events: MessageWriter<AnimEvent>) {
    for (entity, mut graph, mut sprite) in &mut sprites {
        let shared = graph.graph().clone();
        for fired in graph.step(time.delta, |name| shared.clip(name).and_then(nge_anim::Clip::frames_duration)) {
            events.write(AnimEvent::new(entity, fired));
        }
        if let (SpriteImage::Atlas(atlas, old), Some(frame)) = (sprite.image, sprite_frame(&shared, graph.output()))
            && old != frame
        {
            sprite.image = SpriteImage::Atlas(atlas, frame);
        }
    }
}

pub(crate) fn anim_set(mut reader: MessageReader<AnimSet>, mut graphs: Query<(&SceneId, &mut AnimGraph)>) {
    if reader.is_empty() {
        return;
    }
    for c in reader.read() {
        let Some((_, mut graph)) = graphs.iter_mut().find(|(id, _)| id.0 == c.id) else {
            log::warn!("animSet：没有 id 为 {:?} 的节点，或它没有状态机", c.id);
            continue;
        };
        set_params(&mut graph, &c.id, &c.params);
        let triggers = match &c.trigger {
            OneOrMany::One(t) => vec![t.clone()],
            OneOrMany::Many(t) => t.clone(),
        };
        for t in triggers {
            if !graph.trigger(&t) {
                log::warn!("animSet：{:?} 的状态机没有 trigger {t:?}", c.id);
            }
        }
    }
}

pub(crate) fn forward_events(mut reader: MessageReader<AnimEvent>, ids: Query<&SceneId>, mut outbox: ResMut<Outbox>) {
    for e in reader.read() {
        if let Ok(id) = ids.get(e.entity) {
            outbox.send(ANIM_EVENT, &AnimEventOut { id: id.0.clone(), layer: e.layer.clone(), state: e.state.clone(), clip: e.clip.clone(), name: e.name.clone() });
        }
    }
}

#[cfg(test)]
mod tests;
