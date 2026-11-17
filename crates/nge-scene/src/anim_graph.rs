//! 节点上的动画状态机（`animGraph` / `animParams`）、`animSet` command、2D 精灵按状态机选帧、动画事件转给 TSX（见 docs/anim-graph.md）。

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;
use nge_anim::{AnimEvent, AnimGraph, Graph, GraphDef, LayerOutput, OneOrMany, ParamValue};
use nge_core::{CommandKey, EventKey};
use nge_ecs::{Outbox, Time};
use nge_render2d::{Sprite, SpriteImage};
use nge_render3d::Model3d;
use serde::{Deserialize, Serialize};

use crate::sync::SceneId;

pub const ANIM_SET: CommandKey<AnimSet> = CommandKey::new("animSet");
pub const ANIM_EVENT: EventKey<AnimEventOut> = EventKey::new("animEvent");

/// 节点上声明式的参数。
pub type AnimParams = BTreeMap<String, ParamValue>;

/// TSX 的 `animSet` command：设参数、发 trigger。
#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
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

/// 2D：取最上面一层权重 ≥ 0.5 的覆盖层（都不够时取第 0 层）里权重最大的片段，按时间取帧。
pub fn sprite_frame(graph: &Graph, out: &[LayerOutput]) -> Option<u32> {
    let layer = out.iter().skip(1).rev().find(|l| !l.additive && l.weight >= 0.5 && !l.clips.is_empty()).or(out.first())?;
    // 一样重时写在前面的赢
    let top = layer.clips.iter().fold(None, |best: Option<&nge_anim::ClipSample>, c| if best.is_none_or(|b| c.weight > b.weight) { Some(c) } else { best })?;
    graph.clips()[top.clip].frame_at(top.time)
}

/// 同步节点的 `animGraph` / `animParams`：定义按值比，变了才重建；参数只写和上一次声明不一样的。
/// 返回这一帧以后实体上有没有状态机（定义不合法时没有）。
pub(crate) fn apply_graph(entity: &mut EntityCommands, id: &str, before: Option<(&Option<GraphDef>, &Option<AnimParams>)>, def: &Option<GraphDef>, params: &Option<AnimParams>) -> bool {
    let Some(def) = def else {
        entity.remove::<AnimGraph>();
        return false;
    };
    let empty = AnimParams::new();
    let wanted = params.as_ref().unwrap_or(&empty);
    let rebuild = before.is_none_or(|(d, _)| d.as_ref() != Some(def));
    if rebuild {
        return match AnimGraph::from_def(def) {
            Ok(mut graph) => {
                set_params(&mut graph, id, wanted);
                entity.insert(graph);
                true
            }
            Err(e) => {
                log::warn!("节点 {id:?} 的 animGraph 不对，不建状态机：{e}");
                entity.remove::<AnimGraph>();
                false
            }
        };
    }
    // 定义没变：上一次没建成的这次也建不成（编译很快，节点变了才走到这里）
    if Graph::compile(def).is_err() {
        return false;
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
