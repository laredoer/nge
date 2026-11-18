//! 预加载：一组资源在后台加载，报进度（已完成 / 总数），失败的算完成并报出来（见 docs/scene-flow.md）。

use bevy_ecs::prelude::*;
use nge_assets::{Asset, AssetServer, Assets, Handle, Image, LoadGroup, LoadProgress, LoadState};
use nge_audio::Sound;
use nge_core::{CommandKey, EventKey};
use nge_ecs::Outbox;
use nge_render3d::Gltf;
use nge_text::Font;
use serde::{Deserialize, Serialize};

use crate::prefab::SceneFile;
use crate::protocol::{SCENE, Scene};

pub const PRELOAD: CommandKey<Preload> = CommandKey::new("preload");
pub const PRELOADED: EventKey<PreloadProgress> = EventKey::new("preload");

/// 命令 `preload`：开一组预加载；同一个 `id` 再发一次是重新开始。
#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preload {
    pub id: String,
    #[serde(default)]
    pub uris: Vec<String>,
}

/// 事件 `preload`：进度变了时发，第一次检查一定发一次。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreloadProgress {
    pub id: String,
    pub done: usize,
    pub total: usize,
    pub failed: Vec<Failed>,
    pub finished: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Failed {
    pub uri: String,
    pub error: String,
}

pub(crate) fn failed(progress: &LoadProgress) -> Vec<Failed> {
    progress.failed.iter().map(|(uri, error)| Failed { uri: uri.clone(), error: error.clone() }).collect()
}

/// 按扩展名认出的资源类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Image,
    Sound,
    Model,
    Font,
    Json,
}

pub(crate) fn kind_of(uri: &str) -> Option<Kind> {
    let ext = uri.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" | "jpg" | "jpeg" | "ktx2" => Kind::Image,
        "wav" | "ogg" | "mp3" | "flac" => Kind::Sound,
        "gltf" | "glb" => Kind::Model,
        "ttf" | "otf" | "ttc" => Kind::Font,
        "json" => Kind::Json,
        _ => return None,
    })
}

/// 场景（或预制体）引用的 `asset:/` 资源：贴图、法线图、字体、模型、背景音乐、预制体文件、`transition.preload`、`textures` 的键。
pub(crate) fn uris_of(scene: &Scene) -> Vec<String> {
    let mut uris: Vec<String> = Vec::new();
    collect(scene, &mut uris, 0);
    uris.retain(|u| u.starts_with("asset:/"));
    let mut seen = std::collections::HashSet::new();
    uris.retain(|u| seen.insert(u.clone()));
    uris
}

fn collect(scene: &Scene, out: &mut Vec<String>, depth: usize) {
    for s in &scene.sprites {
        out.extend(s.texture.iter().chain(&s.normal).cloned());
    }
    out.extend(scene.tilemaps.iter().map(|t| t.texture.clone()));
    out.extend(scene.texts.iter().filter_map(|t| t.font.clone()));
    out.extend(scene.bubbles.iter().filter_map(|b| b.font.clone()));
    out.extend(scene.emitters.iter().filter_map(|e| e.particles.texture.clone()));
    out.extend(scene.emitters3d.iter().filter_map(|e| e.texture.clone()));
    out.extend(scene.models.iter().map(|m| m.model.clone()));
    if let Some(audio) = &scene.audio {
        out.extend(audio.music.iter().chain(&audio.preload).cloned());
    }
    out.extend(scene.instances.iter().map(|i| i.prefab.clone()));
    if let Some(t) = &scene.transition {
        out.extend(t.preload.iter().cloned());
    }
    out.extend(scene.textures.keys().cloned());
    // 写在场景里的预制体：里面的节点还没展开
    if depth < 4 {
        for def in scene.prefabs.values() {
            if let Ok(inner) = SCENE.decode(def) {
                collect(&inner, out, depth + 1);
            }
        }
    }
}

/// 一组预加载：资源组，加上 JSON 文件加载好以后再加进它引用的资源。
#[derive(Default)]
pub(crate) struct Preloading {
    group: LoadGroup,
    files: Vec<(Handle<SceneFile>, bool)>,
}

impl Preloading {
    pub fn add(&mut self, world: &mut World, uri: &str) {
        match kind_of(uri) {
            Some(Kind::Image) => drop(start::<Image>(world, &mut self.group, uri)),
            Some(Kind::Sound) => drop(start::<Sound>(world, &mut self.group, uri)),
            Some(Kind::Model) => drop(start::<Gltf>(world, &mut self.group, uri)),
            Some(Kind::Font) => drop(start::<Font>(world, &mut self.group, uri)),
            Some(Kind::Json) => {
                if let Some(file) = start::<SceneFile>(world, &mut self.group, uri) {
                    self.files.push((file, false));
                }
            }
            None => self.group.fail(uri, "认不出资源类型（按扩展名：png、jpg、ktx2、wav、ogg、mp3、flac、gltf、glb、ttf、otf、json）"),
        }
    }

    /// 先把加载好的 JSON 文件引用的资源加进来，再算进度。
    pub fn poll(&mut self, world: &mut World) -> LoadProgress {
        loop {
            let ready: Vec<usize> = {
                let files = world.resource::<Assets<SceneFile>>();
                self.files.iter().enumerate().filter(|(_, (h, seen))| !seen && files.state(*h) != LoadState::Loading).map(|(i, _)| i).collect()
            };
            if ready.is_empty() {
                return self.group.progress(world);
            }
            for i in ready {
                self.files[i].1 = true;
                let inner = world.resource::<Assets<SceneFile>>().get(self.files[i].0).and_then(|f| SCENE.decode(&f.0).ok());
                for uri in inner.as_ref().map(uris_of).unwrap_or_default() {
                    self.add(world, &uri);
                }
            }
        }
    }
}

/// 开始加载，记进组里；第一次见到这个句柄时返回它。
fn start<A: Asset>(world: &mut World, group: &mut LoadGroup, uri: &str) -> Option<Handle<A>> {
    if !world.contains_resource::<Assets<A>>() {
        group.fail(uri, format!("{} 没有注册", std::any::type_name::<A>()));
        return None;
    }
    let handle = world.resource_scope(|world, mut server: Mut<AssetServer>| server.load(&mut world.resource_mut::<Assets<A>>(), uri));
    group.add(uri, handle).then_some(handle)
}

/// 正在进行的 `preload` 命令：id、这一组、上一次报的（完成数、总数、失败数）。
#[derive(Resource, Default)]
pub(crate) struct Preloads(Vec<(String, Preloading, Option<(usize, usize, usize)>)>);

/// 收 `preload` 命令开组；每帧查进度，变了发事件，完成后扔掉。
pub(crate) fn preload(world: &mut World) {
    let started: Vec<Preload> = world.resource_mut::<Messages<Preload>>().drain().collect();
    world.resource_scope(|world, mut preloads: Mut<Preloads>| {
        for Preload { id, uris } in started {
            let mut loading = Preloading::default();
            for uri in &uris {
                loading.add(world, uri);
            }
            preloads.0.retain(|(i, ..)| *i != id);
            preloads.0.push((id, loading, None));
        }
        preloads.0.retain_mut(|(id, loading, reported)| {
            let progress = loading.poll(world);
            let now = (progress.done, progress.total, progress.failed.len());
            if *reported != Some(now) {
                *reported = Some(now);
                let event = PreloadProgress { id: id.clone(), done: progress.done, total: progress.total, failed: failed(&progress), finished: progress.finished() };
                world.resource_mut::<Outbox>().send(PRELOADED, &event);
            }
            !progress.finished()
        });
    });
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};

    use super::*;
    use crate::testing::{BEEP, PACK, PNG, TREE, app, frame};

    fn progress(events: &[(String, Value)]) -> Vec<Value> {
        events.iter().filter(|(n, _)| n == "preload").map(|(_, v)| v.clone()).collect()
    }

    /// 按墙钟跑帧，收集 `preload` 事件，直到 `id` 这一组完成（最多 15 秒）。
    fn run_until_finished(app: &mut nge_ecs::App, id: &str) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut seen = Vec::new();
        loop {
            seen.extend(progress(&frame(app, None)));
            if seen.iter().any(|e| e["id"] == json!(id) && e["finished"] == json!(true)) {
                return seen;
            }
            assert!(Instant::now() < deadline, "15 秒还没完成：{seen:?}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn kinds_follow_the_extension() {
        assert_eq!(kind_of("asset:/@g/a.PNG"), Some(Kind::Image));
        assert_eq!(kind_of("asset:/@g/hills.ktx2"), Some(Kind::Image));
        assert_eq!(kind_of("asset:/@g/a.ogg"), Some(Kind::Sound));
        assert_eq!(kind_of("asset:/@g/a.glb"), Some(Kind::Model));
        assert_eq!(kind_of("asset:/@g/a.otf"), Some(Kind::Font));
        assert_eq!(kind_of("asset:/@g/levels/2.json"), Some(Kind::Json));
        assert_eq!(kind_of("asset:/@g/a.txt"), None);
        assert_eq!(kind_of("asset:/@g/noext"), None);
    }

    #[test]
    fn a_scene_lists_the_assets_it_uses() {
        let scene = SCENE
            .decode(&json!({
                "sprites": [{ "id": "a", "texture": "asset:/@g/a.png", "normal": "asset:/@g/n.png" }, { "id": "b", "texture": "named" }, { "id": "c", "texture": "asset:/@g/a.png" }],
                "tilemaps": [{ "id": "m", "texture": "asset:/@g/tiles.png", "tile": 8, "cols": 1, "data": [0] }],
                "texts": [{ "id": "t", "text": "x", "font": "asset:/@g/f.ttf" }],
                "models": [{ "id": "box", "model": "asset:/@g/box.glb" }],
                "audio": { "music": "asset:/@g/m.ogg", "preload": ["asset:/@g/hit.wav"] },
                "instances": [{ "id": "i", "prefab": "asset:/@g/tree.json" }, { "id": "j", "prefab": "inline" }],
                "prefabs": { "inline": { "sprites": [{ "id": "s", "texture": "asset:/@g/inner.png" }] } },
                "transition": { "preload": ["asset:/@g/boss.wav"] },
                "textures": { "asset:/@g/pixel.png": { "mipmaps": false } },
            }))
            .unwrap();
        let mut uris = uris_of(&scene);
        uris.sort();
        assert_eq!(
            uris,
            ["a.png", "boss.wav", "box.glb", "f.ttf", "hit.wav", "inner.png", "m.ogg", "n.png", "pixel.png", "tiles.png", "tree.json"].map(|f| format!("asset:/@g/{f}")),
            "注册的纹理名不算，重复的只算一次"
        );
    }

    #[test]
    fn progress_counts_failures_as_done_and_follows_json_files() {
        let mut app = app();
        app.command("preload", &json!({ "id": "next", "uris": [PNG, BEEP, "asset:/@t/missing.png", "asset:/@t/what.xyz", PACK] }));
        let events = run_until_finished(&mut app, "next");
        let first = &events[0];
        assert_eq!((first["id"].clone(), first["finished"].clone()), (json!("next"), json!(false)), "第一次检查就发");
        let last = events.last().unwrap();
        // PACK 引用了 PNG（已经在组里）和 TREE
        assert_eq!((last["done"].clone(), last["total"].clone()), (json!(6), json!(6)), "{events:?}");
        let failed: Vec<&str> = last["failed"].as_array().unwrap().iter().map(|f| f["uri"].as_str().unwrap()).collect();
        assert_eq!(failed, ["asset:/@t/missing.png", "asset:/@t/what.xyz"]);
        assert!(last["failed"][1]["error"].as_str().unwrap().contains("认不出"));
        let totals: Vec<u64> = events.iter().map(|e| e["total"].as_u64().unwrap()).collect();
        assert!(totals.windows(2).all(|w| w[0] <= w[1]), "总数只会变大：{totals:?}");
        assert!(events.windows(2).all(|w| w[0] != w[1]), "进度没变不重复发：{events:?}");
        assert!(progress(&frame(&mut app, None)).is_empty(), "完成以后不再发");
        let world = app.world();
        assert!(world.resource::<Assets<SceneFile>>().iter().count() >= 2, "PACK 和它引用的 TREE 都加载了");
        let _ = TREE;
    }

    #[test]
    fn an_empty_or_cached_group_finishes_at_once() {
        let mut app = app();
        app.command("preload", &json!({ "id": "none", "uris": [] }));
        assert_eq!(progress(&frame(&mut app, None)), [json!({ "id": "none", "done": 0, "total": 0, "failed": [], "finished": true })]);
        app.command("preload", &json!({ "id": "warm", "uris": [PNG] }));
        run_until_finished(&mut app, "warm");
        app.command("preload", &json!({ "id": "again", "uris": [PNG] }));
        let events = progress(&frame(&mut app, None));
        assert_eq!(events, [json!({ "id": "again", "done": 1, "total": 1, "failed": [], "finished": true })], "已经加载过的当帧完成");
        assert!(app.world().resource::<Preloads>().0.is_empty(), "完成的组扔掉");
    }
}
