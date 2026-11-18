//! 换关过场：`scene.transition` 写了以后，换关时先盖上（淡黑或擦除），同时按住新场景、预加载它的资源；
//! 盖满、资源好了、过了最短时长才交给同步重建，再揭开。过场里挡住输入（见 docs/scene-flow.md）。

use bevy_ecs::change_detection::{DetectChanges, Tick};
use bevy_ecs::prelude::*;
use glam::{Vec2, Vec4};
use nge_core::EventKey;
use nge_ecs::{Input, Outbox, Screen, Time};
use nge_render2d::{Camera2d, GIZMO_Z, SpriteImage, SpriteInstance, SpriteInstances, TextureId};
use serde::{Deserialize, Serialize};

use crate::SceneCamera;
use crate::preload::{Failed, Preloading, failed, uris_of};
use crate::protocol::{Color, Scene};

pub const TRANSITION: EventKey<TransitionEvent> = EventKey::new("transition");

/// 盖子画在所有精灵、调试线上面。
const COVER_Z: f32 = GIZMO_Z * 2.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransitionKind {
    /// 整屏变成 `color`。
    #[default]
    Fade,
    /// 从左往右盖上，再从左往右揭开。
    Wipe,
}

/// `scene.transition`。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TransitionNode {
    pub kind: TransitionKind,
    pub color: Option<Color>,
    /// 盖上用几秒。
    pub out: f32,
    /// 揭开用几秒。
    #[serde(rename = "in")]
    pub into: f32,
    /// 从开始盖到真正换关最少几秒。
    pub min: f32,
    /// 另外要等的资源。
    pub preload: Vec<String>,
    /// 资源最多等几秒：到了还没加载完的记成失败，照样换关。
    pub timeout: f32,
}

impl Default for TransitionNode {
    fn default() -> Self {
        Self { kind: TransitionKind::Fade, color: None, out: 0.3, into: 0.3, min: 0.0, preload: Vec::new(), timeout: 10.0 }
    }
}

/// 事件 `transition`：`out` 开始盖（之后加载进度变了再发）、`in` 换好了开始揭开、`done` 揭开了。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransitionEvent {
    pub phase: &'static str,
    pub level: Option<String>,
    pub done: usize,
    pub total: usize,
    pub failed: Vec<Failed>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Out,
    In,
}

/// 过场的状态。Rust 系统可以问 [`SceneTransition::active`]。
#[derive(Resource, Default)]
pub struct SceneTransition {
    node: TransitionNode,
    phase: Phase,
    cover: f32,
    elapsed: f32,
    /// 已经交给同步的关卡；`None` 是还没见过场景。
    applied: Option<Option<String>>,
    loading: Option<Preloading>,
    /// 正在加载的那一关：盖着的时候又换了一关，重开加载组。
    target: Option<String>,
    reported: Option<(usize, usize, usize)>,
    /// 上一次看到的场景改动；和它不一样说明场景又变了。
    seen: Option<Tick>,
}

impl SceneTransition {
    /// 正在过场（盖上、等资源、揭开）。
    pub fn active(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// 盖住了多少：0 … 1。
    pub fn cover(&self) -> f32 {
        self.cover
    }

    /// 按住新场景：换关和同步都先不跑。
    pub(crate) fn holding(&self) -> bool {
        self.phase == Phase::Out
    }
}

/// 场景变了、又没在过场里按住：同步系统的运行条件，代替 `resource_changed::<Scene>`。
/// 自己写的、按 `Scene` 同步的 Rust 系统也要用它，不然过场按住新场景时会提前同步（见 docs/scene-flow.md）。
pub fn scene_ready(scene: Res<Scene>, transition: Res<SceneTransition>) -> bool {
    scene.is_changed() && !transition.holding()
}

/// 每帧跑：看场景有没有换关，推进过场，挡住或放开输入。
pub(crate) fn run_transition(world: &mut World) {
    let dt = world.resource::<Time>().real_delta;
    world.resource_scope(|world, mut t: Mut<SceneTransition>| {
        let tick = world.resource_ref::<Scene>().last_changed();
        if t.seen != Some(tick) {
            t.seen = Some(tick);
            on_scene(world, &mut t);
        }
        step(world, &mut t, dt);
        let active = t.active();
        if world.resource::<Input>().blocked() != active {
            world.resource_mut::<Input>().block(active);
        }
    });
}

fn on_scene(world: &mut World, t: &mut SceneTransition) {
    let (level, node) = {
        let scene = world.resource::<Scene>();
        (scene.level.clone(), scene.transition.clone())
    };
    let Some(Some(applied)) = t.applied.clone() else {
        // 第一次声明关卡（之前没有场景，或者场景里没写 level）：不盖，直接从盖满揭开
        t.applied = Some(level.clone());
        if let (Some(_), Some(node)) = (&level, node) {
            t.node = node;
            t.cover = 1.0;
            t.phase = Phase::In;
            send(world, "in", level, None);
        }
        return;
    };
    if Some(&applied) == level.as_ref() {
        // 盖的时候又换回了正在玩的那一关：不重建，揭开
        if t.holding() {
            release(world, t);
        }
        return;
    }
    match node.filter(|_| level.is_some()) {
        Some(node) => {
            t.node = node;
            if !t.holding() {
                t.phase = Phase::Out;
                t.elapsed = 0.0;
            }
            if !t.holding() || t.target != level {
                // 新开一组：盖着的时候又换了一关，不再等中间那一关的资源
                t.loading = Some(Preloading::default());
                t.reported = None;
                t.target = level;
            }
            // 贴图开关先设好：预加载的贴图一上传就是要的样子
            if let Err(e) = world.run_system_cached(crate::sync::sync_textures) {
                log::error!("scene.textures 没设上：{e}");
            }
            let uris = uris_of(world.resource::<Scene>());
            let loading = t.loading.get_or_insert_with(Preloading::default);
            for uri in uris {
                loading.add(world, &uri);
            }
        }
        // 没写过场：和以前一样当帧换
        None if t.holding() => release(world, t),
        None => t.applied = Some(level),
    }
}

fn step(world: &mut World, t: &mut SceneTransition, dt: f32) {
    match t.phase {
        Phase::Idle => {}
        Phase::Out => {
            t.cover = if t.node.out > 0.0 { (t.cover + dt / t.node.out).min(1.0) } else { 1.0 };
            t.elapsed += dt;
            let mut progress = t.loading.as_mut().map(|l| l.poll(world)).unwrap_or_default();
            if !progress.finished() && t.elapsed >= t.node.timeout {
                // 超时：没加载完的记成失败，照样放开
                let reason = format!("超时：{} 秒还没加载完", t.node.timeout);
                let pending = t.loading.as_ref().map(|l| l.pending(world)).unwrap_or_default();
                progress.failed.extend(pending.into_iter().map(|uri| (uri, reason.clone())));
                progress.done = progress.total;
            }
            let now = (progress.done, progress.total, progress.failed.len());
            if t.reported != Some(now) {
                t.reported = Some(now);
                let level = world.resource::<Scene>().level.clone();
                send(world, "out", level, Some(&progress));
            }
            if t.cover >= 1.0 && progress.finished() && t.elapsed >= t.node.min {
                release(world, t);
            }
        }
        Phase::In => {
            t.cover = if t.node.into > 0.0 { (t.cover - dt / t.node.into).max(0.0) } else { 0.0 };
            if t.cover <= 0.0 {
                t.phase = Phase::Idle;
                let level = t.applied.clone().flatten();
                send(world, "done", level, None);
            }
        }
    }
}

/// 放开按住的场景：标记成变了，同一帧里换关、同步照常跑；开始揭开。
fn release(world: &mut World, t: &mut SceneTransition) {
    world.resource_mut::<Scene>().set_changed();
    t.seen = Some(world.resource_ref::<Scene>().last_changed());
    let level = world.resource::<Scene>().level.clone();
    t.applied = Some(level.clone());
    t.loading = None;
    t.target = None;
    t.phase = Phase::In;
    send(world, "in", level, None);
}

fn send(world: &mut World, phase: &'static str, level: Option<String>, progress: Option<&nge_assets::LoadProgress>) {
    let (done, total, failed) = progress.map_or((0, 0, Vec::new()), |p| (p.done, p.total, failed(p)));
    world.resource_mut::<Outbox>().send(TRANSITION, &TransitionEvent { phase, level, done, total, failed });
}

/// 画盖子：跟着相机，盖满视野（多 2 point）。淡黑整块乘透明度；擦除盖住左边（揭开时盖住右边）`cover` 那么宽。
pub(crate) fn draw_cover(t: Res<SceneTransition>, cameras: Query<&Camera2d, With<SceneCamera>>, screen: Res<Screen>, mut sprites: ResMut<SpriteInstances>) {
    if t.cover <= 0.0 {
        return;
    }
    let camera = cameras.iter().next().copied().unwrap_or_default();
    let zoom = camera.zoom.max(1e-4);
    let full = (screen.size.max(Vec2::ONE) + 2.0) / zoom;
    let color = t.node.color.map_or(Vec4::new(0.0, 0.0, 0.0, 1.0), |c| c.0);
    let (local, size, alpha) = match t.node.kind {
        TransitionKind::Fade => (Vec2::ZERO, full, t.cover),
        TransitionKind::Wipe => {
            let w = full.x * t.cover;
            let x = if t.phase == Phase::In { (full.x - w) / 2.0 } else { (w - full.x) / 2.0 };
            (Vec2::new(x, 0.0), Vec2::new(w, full.y), 1.0)
        }
    };
    let position = camera.position + Vec2::from_angle(camera.rotation).rotate(local);
    sprites.push(
        SpriteImage::Texture(TextureId::WHITE),
        SpriteInstance { position, size, rotation: camera.rotation, z: COVER_Z, color: color.with_w(color.w * alpha) },
    );
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use glam::Vec2;
    use nge_render2d::{SpriteInstances, Transform2d};
    use serde_json::{Value, json};

    use super::*;
    use crate::sync::SceneId;
    use crate::testing::{PNG, SLOW, app, frame};

    fn phases(events: &[(String, Value)]) -> Vec<String> {
        events.iter().filter(|(n, _)| n == "transition").map(|(_, v)| v["phase"].as_str().unwrap().to_owned()).collect()
    }

    fn level_starts(events: &[(String, Value)]) -> usize {
        events.iter().filter(|(n, _)| n == "levelStart").count()
    }

    fn has(app: &mut nge_ecs::App, id: &str) -> bool {
        let world = app.world_mut();
        world.query::<&SceneId>().iter(world).any(|s| s.0 == id)
    }

    fn cover(app: &nge_ecs::App) -> Option<SpriteInstance> {
        app.world().resource::<SpriteInstances>().iter().find(|(_, s)| s.z == COVER_Z).map(|(_, s)| *s)
    }

    fn level(n: u32, transition: Value) -> Value {
        json!({ "level": n, "transition": transition, "sprites": [{ "id": format!("only-{n}") }] })
    }

    /// 带墙钟期限地跑帧，直到 `done`（最多 15 秒），返回一路上的事件。
    fn run_until(app: &mut nge_ecs::App, mut done: impl FnMut(&mut nge_ecs::App, &[(String, Value)]) -> bool) -> Vec<(String, Value)> {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut all = Vec::new();
        loop {
            let events = frame(app, None);
            all.extend(events.clone());
            if done(app, &events) {
                return all;
            }
            assert!(Instant::now() < deadline, "15 秒还没好：{all:?}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// 等开场的揭开放完。
    fn settle(app: &mut nge_ecs::App) {
        run_until(app, |app, _| !app.world().resource::<SceneTransition>().active());
    }

    #[test]
    fn without_a_transition_levels_switch_at_once() {
        let mut app = app();
        frame(&mut app, Some(json!({ "level": 1, "sprites": [{ "id": "only-1" }] })));
        let events = frame(&mut app, Some(json!({ "level": 2, "sprites": [{ "id": "only-2" }] })));
        assert!(has(&mut app, "only-2") && level_starts(&events) == 1 && phases(&events).is_empty());
        assert!(!app.world().resource::<nge_ecs::Input>().blocked());
    }

    #[test]
    fn the_old_level_stays_until_covered_then_the_new_one_fades_in() {
        let mut app = app();
        let t = json!({ "out": 1.0, "in": 1.0 });
        let first = frame(&mut app, Some(level(1, t.clone())));
        assert_eq!(phases(&first), ["in"], "开场从盖满揭开");
        run_until(&mut app, |_, e| phases(e).contains(&"done".to_owned()));
        // 一帧 0.5 秒：盖一半
        let events = frame(&mut app, Some(level(2, t.clone())));
        assert_eq!((phases(&events), level_starts(&events)), (vec!["out".to_owned()], 0));
        assert!(has(&mut app, "only-1") && !has(&mut app, "only-2"), "盖的时候还是上一关");
        let half = cover(&app).expect("画了盖子");
        assert!((half.color.w - 0.5).abs() < 1e-4 && half.color.truncate() == glam::Vec3::ZERO, "默认淡黑：{half:?}");
        let events = frame(&mut app, None);
        assert_eq!((phases(&events), level_starts(&events)), (vec!["in".to_owned()], 1), "盖满就换，同一帧发 levelStart");
        assert!(has(&mut app, "only-2") && !has(&mut app, "only-1"));
        frame(&mut app, None);
        let events = frame(&mut app, None);
        assert_eq!(phases(&events), ["done"]);
        assert!(cover(&app).is_none(), "揭开以后不画");
    }

    #[test]
    fn the_switch_waits_for_preloads_and_the_minimum_time() {
        let mut app = app();
        let t = json!({ "out": 0.1, "in": 0.1, "min": 2.0 });
        frame(&mut app, Some(level(1, t.clone())));
        let mut next = level(2, t);
        next["sprites"] = json!([{ "id": "only-2", "texture": PNG }]);
        let start = app.world().resource::<nge_ecs::Time>().elapsed;
        let events = frame(&mut app, Some(next));
        let out: Vec<&Value> = events.iter().filter(|(n, _)| n == "transition").map(|(_, v)| v).collect();
        assert_eq!(out[0]["total"], json!(1), "新关卡的贴图要等：{out:?}");
        let events = run_until(&mut app, |app, _| has(app, "only-2"));
        let waited = app.world().resource::<nge_ecs::Time>().elapsed - start;
        assert!(waited >= 2.0, "最短时长：{waited}");
        let loaded = app.world_mut().resource_scope(|world, mut server: Mut<nge_assets::AssetServer>| {
            let mut images = world.resource_mut::<nge_assets::Assets<nge_assets::Image>>();
            let handle = server.load(&mut images, PNG);
            images.state(handle) == nge_assets::LoadState::Loaded
        });
        assert!(loaded, "换关时贴图已经好了");
        assert!(events.iter().any(|(n, v)| n == "transition" && v["done"] == json!(1)), "报过加载完");
    }

    #[test]
    fn failed_preloads_do_not_hold_the_switch() {
        let mut app = app();
        let t = json!({ "out": 0.1, "preload": ["asset:/@t/missing.wav", "asset:/@t/what.xyz"] });
        frame(&mut app, Some(level(1, t.clone())));
        frame(&mut app, Some(level(2, t)));
        let events = run_until(&mut app, |app, _| has(app, "only-2"));
        let last = events.iter().filter(|(n, v)| n == "transition" && v["phase"] == json!("out")).map(|(_, v)| v).last().unwrap();
        assert_eq!((last["done"].clone(), last["total"].clone(), last["failed"].as_array().unwrap().len()), (json!(2), json!(2), 2));
    }

    #[test]
    fn input_is_blocked_for_the_whole_transition() {
        let mut app = app();
        let t = json!({ "out": 1.0, "in": 1.0 });
        frame(&mut app, Some(level(1, t.clone())));
        settle(&mut app);
        app.command("key", &json!({ "code": "KeyA", "down": true }));
        frame(&mut app, None);
        assert!(app.world().resource::<nge_ecs::Input>().key_down("KeyA"));
        app.prop("scene", Some(&level(2, t)));
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 1, "y": 1 }));
        frame(&mut app, None);
        let input = app.world().resource::<nge_ecs::Input>();
        assert!(input.blocked() && !input.key_down("KeyA") && input.pointers().next().is_none(), "开始过场的那一帧就挡住");
        let mut blocked = Vec::new();
        run_until(&mut app, |app, e| {
            blocked.push(app.world().resource::<nge_ecs::Input>().blocked());
            phases(e).contains(&"done".to_owned())
        });
        assert!(blocked[..blocked.len() - 1].iter().all(|b| *b), "揭开时也挡着：{blocked:?}");
        assert!(!blocked.last().unwrap(), "揭开以后放开");
    }

    #[test]
    fn going_back_to_the_current_level_while_covering_cancels() {
        let mut app = app();
        let t = json!({ "out": 2.0, "in": 4.0 });
        frame(&mut app, Some(level(1, t.clone())));
        settle(&mut app);
        let (before, ..) = {
            let world = app.world_mut();
            world.query::<(Entity, &SceneId)>().iter(world).find(|(_, s)| s.0 == "only-1").unwrap()
        };
        frame(&mut app, Some(level(2, t.clone())));
        let events = frame(&mut app, Some(level(1, t)));
        assert_eq!((phases(&events), level_starts(&events)), (vec!["in".to_owned()], 0), "揭开、不重建");
        let world = app.world_mut();
        let after = world.query::<(Entity, &SceneId)>().iter(world).find(|(_, s)| s.0 == "only-1").unwrap().0;
        assert_eq!(after, before);
    }

    #[test]
    fn scene_updates_while_covered_are_kept_and_the_latest_wins() {
        let mut app = app();
        let t = json!({ "out": 1.5 });
        frame(&mut app, Some(level(1, t.clone())));
        settle(&mut app);
        frame(&mut app, Some(level(2, t.clone())));
        let mut later = level(2, t);
        later["sprites"] = json!([{ "id": "only-2", "x": 30 }]);
        frame(&mut app, Some(later));
        run_until(&mut app, |app, _| has(app, "only-2"));
        let world = app.world_mut();
        let x = world.query::<(&SceneId, &Transform2d)>().iter(world).find(|(s, _)| s.0 == "only-2").unwrap().1.translation.x;
        assert_eq!(x, 30.0);
    }

    #[test]
    fn a_wipe_covers_from_the_left_and_uncovers_to_the_right() {
        let mut app = app();
        let t = json!({ "kind": "wipe", "out": 1.0, "in": 1.0, "color": "#ff0000" });
        frame(&mut app, Some(json!({ "level": 1, "transition": t, "camera": { "x": 500, "y": 0, "zoom": 2 } })));
        let entering = cover(&app).unwrap();
        // 视口 100：缩放 2 时视野 51 宽；揭开一半，盖住右边一半
        assert!((entering.size.x - 25.5).abs() < 1e-3 && entering.position.x > 500.0, "{entering:?}");
        assert_eq!(entering.color, glam::Vec4::new(1.0, 0.0, 0.0, 1.0), "擦除不淡");
        run_until(&mut app, |_, e| phases(e).contains(&"done".to_owned()));
        frame(&mut app, Some(json!({ "level": 2, "transition": t, "camera": { "x": 500, "y": 0, "zoom": 2 } })));
        let leaving = cover(&app).unwrap();
        assert!((leaving.size.x - 25.5).abs() < 1e-3 && leaving.position.x < 500.0, "盖住左边一半：{leaving:?}");
        assert_eq!(leaving.position.y, 0.0);
    }

    #[test]
    fn sync_waits_while_held_but_runs_once_released() {
        let mut app = app();
        let t = json!({ "out": 1.0 });
        frame(&mut app, Some(level(1, t.clone())));
        settle(&mut app);
        let mut next = level(2, t);
        next["camera"] = json!({ "x": 77 });
        frame(&mut app, Some(next));
        let world = app.world_mut();
        let cam = world.query::<&nge_render2d::Camera2d>().single(world).unwrap().position;
        assert_eq!(cam, Vec2::ZERO, "按住时相机也不动");
        run_until(&mut app, |app, _| has(app, "only-2"));
        let world = app.world_mut();
        assert_eq!(world.query::<&nge_render2d::Camera2d>().single(world).unwrap().position.x, 77.0);
    }

    fn png_loaded(app: &mut nge_ecs::App) -> (nge_assets::ImageId, bool) {
        app.world_mut().resource_scope(|world, mut server: Mut<nge_assets::AssetServer>| {
            let mut images = world.resource_mut::<nge_assets::Assets<nge_assets::Image>>();
            let handle = server.load(&mut images, PNG);
            (handle, images.state(handle) == nge_assets::LoadState::Loaded)
        })
    }

    fn holding(app: &nge_ecs::App) -> bool {
        app.world().resource::<SceneTransition>().holding()
    }

    #[test]
    fn the_new_lighting_waits_for_the_switch() {
        let mut app = app();
        let t = json!({ "out": 1.0 });
        frame(&mut app, Some(level(1, t.clone())));
        settle(&mut app);
        let mut next = level(2, t);
        next["lighting"] = json!({ "ambient": "#000000" });
        frame(&mut app, Some(next));
        assert!(holding(&app) && has(&mut app, "only-1"));
        assert!(!app.world().resource::<nge_render2d::Lighting2d>().enabled, "按住时上一关不被新关卡的光照照着");
        run_until(&mut app, |app, _| has(app, "only-2"));
        assert!(app.world().resource::<nge_render2d::Lighting2d>().enabled, "换了就照");
    }

    #[test]
    fn the_same_unknown_uri_counts_once_while_held() {
        let mut app = app();
        let t = json!({ "out": 100.0, "preload": ["asset:/@t/what.xyz"] });
        frame(&mut app, Some(level(1, t.clone())));
        settle(&mut app);
        let mut last = None;
        for i in 0..4 {
            let mut s = level(2, t.clone());
            s["sprites"][0]["x"] = json!(i);
            for (n, v) in frame(&mut app, Some(s)) {
                if n == "transition" {
                    last = Some((v["total"].clone(), v["failed"].as_array().unwrap().len()));
                }
            }
        }
        assert_eq!(last, Some((json!(1), 1)));
    }

    #[test]
    fn the_first_level_fades_in_even_after_frames_without_one() {
        let mut app = app();
        frame(&mut app, None);
        frame(&mut app, Some(json!({ "sprites": [] })));
        let events = frame(&mut app, Some(level(1, json!({ "out": 1.0, "in": 1.0 }))));
        assert_eq!(phases(&events), ["in"], "第一次声明关卡：从盖满揭开");
        assert!(has(&mut app, "only-1"));
    }

    #[test]
    fn scene_textures_apply_before_the_preload_uploads() {
        let mut app = app();
        let t = json!({ "out": 0.1, "min": 3.0 });
        frame(&mut app, Some(level(1, t.clone())));
        settle(&mut app);
        let mut next = level(2, t);
        next["sprites"] = json!([{ "id": "only-2", "texture": PNG }]);
        next["textures"] = json!({ PNG: { "mipmaps": false } });
        frame(&mut app, Some(next));
        run_until(&mut app, |app, _| png_loaded(app).1);
        assert!(holding(&app));
        let (id, _) = png_loaded(&mut app);
        assert_eq!(app.world().resource::<nge_assets::GpuImages>().mipmaps(id), Some(false), "贴图加载好、渲染器上传之前开关已经设了");
    }

    #[test]
    fn a_load_that_never_ends_times_out_and_lets_go() {
        let mut app = app();
        let t = json!({ "out": 0.1, "timeout": 2.0 });
        frame(&mut app, Some(level(1, t.clone())));
        settle(&mut app);
        let mut next = level(2, t);
        next["sprites"] = json!([{ "id": "only-2", "texture": SLOW }]);
        frame(&mut app, Some(next));
        let mut frames = 0;
        let events = run_until(&mut app, |app, _| {
            frames += 1;
            has(app, "only-2")
        });
        // 一帧 0.5 秒：2 秒超时
        assert!(frames <= 5, "{frames} 帧才放开");
        let out = events.iter().filter(|(n, v)| n == "transition" && v["phase"] == json!("out")).map(|(_, v)| v).last().unwrap();
        assert_eq!((out["done"].clone(), out["total"].clone()), (json!(1), json!(1)), "{out}");
        assert_eq!(out["failed"][0]["uri"], json!(SLOW));
        assert!(out["failed"][0]["error"].as_str().unwrap().contains("超时"), "{out}");
    }

    #[test]
    fn updates_while_held_do_not_restart_the_minimum_time() {
        let mut app = app();
        let t = json!({ "out": 0.1, "min": 2.0 });
        frame(&mut app, Some(level(1, t.clone())));
        settle(&mut app);
        let mut frames = 0;
        while !has(&mut app, "only-2") {
            frames += 1;
            assert!(frames < 20, "一直在更新场景就一直换不了");
            let mut s = level(2, t.clone());
            s["sprites"][0]["x"] = json!(frames);
            frame(&mut app, Some(s));
        }
        assert!(frames <= 5, "{frames} 帧");
    }

    #[test]
    fn switching_again_while_covered_stops_waiting_for_the_middle_level() {
        let mut app = app();
        let t = json!({ "out": 0.1, "timeout": 100.0 });
        frame(&mut app, Some(level(1, t.clone())));
        settle(&mut app);
        let mut b = level(2, t.clone());
        b["sprites"] = json!([{ "id": "only-2", "texture": SLOW }]);
        frame(&mut app, Some(b));
        let mut frames = 0;
        let events = run_until(&mut app, |app, _| {
            frames += 1;
            if frames == 1 {
                app.prop("scene", Some(&level(3, t.clone())));
            }
            has(app, "only-3")
        });
        assert!(frames <= 3, "不等第 2 关的贴图（{frames} 帧）：{events:?}");
    }
}
