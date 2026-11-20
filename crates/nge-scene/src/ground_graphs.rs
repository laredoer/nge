//! 地面寻路图的缓存：地图第一次用时、整张重建以后当场建；只改了格子时先接着用旧图，
//! 原生平台在后台线程重建，wasm 上没有线程，最多每 [`GRAPH_EVERY`] 秒在这一帧重建一次（见 docs/big-scene-perf.md）。

use std::collections::HashMap;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use bevy_ecs::prelude::Entity;
use nge_ai::{GroundGraph, Terrain, Walker};

/// wasm 上地图改了以后最多多久重建一次（秒）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const GRAPH_EVERY: f64 = 0.5;

/// 一张地图按一种角色（跳跃能力、大小、坡）建的图的键：地图、角色、地图的位置和大小（左上角 x、y，列数、行数）。
pub(crate) type GraphKey = (Entity, [u32; 6], [u32; 4]);

struct Entry {
    graph: Arc<GroundGraph>,
    /// 地图改过，这张图旧了。
    stale: bool,
    built_at: f64,
    #[cfg(not(target_arch = "wasm32"))]
    pending: Option<Receiver<GroundGraph>>,
}

/// 资源（`Local`）：所有建好的图。
#[derive(Default)]
pub(crate) struct GroundGraphs {
    entries: HashMap<GraphKey, Entry>,
    /// 一共重建过几次（第一次建的不算）。
    rebuilt: u64,
}

impl GroundGraphs {
    /// 这一帧开始：去掉没了的、整张重建过的地图的图（用到时当场建），只改了格子的地图的图标旧，收后台建好的。
    pub(crate) fn begin(&mut self, alive: impl Fn(Entity) -> bool, changed: &[Entity], rebuilt: &[Entity]) {
        self.entries.retain(|(e, ..), _| alive(*e) && !rebuilt.contains(e));
        for ((e, ..), entry) in &mut self.entries {
            entry.stale |= changed.contains(e);
        }
        #[cfg(not(target_arch = "wasm32"))]
        for entry in self.entries.values_mut() {
            let Some(rx) = &entry.pending else { continue };
            match rx.try_recv() {
                Ok(graph) => {
                    entry.graph = Arc::new(graph);
                    entry.pending = None;
                    self.rebuilt += 1;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => entry.pending = None,
            }
        }
    }

    /// 这张地图、这种角色的图：没有就当场建；旧了就先给旧的，按平台在后台或节流重建。
    pub(crate) fn get(&mut self, key: GraphKey, terrain: &Terrain, walker: Walker, now: f64) -> Arc<GroundGraph> {
        // 同一张地图、同一种角色，位置或大小变了的旧图不要了
        self.entries.retain(|k, _| !(k.0 == key.0 && k.1 == key.1 && k.2 != key.2));
        let entry = self.entries.entry(key).or_insert_with(|| Entry {
            graph: Arc::new(GroundGraph::build_for(terrain, walker)),
            stale: false,
            built_at: now,
            #[cfg(not(target_arch = "wasm32"))]
            pending: None,
        });
        if entry.stale {
            #[cfg(not(target_arch = "wasm32"))]
            if entry.pending.is_none() {
                let (tx, rx) = channel();
                let terrain = terrain.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(GroundGraph::build_for(&terrain, walker));
                });
                entry.pending = Some(rx);
                (entry.stale, entry.built_at) = (false, now);
            }
            #[cfg(target_arch = "wasm32")]
            if now - entry.built_at >= GRAPH_EVERY {
                entry.graph = Arc::new(GroundGraph::build_for(terrain, walker));
                (entry.stale, entry.built_at) = (false, now);
                self.rebuilt += 1;
            }
        }
        entry.graph.clone()
    }

    #[cfg(test)]
    pub(crate) fn rebuilt(&self) -> u64 {
        self.rebuilt
    }

    /// 还在后台建的图。
    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        return self.entries.values().filter(|e| e.pending.is_some()).count();
        #[cfg(target_arch = "wasm32")]
        0
    }
}

#[cfg(test)]
mod tests {
    use nge_ai::{Ground, Reach};

    use super::*;

    fn terrain(gap: bool) -> Terrain {
        let mut tiles = vec![Ground::Empty; 8 * 3];
        tiles[16..].fill(Ground::Solid);
        if gap {
            tiles[16 + 4] = Ground::Empty;
        }
        Terrain::new(8, tiles)
    }

    fn walker() -> Walker {
        Walker::new(Reach { height: 0.0, distance: 0.0 })
    }

    fn wait(graphs: &mut GroundGraphs, key: GraphKey) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while graphs.pending() > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
            graphs.begin(|_| true, &[], &[]);
        }
        assert!(graphs.entries.contains_key(&key));
    }

    #[test]
    fn a_changed_map_keeps_the_old_graph_until_the_new_one_is_ready() {
        let key = (Entity::PLACEHOLDER, [0; 6], [0; 4]);
        let mut graphs = GroundGraphs::default();
        graphs.begin(|_| true, &[], &[]);
        let old = graphs.get(key, &terrain(false), walker(), 0.0);
        assert!(Arc::ptr_eq(&old, &graphs.get(key, &terrain(false), walker(), 0.1)), "没改就一直用");
        graphs.begin(|_| true, &[Entity::PLACEHOLDER], &[]);
        let during = graphs.get(key, &terrain(true), walker(), 0.2);
        assert!(Arc::ptr_eq(&old, &during), "先接着用旧图，不卡这一帧");
        wait(&mut graphs, key);
        let new = graphs.get(key, &terrain(true), walker(), 0.3);
        assert!(!Arc::ptr_eq(&old, &new));
        assert_eq!(*new, GroundGraph::build_for(&terrain(true), walker()), "建好的是新地图的");
        assert_eq!(graphs.rebuilt(), 1);
    }

    #[test]
    fn graphs_of_removed_maps_are_dropped() {
        let key = (Entity::PLACEHOLDER, [0; 6], [0; 4]);
        let mut graphs = GroundGraphs::default();
        graphs.get(key, &terrain(false), walker(), 0.0);
        graphs.begin(|_| false, &[], &[]);
        assert!(graphs.entries.is_empty());
    }

    #[test]
    fn a_rebuilt_map_gets_its_graph_built_right_away() {
        let key = (Entity::PLACEHOLDER, [0; 6], [0; 4]);
        let mut graphs = GroundGraphs::default();
        graphs.get(key, &terrain(false), walker(), 0.0);
        graphs.begin(|_| true, &[Entity::PLACEHOLDER], &[Entity::PLACEHOLDER]);
        let new = graphs.get(key, &terrain(true), walker(), 0.1);
        assert_eq!(*new, GroundGraph::build_for(&terrain(true), walker()), "整张重建的地图不用旧图");
        assert_eq!(graphs.pending(), 0);
    }

    #[test]
    fn a_map_that_moved_or_resized_drops_its_old_graph() {
        let mut graphs = GroundGraphs::default();
        graphs.get((Entity::PLACEHOLDER, [0; 6], [0; 4]), &terrain(false), walker(), 0.0);
        let moved = graphs.get((Entity::PLACEHOLDER, [0; 6], [1, 0, 8, 3]), &terrain(true), walker(), 0.1);
        assert_eq!(*moved, GroundGraph::build_for(&terrain(true), walker()));
        assert_eq!(graphs.entries.len(), 1);
    }
}
