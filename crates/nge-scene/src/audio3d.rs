//! 声明式的声源（`scene.audio.emitters`）、声音跟着精灵 / 模型走、3D 的听者、遮挡的射线（见 docs/audio-3d.md）。

use std::collections::HashSet;

use bevy_ecs::prelude::*;
use glam::Vec3;
use nge_assets::{AssetServer, Assets, Handle};
use nge_audio::{Audio, Cone, Emit, Play, Rolloff, Sound, Voice};
use nge_physics2d::Physics;
use nge_render2d::{DrawAt, Transform2d};
use nge_render3d::Transform3d;
use serde::Deserialize;

use crate::SceneCamera;
use crate::audio::{BusName, PlaySound};
use crate::nodes::ModelNode;
use crate::protocol::{Scene, SpriteNode};
use crate::sync::{Declared, SceneId};

/// 声源出现、消失时默认淡入淡出多久；只改音量时过渡多久；跟着的实体没了、一次性的音效停下用多久。
const EMITTER_FADE: f32 = 0.3;
const VOLUME_FADE: f32 = 0.2;
const LOST_FADE: f32 = 0.1;
/// 遮挡的射线最多穿过几个不算遮挡的东西（动态、kinematic 的刚体）。
const MAX_HOPS: usize = 4;
/// 射线离声源还差这么远就停：不打到声源自己身上。
const RAY_MARGIN: f32 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub enum SpaceKind {
    /// 2D：位置是 `x`、`y`，跟着精灵，听的人在 2D 相机上。
    #[default]
    #[serde(rename = "2d")]
    D2,
    /// 3D：位置是 `x`、`y`、`z`，跟着模型，听的人在 3D 相机上。
    #[serde(rename = "3d")]
    D3,
}

/// 跟着的实体没了以后怎么办。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnLost {
    /// 淡出停掉。
    #[default]
    Stop,
    /// 留在最后的位置播完。
    Finish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RolloffName {
    Linear,
    #[default]
    Inverse,
    Exponential,
}

/// 锥形指向，角度是度（整个锥的张角）。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConeNode {
    pub inner: f32,
    pub outer: f32,
    #[serde(default)]
    pub outer_gain: f32,
    /// 朝哪（世界坐标）；不写时用跟着的模型的朝向（模型朝 -z），没跟着就是 -z。
    #[serde(default)]
    pub direction: Option<[f32; 3]>,
}

/// 声源和 `sound` 命令共用的一组：在哪个空间、跟着谁、衰减、锥形、多普勒、遮挡。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SpaceNode {
    pub space: SpaceKind,
    pub z: Option<f32>,
    /// 跟着这个 id 的精灵（2D）或模型（3D）走；这时 `x`、`y`、`z` 是相对它的偏移。
    pub follow: Option<String>,
    pub on_lost: OnLost,
    pub rolloff: RolloffName,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub factor: Option<f32>,
    pub cone: Option<ConeNode>,
    /// 多普勒：0 关，1 真实，大于 1 夸张。
    pub doppler: f32,
    pub occlusion: bool,
}

impl SpaceNode {
    /// 没写的给 NaN（`Emit::sane` 换成默认值）。`forward` 是跟着的模型的朝向。
    pub(crate) fn emit(&self, forward: Option<Vec3>) -> Emit {
        let or = |v: Option<f32>| v.unwrap_or(f32::NAN);
        let cone = self.cone.map(|c| Cone { inner: c.inner.to_radians(), outer: c.outer.to_radians(), outer_gain: c.outer_gain });
        let forward = self.cone.and_then(|c| c.direction).map(Vec3::from).or(forward).unwrap_or(Vec3::NEG_Z);
        let rolloff = match self.rolloff {
            RolloffName::Linear => Rolloff::Linear,
            RolloffName::Inverse => Rolloff::Inverse,
            RolloffName::Exponential => Rolloff::Exponential,
        };
        Emit { rolloff, min: or(self.min), max: or(self.max), factor: or(self.factor), cone, forward, doppler: self.doppler, occlude: self.occlusion }.sane()
    }
}

fn yes() -> bool {
    true
}

fn one() -> f32 {
    1.0
}

fn emitter_fade() -> f32 {
    EMITTER_FADE
}

/// 一个声明式的声源：场景里有就响，没了就停。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SoundEmitterNode {
    pub id: String,
    pub src: String,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    /// 默认循环。不循环的出现时播一次。
    #[serde(default = "yes", rename = "loop")]
    pub looped: bool,
    #[serde(default = "one")]
    pub volume: f32,
    #[serde(default = "one")]
    pub pitch: f32,
    #[serde(default)]
    pub bus: BusName,
    /// 出现、消失时淡入淡出的秒数。
    #[serde(default = "emitter_fade")]
    pub fade: f32,
    #[serde(flatten)]
    pub space: SpaceNode,
}

impl SoundEmitterNode {
    /// 这些变了要重新开始播；别的原地改。
    fn restarts(&self, other: &Self) -> bool {
        (&self.src, self.looped, self.bus, self.space.space, self.space.occlusion) != (&other.src, other.looped, other.bus, other.space.space, other.space.occlusion)
    }
}

/// 听的人放在哪个精灵（`space: "2d"`）或模型（`"3d"`）上。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ListenerNode {
    pub follow: String,
    #[serde(default)]
    pub space: SpaceKind,
}

/// 跟着的实体现在的样子。
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pose {
    entity: Entity,
    at: Vec3,
    /// 模型的朝向（-z 转过以后）；精灵没有。
    forward: Option<Vec3>,
}

/// 跟着一个实体的状态。
#[derive(Debug, Clone, Copy, Default)]
struct Following {
    /// 实体出现过（没出现过的不算丢）。
    seen: bool,
    lost: bool,
    entity: Option<Entity>,
}

/// 丢了没有、现在该在哪。
enum Place {
    /// 没跟着谁，或者跟着的实体在：声音在这儿。
    At(Vec3, Option<Vec3>),
    /// 跟着的实体还没出现过：等着。
    Waiting,
    /// 这一帧刚丢。
    JustLost,
    /// 早就丢了。
    Lost,
}

impl Following {
    fn place(&mut self, space: &SpaceNode, offset: Vec3, sprites: &Sprites, models: &Models) -> Place {
        let Some(id) = space.follow.as_deref() else { return Place::At(offset, None) };
        match pose(space.space, id, self.entity, sprites, models) {
            Some(pose) => {
                (self.seen, self.lost, self.entity) = (true, false, Some(pose.entity));
                Place::At(pose.at + offset, pose.forward)
            }
            None if !self.seen => Place::Waiting,
            None if self.lost => Place::Lost,
            None => {
                (self.lost, self.entity) = (true, None);
                Place::JustLost
            }
        }
    }
}

struct Emitter {
    node: SoundEmitterNode,
    voice: Option<Voice>,
    /// 不循环的已经播过了：不再播。
    played: bool,
    following: Following,
}

/// `sound` 命令放的、跟着实体走的一次性音效。
struct Follower {
    voice: Voice,
    space: SpaceNode,
    offset: Vec3,
    following: Following,
}

/// 场景里的声源和跟着实体走的音效。
#[derive(Resource, Default)]
pub(crate) struct SoundEmitters {
    emitters: Vec<Emitter>,
    followers: Vec<Follower>,
    /// 这一帧 `sound` 命令要放的、带 `follow` 的：位置定了再开始。
    queued: Vec<(Handle<Sound>, Play, PlaySound)>,
    /// `listener.follow` 上一帧找到的实体。
    listener: Option<Entity>,
}

impl SoundEmitters {
    pub(crate) fn queue(&mut self, sound: Handle<Sound>, play: Play, command: PlaySound) {
        self.queued.push((sound, play, command));
    }

    /// 这个声音跟着哪个实体（查遮挡时不算它）。
    fn entity_of(&self, voice: Voice) -> Option<Entity> {
        self.emitters
            .iter()
            .filter(|e| e.voice == Some(voice))
            .map(|e| e.following.entity)
            .chain(self.followers.iter().filter(|f| f.voice == voice).map(|f| f.following.entity))
            .next()
            .flatten()
    }
}

/// `scene.audio.emitters` 按 id 增量同步：没了的淡出停掉，新的记下来（位置定了再开始，见 [`place_sounds`]），
/// `src` 这类变了的重新开始，音量、音高变了原地改。
pub(crate) fn sync_sound_emitters(scene: Res<Scene>, mut audio: ResMut<Audio>, mut state: ResMut<SoundEmitters>) {
    if !audio.enabled() {
        return;
    }
    let declared: &[SoundEmitterNode] = scene.audio.as_ref().map_or(&[], |a| &a.emitters);
    if declared.is_empty() && state.emitters.is_empty() {
        return;
    }
    let mut seen = HashSet::new();
    let wanted: Vec<&SoundEmitterNode> = declared
        .iter()
        .filter(|n| {
            let fresh = seen.insert(n.id.as_str());
            if !fresh {
                crate::lenient::warn_once(format!("audio.emitters 里有重复的 id {:?}，只保留第一个", n.id));
            }
            fresh
        })
        .collect();
    state.emitters.retain_mut(|e| {
        let keep = seen.contains(e.node.id.as_str());
        if !keep && let Some(voice) = e.voice.take() {
            audio.stop(voice, e.node.fade);
        }
        keep
    });
    for node in wanted {
        let Some(e) = state.emitters.iter_mut().find(|e| e.node.id == node.id) else {
            state.emitters.push(Emitter { node: node.clone(), voice: None, played: false, following: Following::default() });
            continue;
        };
        if e.node == *node {
            continue;
        }
        if e.node.restarts(node) {
            if let Some(voice) = e.voice.take() {
                audio.stop(voice, node.fade);
            }
            e.played = false;
        } else if let Some(voice) = e.voice {
            if e.node.volume != node.volume {
                audio.set_voice_volume(voice, node.volume, VOLUME_FADE);
            }
            if e.node.pitch != node.pitch {
                audio.set_voice_pitch(voice, node.pitch);
            }
        }
        if e.node.space.follow != node.space.follow {
            e.following = Following::default();
        }
        e.node = node.clone();
    }
}

type Sprites<'w, 's> = Query<'w, 's, (Entity, &'static Declared<SpriteNode>, &'static Transform2d, Option<&'static DrawAt>)>;
type Models<'w, 's> = Query<'w, 's, (Entity, &'static SceneId, &'static Transform3d), With<Declared<ModelNode>>>;

/// 这个 id 的精灵（2D）或模型（3D）现在在哪。上一帧找到过的实体先直接查，不在了（或者换了 id）才整个扫一遍。
fn pose(space: SpaceKind, id: &str, cached: Option<Entity>, sprites: &Sprites, models: &Models) -> Option<Pose> {
    match space {
        SpaceKind::D2 => {
            let hit = cached.and_then(|e| sprites.get(e).ok()).filter(|(_, d, ..)| d.0.id == id);
            let (entity, _, t, drawn) = hit.or_else(|| sprites.iter().find(|(_, d, ..)| d.0.id == id))?;
            // 有刚体时用画的位置（两步之间插值过的）
            let at = drawn.map_or(t.translation, |d| d.translation);
            Some(Pose { entity, at: at.extend(0.0), forward: None })
        }
        SpaceKind::D3 => {
            let hit = cached.and_then(|e| models.get(e).ok()).filter(|(_, model, _)| model.0 == id);
            let (entity, _, t) = hit.or_else(|| models.iter().find(|(_, model, _)| model.0 == id))?;
            Some(Pose { entity, at: t.translation, forward: Some(t.rotation * Vec3::NEG_Z) })
        }
    }
}

fn play_at(play: Play, space: SpaceKind, at: Vec3) -> Play {
    match space {
        SpaceKind::D2 => Play { at: Some(at.truncate()), ..play },
        SpaceKind::D3 => Play { at3: Some(at), ..play },
    }
}

fn move_to(audio: &mut Audio, voice: Voice, space: &SpaceNode, at: Vec3, forward: Option<Vec3>) {
    match space.space {
        SpaceKind::D2 => audio.set_voice_position(voice, at.truncate()),
        SpaceKind::D3 => {
            audio.set_voice_position3(voice, at);
            audio.set_voice_emit(voice, space.emit(forward));
        }
    }
}

/// 物理、跟随、相机都走完以后：摆 3D 的听者（和 `listener.follow`），开始还没开始的声源，让跟着实体的声音跟上，
/// 实体没了的按 `onLost` 处理。
#[allow(clippy::too_many_arguments)]
pub(crate) fn place_sounds(
    scene: Res<Scene>,
    mut audio: ResMut<Audio>,
    mut state: ResMut<SoundEmitters>,
    mut server: ResMut<AssetServer>,
    mut sounds: ResMut<Assets<Sound>>,
    camera: Query<&Transform3d, With<SceneCamera>>,
    sprites: Sprites,
    models: Models,
) {
    if !audio.enabled() {
        state.queued.clear();
        return;
    }
    let state = &mut *state;
    // 听的人放在哪个实体上（没有这个实体时还在相机上）
    let ears = scene.audio.as_ref().and_then(|a| a.listener.as_ref()).and_then(|l| {
        let found = pose(l.space, &l.follow, state.listener, &sprites, &models);
        state.listener = found.map(|p| p.entity);
        Some((l.space, found?.at))
    });
    // 3D 的听者：在相机上，左右按相机的朝向；`listener.follow` 只改位置（左右还是和画面一致）
    if let Ok(camera) = camera.single() {
        let at = ears.filter(|(space, _)| *space == SpaceKind::D3).map_or(camera.translation, |(_, at)| at);
        let right = camera.rotation * Vec3::X;
        if audio.listener3() != (at, right) {
            audio.set_listener3(at, right);
        }
    }
    // 2D 的听者 `audio::listen` 已经放在 2D 相机上了
    if let Some((SpaceKind::D2, at)) = ears {
        audio.set_listener(at.truncate());
    }

    for e in &mut state.emitters {
        let node = &e.node;
        let offset = Vec3::new(node.x, node.y, node.space.z.unwrap_or(0.0));
        match e.following.place(&node.space, offset, &sprites, &models) {
            Place::At(at, forward) => match e.voice {
                Some(voice) => move_to(&mut audio, voice, &node.space, at, forward),
                None if !e.played => {
                    let sound = server.load(&mut sounds, &node.src);
                    let play = Play { volume: node.volume, pitch: node.pitch, looped: node.looped, fade_in: node.fade, bus: node.bus.into(), emit: node.space.emit(forward), ..Play::default() };
                    e.voice = Some(audio.play(sound, play_at(play, node.space.space, at)));
                                e.played = !node.looped;
                }
                None => {}
            },
            Place::JustLost => {
                if node.space.on_lost == OnLost::Stop && let Some(voice) = e.voice.take() {
                    audio.stop(voice, node.fade);
                }
            }
            Place::Waiting | Place::Lost => {}
        }
    }

    for (sound, play, command) in state.queued.drain(..) {
        let offset = Vec3::new(command.x.unwrap_or(0.0), command.y.unwrap_or(0.0), command.space.z.unwrap_or(0.0));
        let mut following = Following::default();
        match following.place(&command.space, offset, &sprites, &models) {
            Place::At(at, forward) => {
                let play = Play { emit: command.space.emit(forward), ..play };
                let voice = audio.play(sound, play_at(play, command.space.space, at));
                state.followers.push(Follower { voice, space: command.space, offset, following });
            }
            _ => {
                crate::lenient::warn_once(format!("sound 要跟着的 {:?} 不在场景里，不播了", command.space.follow));
            }
        }
    }
    state.followers.retain_mut(|f| {
        if !audio.is_playing(f.voice) {
            return false;
        }
        match f.following.place(&f.space, f.offset, &sprites, &models) {
            Place::At(at, forward) => {
                move_to(&mut audio, f.voice, &f.space, at, forward);
                true
            }
            // 实体没了：停下，或者留在最后的位置播完
            _ => {
                if f.space.on_lost == OnLost::Stop {
                    audio.stop(f.voice, LOST_FADE);
                }
                false
            }
        }
    });
}

/// 从 `from` 到 `to` 之间有没有东西挡着。`cast(起点, 方向, 最远, 不算谁)` 返回第一个碰到的实体和距离；
/// `solid(实体)` 说它算不算遮挡（固定的才算）。不算的穿过去接着查，最多穿 [`MAX_HOPS`] 个；碰到声源自己（`own`）就是没挡。
pub(crate) fn blocked(mut cast: impl FnMut(Vec3, Vec3, f32, Option<Entity>) -> Option<(Entity, f32)>, solid: impl Fn(Entity) -> bool, from: Vec3, to: Vec3, own: Option<Entity>) -> bool {
    let Some(dir) = (to - from).try_normalize() else { return false };
    let mut left = from.distance(to) - RAY_MARGIN;
    let (mut origin, mut skip) = (from, own);
    for _ in 0..=MAX_HOPS {
        if !(left > 0.0) {
            return false;
        }
        let Some((entity, distance)) = cast(origin, dir, left, skip) else { return false };
        if Some(entity) == own {
            return false;
        }
        if solid(entity) {
            return true;
        }
        // 动的东西不算：从碰到的地方接着往前
        origin += dir * distance;
        left -= distance;
        skip = Some(entity);
    }
    false
}

/// 遮挡：给音频这一帧要查的声音打射线（它自己节流），结果告诉它。固定刚体、没有刚体的碰撞体算挡住，
/// 动态、kinematic 的刚体和 sensor 不算。
pub(crate) fn occlude_sounds(
    mut audio: ResMut<Audio>,
    state: Res<SoundEmitters>,
    physics: Res<Physics>,
    bodies: Query<&nge_physics2d::RigidBody>,
    #[cfg(feature = "physics3d")] physics3d: Option<Res<nge_physics3d::Physics3d>>,
    #[cfg(feature = "physics3d")] bodies3d: Query<&nge_physics3d::RigidBody>,
) {
    for probe in audio.occlusion_probes() {
        let own = state.entity_of(probe.voice);
        let hidden = if probe.three_d {
            #[cfg(feature = "physics3d")]
            {
                physics3d.as_ref().is_some_and(|physics| {
                    let cast = |origin: Vec3, dir: Vec3, max: f32, skip: Option<Entity>| {
                        let filter = skip.map_or_else(nge_physics3d::QueryFilter::default, |e| nge_physics3d::QueryFilter::default().exclude(e));
                        physics.raycast(origin, dir, max, filter).map(|hit| (hit.entity, hit.distance))
                    };
                    blocked(cast, |e| !matches!(bodies3d.get(e), Ok(nge_physics3d::RigidBody::Dynamic | nge_physics3d::RigidBody::Kinematic)), probe.from, probe.to, own)
                })
            }
            #[cfg(not(feature = "physics3d"))]
            false
        } else {
            let cast = |origin: Vec3, dir: Vec3, max: f32, skip: Option<Entity>| {
                let filter = skip.map_or_else(nge_physics2d::QueryFilter::default, |e| nge_physics2d::QueryFilter::default().exclude(e));
                physics.raycast(origin.truncate(), dir.truncate(), max, filter).map(|hit| (hit.entity, hit.distance))
            };
            blocked(cast, |e| !matches!(bodies.get(e), Ok(nge_physics2d::RigidBody::Dynamic | nge_physics2d::RigidBody::Kinematic)), probe.from, probe.to, own)
        };
        audio.set_occluded(probe.voice, hidden);
    }
}


#[cfg(test)]
impl SoundEmitters {
    /// 这个 id 的声源现在的声音。
    pub(crate) fn voice(&self, id: &str) -> Option<Voice> {
        self.emitters.iter().find(|e| e.node.id == id).and_then(|e| e.voice)
    }

    pub(crate) fn following(&self) -> usize {
        self.followers.len()
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;
    use nge_audio::{Bus, EffectSpec};
    use serde_json::{Value, json};

    use super::*;
    use crate::testing::{BEEP, app, frame};

    const CAMERA: &str = r#"{ "x": 0, "y": 0, "z": 100, "target": [0, 0, 0] }"#;

    fn camera() -> Value {
        serde_json::from_str(CAMERA).unwrap()
    }

    fn settle(app: &mut nge_ecs::App) {
        AssetServer::apply(app.world_mut(), true);
        frame(app, None);
    }

    fn audio(app: &nge_ecs::App) -> &Audio {
        app.world().resource::<Audio>()
    }

    fn voice(app: &nge_ecs::App, id: &str) -> Option<Voice> {
        app.world().resource::<SoundEmitters>().voice(id).filter(|v| audio(app).is_playing(*v))
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// 线性衰减、100 到 300 的 3D 声源。
    fn source(id: &str, at: [f32; 3], more: Value) -> Value {
        let mut node = json!({ "id": id, "src": BEEP, "space": "3d", "x": at[0], "y": at[1], "z": at[2], "rolloff": "linear", "min": 100, "max": 300 });
        node.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
        node
    }

    #[test]
    fn emitters_start_change_and_stop_with_the_scene() {
        let mut app = app();
        let scene = |emitters: Value| json!({ "camera3d": camera(), "audio": { "emitters": emitters } });
        frame(&mut app, Some(scene(json!([source("fire", [-200.0, 0.0, 100.0], json!({ "volume": 0.8 })), source("fire", [0.0, 0.0, 0.0], json!({}))]))));
        settle(&mut app);
        assert_eq!(audio(&app).voices(), 1, "有就响；重复的 id 只留第一个");
        let fire = voice(&app, "fire").unwrap();
        let (v, p) = audio(&app).mix(fire).unwrap();
        assert!(near(v, 0.4) && near(p, -1.0), "听的人在 3D 相机上：左边 200，线性衰减一半 {v} {p}");
        assert_eq!(audio(&app).bus_sounds(Bus::Sfx), 1);

        // 位置、音量、音高、衰减变了：原地改，还是那个声音
        frame(&mut app, Some(scene(json!([source("fire", [150.0, 0.0, 100.0], json!({ "volume": 0.4, "pitch": 2, "max": 200 }))]))));
        frame(&mut app, None);
        assert_eq!(voice(&app, "fire"), Some(fire));
        let (v, p) = audio(&app).mix(fire).unwrap();
        assert!(near(v, 0.2) && near(p, 1.0), "右边 150，最远改成 200 {v} {p}");
        assert_eq!(audio(&app).rate(fire), Some(2.0));

        // 换了 src（或者循环、分组、空间、遮挡）：重新开始
        for change in [json!({ "bus": "ui" }), json!({ "loop": false }), json!({ "occlusion": true }), json!({ "space": "2d" })] {
            let before = voice(&app, "fire").unwrap();
            frame(&mut app, Some(scene(json!([source("fire", [150.0, 0.0, 100.0], change.clone())]))));
            frame(&mut app, None);
            let after = voice(&app, "fire").unwrap();
            assert!(after != before && !audio(&app).is_playing(before), "{change} 变了重新开始");
            if change.get("bus").is_some() {
                assert_eq!(audio(&app).bus_sounds(Bus::Ui), 1, "进声明的分组");
            }
            assert_eq!(audio(&app).voices(), 1);
            frame(&mut app, Some(scene(json!([source("fire", [150.0, 0.0, 100.0], json!({}))]))));
            frame(&mut app, None);
        }

        // 多一个、少一个：只动变了的
        let kept = voice(&app, "fire").unwrap();
        frame(&mut app, Some(scene(json!([source("fire", [150.0, 0.0, 100.0], json!({})), source("fall", [0.0, 0.0, 0.0], json!({}))]))));
        frame(&mut app, None);
        assert_eq!((audio(&app).voices(), voice(&app, "fire")), (2, Some(kept)));
        frame(&mut app, Some(scene(json!([source("fall", [0.0, 0.0, 0.0], json!({}))]))));
        assert!(!audio(&app).is_playing(kept), "没了就停");
        assert_eq!(audio(&app).voices(), 1);
        frame(&mut app, Some(json!({})));
        assert_eq!(audio(&app).voices(), 0, "去掉 audio 全停");
        // 写错的那一个跳过，别的照常
        frame(&mut app, Some(scene(json!([{ "id": "bad" }, source("ok", [0.0, 0.0, 0.0], json!({ "rolloff": "exponential", "doppler": 3, "cone": { "inner": 60, "outer": 120, "outerGain": 0.2, "direction": [1, 0, 0] } }))]))));
        frame(&mut app, None);
        assert_eq!(audio(&app).voices(), 1);
        let emit = audio(&app).voice_emit(voice(&app, "ok").unwrap()).unwrap();
        assert_eq!((emit.rolloff, emit.doppler, emit.forward), (Rolloff::Exponential, 3.0, Vec3::X));
        let cone = emit.cone.unwrap();
        assert!(near(cone.inner, 60f32.to_radians()) && near(cone.outer, 120f32.to_radians()) && cone.outer_gain == 0.2);
        // 声源的声音文件算进场景要预加载的资源
        let scene: Scene = serde_json::from_value(scene(json!([source("a", [0.0; 3], json!({}))]))).unwrap();
        assert_eq!(crate::preload::uris_of(&scene), vec![BEEP.to_owned()]);
    }

    #[test]
    fn emitters_follow_sprites_and_models_and_handle_losing_them() {
        let mut app = app();
        let scene = |sprites: Value, models: Value, emitters: Value| json!({ "camera3d": camera(), "physics": { "gravity": [0, 0] }, "sprites": sprites, "models": models, "audio": { "emitters": emitters } });
        let ghost = json!([{ "id": "ghost", "x": 100, "y": 50, "w": 10 }]);
        let drone = |x: f32, yaw: f32| json!([{ "id": "drone", "model": "asset:/@g/a.glb", "x": x, "y": 10, "z": 20, "rotation": [0, yaw, 0] }]);
        let emitters = |on_lost: &str| {
            json!([
                { "id": "moan", "src": BEEP, "follow": "ghost", "x": 5, "onLost": on_lost },
                source("hum", [0.0, 30.0, 0.0], json!({ "follow": "drone", "onLost": on_lost, "cone": { "inner": 90, "outer": 180 } })),
                { "id": "once", "src": BEEP, "follow": "ghost", "loop": false },
            ])
        };
        // 跟着的实体还没有：等着，不开始
        frame(&mut app, Some(scene(json!([]), json!([]), emitters("stop"))));
        settle(&mut app);
        assert_eq!(audio(&app).voices(), 0);
        frame(&mut app, Some(scene(ghost.clone(), drone(-100.0, 0.0), emitters("stop"))));
        settle(&mut app);
        let (moan, hum, once) = (voice(&app, "moan").unwrap(), voice(&app, "hum").unwrap(), voice(&app, "once").unwrap());
        assert_eq!(audio(&app).voice_position(moan), Some(Vec3::new(105.0, 50.0, 0.0)), "精灵的位置加偏移");
        assert_eq!(audio(&app).voice_position(hum), Some(Vec3::new(-100.0, 40.0, 20.0)), "模型的位置加偏移");
        assert!(audio(&app).voice_emit(hum).unwrap().forward.distance(Vec3::NEG_Z) < 1e-5, "锥形朝模型的前面");

        // 实体挪了、转了：声音跟着
        frame(&mut app, Some(scene(json!([{ "id": "ghost", "x": -40, "y": 0, "w": 10 }]), drone(60.0, 90.0), emitters("stop"))));
        assert_eq!(audio(&app).voice_position(moan), Some(Vec3::new(-35.0, 0.0, 0.0)));
        assert_eq!(audio(&app).voice_position(hum), Some(Vec3::new(60.0, 40.0, 20.0)));
        assert!(audio(&app).voice_emit(hum).unwrap().forward.distance(Vec3::NEG_X) < 1e-5, "转了 90°：朝 -x");

        // 实体没了：默认停掉；回来以后循环的重新开始，播过一次的不再播
        frame(&mut app, Some(scene(json!([]), json!([]), emitters("stop"))));
        assert_eq!(audio(&app).voices(), 0, "实体没了就停");
        frame(&mut app, None);
        frame(&mut app, Some(scene(ghost.clone(), drone(0.0, 0.0), emitters("stop"))));
        frame(&mut app, None);
        let again = voice(&app, "moan").unwrap();
        assert!(again != moan && voice(&app, "hum").is_some(), "回来了重新开始");
        assert!(!audio(&app).is_playing(once) && voice(&app, "once").is_none(), "不循环的只播一次");
        assert_eq!(audio(&app).voices(), 2);

        // onLost: "finish"：留在最后的位置接着播；回来了接着跟
        frame(&mut app, Some(scene(ghost.clone(), drone(0.0, 0.0), emitters("finish"))));
        frame(&mut app, Some(scene(json!([]), json!([]), emitters("finish"))));
        frame(&mut app, None);
        assert_eq!((voice(&app, "moan"), audio(&app).voices()), (Some(again), 2), "接着播");
        assert_eq!(audio(&app).voice_position(again), Some(Vec3::new(105.0, 50.0, 0.0)), "留在最后的位置");
        frame(&mut app, Some(scene(json!([{ "id": "ghost", "x": 0, "y": 0, "w": 10 }]), json!([]), emitters("finish"))));
        assert_eq!(audio(&app).voice_position(again), Some(Vec3::new(5.0, 0.0, 0.0)), "回来了接着跟");

        // 换了跟着的对象：从头算（新的对象还没出现过，不算丢）
        let other = json!([{ "id": "moan", "src": BEEP, "follow": "nobody", "x": 5, "onLost": "stop" }]);
        frame(&mut app, Some(scene(ghost.clone(), json!([]), other)));
        assert_eq!(voice(&app, "moan"), Some(again), "等新的对象出现，原来的声音留着");
    }

    #[test]
    fn sound_command_in_3d_and_following_a_node() {
        let mut app = app();
        let scene = |sprites: Value| json!({ "camera3d": camera(), "sprites": sprites, "models": [{ "id": "drone", "model": "asset:/@g/a.glb", "x": -200, "z": 100 }], "audio": { "preload": [BEEP] } });
        let ghost = json!([{ "id": "ghost", "x": 100, "y": 50, "w": 10 }, { "id": "bat", "x": 0, "y": 0, "w": 10 }]);
        frame(&mut app, Some(scene(ghost.clone())));
        settle(&mut app);
        let mixes = |app: &nge_ecs::App| {
            let audio = audio(app);
            let mut all: Vec<(f32, f32)> = audio.playing().filter_map(|v| audio.mix(v)).collect();
            all.sort_by(|a, b| a.0.total_cmp(&b.0));
            all
        };
        app.command("sound", &json!({ "src": BEEP, "space": "3d", "x": 250, "z": 100, "rolloff": "linear", "min": 100, "max": 300, "volume": 0.8 }));
        app.command("sound", &json!({ "src": BEEP, "space": "3d", "follow": "drone", "rolloff": "linear", "min": 100, "max": 300 }));
        app.command("sound", &json!({ "src": BEEP, "space": "3d", "follow": "nobody" }));
        app.command("sound", &json!({ "src": BEEP, "follow": "ghost", "y": -50 }));
        app.command("sound", &json!({ "src": BEEP, "follow": "bat", "onLost": "finish" }));
        frame(&mut app, None);
        let all = mixes(&app);
        assert_eq!(all.len(), 4, "跟着的实体不在场景里的不播 {all:?}");
        assert!(near(all[0].0, 0.2) && near(all[0].1, 1.0), "3D 的位置：右边 250 {all:?}");
        assert!(near(all[1].0, 0.5) && near(all[1].1, -1.0), "跟着模型：左边 200 {all:?}");
        assert!(near(all[2].0, 0.9) && near(all[2].1, 0.2), "跟着精灵加偏移：2D 的右边 100 {all:?}");
        assert_eq!(app.world().resource::<SoundEmitters>().following(), 3);
        // 精灵没了：默认的停掉，finish 的接着播、不再跟
        frame(&mut app, Some(scene(json!([]))));
        assert_eq!(mixes(&app).len(), 3, "跟着的精灵没了：停掉一个，finish 的留着");
        assert_eq!(app.world().resource::<SoundEmitters>().following(), 1);
    }

    #[test]
    fn the_listener_is_the_camera_or_a_node() {
        let mut app = app();
        let scene = |listener: Value| {
            json!({ "camera": { "x": 30, "y": 40 }, "camera3d": { "x": 0, "y": 0, "z": 100, "target": [-100, 0, 100] },
                "sprites": [{ "id": "hero", "x": 7, "y": 8, "w": 10 }], "models": [{ "id": "pilot", "model": "asset:/@g/a.glb", "x": 1, "y": 2, "z": 3 }],
                "audio": { "listener": listener } })
        };
        frame(&mut app, Some(scene(Value::Null)));
        frame(&mut app, None);
        let (at, right) = audio(&app).listener3();
        assert!(at.distance(Vec3::new(0.0, 0.0, 100.0)) < 1e-4 && right.distance(Vec3::NEG_Z) < 1e-4, "在 3D 相机上，朝 -x 看时右手是 -z：{at} {right}");
        assert_eq!(audio(&app).listener(), Vec2::new(30.0, 40.0));
        frame(&mut app, Some(scene(json!({ "follow": "pilot", "space": "3d" }))));
        let (at, right) = audio(&app).listener3();
        assert!(at.distance(Vec3::new(1.0, 2.0, 3.0)) < 1e-4 && right.distance(Vec3::NEG_Z) < 1e-4, "位置在模型上，左右还按相机：{at} {right}");
        assert_eq!(audio(&app).listener(), Vec2::new(30.0, 40.0), "2D 的不受影响");
        frame(&mut app, Some(scene(json!({ "follow": "hero" }))));
        assert_eq!(audio(&app).listener(), Vec2::new(7.0, 8.0), "2D 的在精灵上");
        assert!(audio(&app).listener3().0.distance(Vec3::new(0.0, 0.0, 100.0)) < 1e-4, "3D 的回到相机上");
        frame(&mut app, Some(scene(json!({ "follow": "nobody", "space": "3d" }))));
        frame(&mut app, None);
        assert!(audio(&app).listener3().0.distance(Vec3::new(0.0, 0.0, 100.0)) < 1e-4, "没有这个模型：还在相机上");
        assert_eq!(audio(&app).listener(), Vec2::new(30.0, 40.0));
    }

    #[test]
    fn rays_stop_at_walls_but_pass_moving_things_and_the_source_itself() {
        let (wall, crate_, me) = (Entity::from_raw_u32(1).unwrap(), Entity::from_raw_u32(2).unwrap(), Entity::from_raw_u32(3).unwrap());
        let (from, to) = (Vec3::ZERO, Vec3::new(0.0, 0.0, -100.0));
        // 沿 -z 排着的东西：(实体, 离起点多远)
        let world = |things: Vec<(Entity, f32)>| {
            move |origin: Vec3, dir: Vec3, max: f32, skip: Option<Entity>| {
                assert!(dir.distance(Vec3::NEG_Z) < 1e-6);
                things.iter().map(|(e, z)| (*e, *z + origin.z)).find(|(e, d)| Some(*e) != skip && *d >= 0.0 && *d <= max)
            }
        };
        let solid = |e: Entity| e == wall || e == me;
        assert!(!blocked(world(vec![]), solid, from, to, None));
        assert!(blocked(world(vec![(wall, 40.0)]), solid, from, to, None));
        assert!(!blocked(world(vec![(wall, 140.0)]), solid, from, to, None), "墙在声源后面");
        assert!(!blocked(world(vec![(wall, 99.8)]), solid, from, to, None), "贴着声源的不算（声源自己的碰撞体）");
        assert!(!blocked(world(vec![(crate_, 40.0)]), solid, from, to, None), "动的东西不挡");
        assert!(blocked(world(vec![(crate_, 20.0), (wall, 60.0)]), solid, from, to, None), "穿过动的东西接着查");
        assert!(!blocked(world(vec![(me, 50.0)]), solid, from, to, Some(me)), "跟着的实体自己不算");
        assert!(!blocked(world(vec![(me, 50.0), (wall, 30.0)]), |_| false, from, to, None));
        assert!(!blocked(world(vec![(wall, 0.0)]), solid, from, from, None), "重合");
        // 一路都是不算遮挡的东西：穿够了就不查了
        let mut casts = 0;
        let endless = |_: Vec3, _: Vec3, _: f32, _: Option<Entity>| {
            casts += 1;
            Some((crate_, 1.0))
        };
        assert!(!blocked(endless, solid, from, to, None));
        assert_eq!(casts, MAX_HOPS + 1);
    }

    #[cfg(feature = "physics3d")]
    #[test]
    fn walls_muffle_emitters_in_3d() {
        let mut app = app();
        let wall = |kind: &str| json!({ "id": "wall", "model": "asset:/@g/a.glb", "z": 0, "body": { "kind": kind, "size": [200, 200, 10] } });
        let scene = |models: Value| {
            json!({ "camera3d": camera(), "physics3d": { "gravity": [0, 0, 0] }, "models": models,
                "audio": { "emitters": [source("radio", [0.0, 0.0, -100.0], json!({ "occlusion": true })), source("open", [0.0, 0.0, 150.0], json!({ "occlusion": true })),
                    source("plain", [0.0, 0.0, -100.0], json!({}))] } })
        };
        frame(&mut app, Some(scene(json!([wall("fixed")]))));
        settle(&mut app);
        frame(&mut app, None);
        let (radio, open, plain) = (voice(&app, "radio").unwrap(), voice(&app, "open").unwrap(), voice(&app, "plain").unwrap());
        assert_eq!(audio(&app).occlusion(radio), Some(1.0), "墙后面的被挡住");
        assert_eq!(audio(&app).occlusion(open), Some(0.0), "相机这一侧的没挡");
        assert_eq!(audio(&app).occlusion(plain), Some(0.0), "没开遮挡的不查");
        assert!(near(audio(&app).mix(radio).unwrap().0, audio(&app).mix(plain).unwrap().0 * 0.35), "音量压下去");
        assert!(audio(&app).voice_cutoff(radio).unwrap() < 900.0, "加了低通");
        // 墙换成会动的：不算遮挡
        frame(&mut app, Some(scene(json!([wall("dynamic")]))));
        for _ in 0..3 {
            frame(&mut app, None);
        }
        assert_eq!(audio(&app).occlusion(radio), Some(0.0), "动态刚体不挡");
        frame(&mut app, Some(scene(json!([wall("fixed")]))));
        for _ in 0..3 {
            frame(&mut app, None);
        }
        assert_eq!(audio(&app).occlusion(radio), Some(1.0));
        // 墙没了：放开
        frame(&mut app, Some(scene(json!([]))));
        for _ in 0..3 {
            frame(&mut app, None);
        }
        assert_eq!(audio(&app).occlusion(radio), Some(0.0));
        assert_eq!(audio(&app).voice_cutoff(radio), Some(20000.0));
    }

    #[cfg(feature = "physics3d")]
    #[test]
    fn a_followed_body_does_not_occlude_itself() {
        let mut app = app();
        let scene = json!({ "camera3d": camera(), "physics3d": { "gravity": [0, 0, 0] },
            "models": [{ "id": "speaker", "model": "asset:/@g/a.glb", "z": -100, "body": { "kind": "fixed", "size": [40, 40, 40] } }],
            "audio": { "emitters": [source("song", [0.0, 0.0, 0.0], json!({ "follow": "speaker", "occlusion": true })), source("inside", [0.0, 0.0, -100.0], json!({ "occlusion": true }))] } });
        frame(&mut app, Some(scene));
        settle(&mut app);
        frame(&mut app, None);
        assert_eq!(audio(&app).occlusion(voice(&app, "song").unwrap()), Some(0.0), "跟着的刚体自己不挡");
        assert_eq!(audio(&app).occlusion(voice(&app, "inside").unwrap()), Some(1.0), "别人的盒子里面的被挡");
    }

    #[test]
    fn walls_muffle_emitters_in_2d() {
        let mut app = app();
        let scene = |sprites: Value| {
            json!({ "physics": { "gravity": [0, 0] }, "sprites": sprites,
                "audio": { "emitters": [{ "id": "drip", "src": BEEP, "x": 300, "y": 0, "occlusion": true }, { "id": "bird", "src": BEEP, "x": -300, "y": 0, "occlusion": true }] } })
        };
        let wall = |kind: &str, sensor: bool| json!([{ "id": "wall", "x": 150, "y": 0, "w": 20, "h": 200, "body": { "kind": kind, "sensor": sensor } }]);
        frame(&mut app, Some(scene(wall("fixed", false))));
        settle(&mut app);
        frame(&mut app, None);
        let (drip, bird) = (voice(&app, "drip").unwrap(), voice(&app, "bird").unwrap());
        assert_eq!((audio(&app).occlusion(drip), audio(&app).occlusion(bird)), (Some(1.0), Some(0.0)), "2D 的从 2D 相机到声源");
        for (kind, sensor) in [("dynamic", false), ("fixed", true)] {
            frame(&mut app, Some(scene(wall(kind, sensor))));
            for _ in 0..3 {
                frame(&mut app, None);
            }
            assert_eq!(audio(&app).occlusion(drip), Some(0.0), "{kind} sensor={sensor} 不挡");
            frame(&mut app, Some(scene(wall("fixed", false))));
            for _ in 0..3 {
                frame(&mut app, None);
            }
            assert_eq!(audio(&app).occlusion(drip), Some(1.0));
        }
    }

    #[test]
    fn bus_effect_chains_follow_the_scene() {
        use EffectSpec::*;
        let mut app = app();
        frame(&mut app, Some(json!({ "audio": { "speedOfSound": 5000, "sendEffects": [{ "type": "delay", "time": 0.4 }],
            "buses": { "sfx": { "effects": [{ "type": "lowpass", "cutoff": 500 }, { "type": "bogus" }, { "type": "reverb", "room": 0.9, "mix": 2 }], "send": 0.4, "fade": 1.5 },
                       "music": { "effects": [{ "type": "compressor", "ratio": 8 }, { "type": "limiter" }, { "type": "highpass", "cutoff": 80, "q": 2 }] } } } })));
        let a = audio(&app);
        assert_eq!(a.effects(Bus::Sfx), &[Lowpass { cutoff: 500.0, q: std::f32::consts::FRAC_1_SQRT_2 }, Reverb { room: 0.9, damping: 0.5, mix: 1.0 }], "写错的那个跳过，没写的用默认，超出的截回去");
        assert_eq!(
            a.effects(Bus::Music),
            &[Compressor { threshold: -18.0, ratio: 8.0, attack: 0.01, release: 0.15, makeup: 0.0 }, Limiter { ceiling: -1.0, release: 0.1 }, Highpass { cutoff: 80.0, q: 2.0 }]
        );
        assert!(a.effects(Bus::Ui).is_empty());
        assert_eq!(Bus::ALL.map(|b| a.send(b)), [0.0, 0.4, 0.0]);
        assert_eq!(a.send_effects(), &[Delay { time: 0.4, feedback: 0.35, mix: 0.3 }]);
        assert_eq!(a.speed_of_sound(), 5000.0);
        // 去掉：回到默认
        frame(&mut app, Some(json!({ "audio": {} })));
        let a = audio(&app);
        assert!(Bus::ALL.iter().all(|b| a.effects(*b).is_empty() && a.send(*b) == 0.0));
        assert_eq!(a.send_effects(), Audio::default_send_effects().as_slice());
        assert_eq!(a.speed_of_sound(), 34300.0);
    }
}
