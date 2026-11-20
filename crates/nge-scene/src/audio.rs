//! `scene.audio`（背景音乐、总音量、预加载、混音分组和效果器链）、`sound` command，听的人跟着相机（见 docs/audio-more.md、docs/audio-3d.md）。
//! 声明式的声源、跟着实体走、遮挡在 `audio3d.rs`。

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_assets::{AssetServer, Assets};
use nge_audio::{Audio, Bus, DEFAULT_RANGE, EffectSpec, Play, Sound, Voice};
use nge_render2d::Camera2d;
use nge_core::CommandKey;
use serde::Deserialize;

use crate::SceneCamera;
use crate::audio3d::{SoundEmitterNode, SoundEmitters, SpaceKind, SpaceNode};
use crate::protocol::Scene;

pub const SOUND: CommandKey<PlaySound> = CommandKey::new("sound");

/// 换音乐、停音乐时淡入淡出的秒数。
const MUSIC_FADE: f32 = 0.5;
/// 只改音乐音量时过渡的秒数。
const VOLUME_FADE: f32 = 0.2;
/// 分组的设置变了时过渡的秒数。
const BUS_FADE: f32 = 0.3;

/// 效果器链里的一个效果器（见 docs/audio-3d.md 的表）。没写的参数用默认值，超出范围的截回去。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum EffectNode {
    Lowpass { cutoff: f32, q: Option<f32> },
    Highpass { cutoff: f32, q: Option<f32> },
    Reverb { room: Option<f32>, damping: Option<f32>, mix: Option<f32> },
    Delay { time: Option<f32>, feedback: Option<f32>, mix: Option<f32> },
    Compressor { threshold: Option<f32>, ratio: Option<f32>, attack: Option<f32>, release: Option<f32>, makeup: Option<f32> },
    Limiter { ceiling: Option<f32>, release: Option<f32> },
}

impl From<EffectNode> for EffectSpec {
    fn from(node: EffectNode) -> Self {
        // 没写的给 NaN：`EffectSpec::sanitize` 把不是数的换成默认值
        let or = |v: Option<f32>| v.unwrap_or(f32::NAN);
        match node {
            EffectNode::Lowpass { cutoff, q } => EffectSpec::Lowpass { cutoff, q: or(q) },
            EffectNode::Highpass { cutoff, q } => EffectSpec::Highpass { cutoff, q: or(q) },
            EffectNode::Reverb { room, damping, mix } => EffectSpec::Reverb { room: or(room), damping: or(damping), mix: or(mix) },
            EffectNode::Delay { time, feedback, mix } => EffectSpec::Delay { time: or(time), feedback: or(feedback), mix: or(mix) },
            EffectNode::Compressor { threshold, ratio, attack, release, makeup } => {
                EffectSpec::Compressor { threshold: or(threshold), ratio: or(ratio), attack: or(attack), release: or(release), makeup: or(makeup) }
            }
            EffectNode::Limiter { ceiling, release } => EffectSpec::Limiter { ceiling: or(ceiling), release: or(release) },
        }
    }
}

fn specs(effects: &[EffectNode]) -> Vec<EffectSpec> {
    effects.iter().map(|e| EffectSpec::from(*e)).collect()
}

/// 一个混音分组的设置。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct BusNode {
    pub volume: f32,
    /// 低通的截止频率（赫兹），不写是关。
    pub lowpass: Option<f32>,
    /// 混响的湿声比例，0 到 1。
    pub reverb: f32,
    /// 效果器链，接在上面的低通、混响后面，按顺序处理。
    #[serde(deserialize_with = "crate::lenient::audio_effects")]
    pub effects: Vec<EffectNode>,
    /// 往混响总线送多少，0 到 1。
    pub send: f32,
    /// 这个分组的设置变了时过渡多久（秒），默认 0.3。
    pub fade: Option<f32>,
}

impl Default for BusNode {
    fn default() -> Self {
        Self { volume: 1.0, lowpass: None, reverb: 0.0, effects: Vec::new(), send: 0.0, fade: None }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct BusesNode {
    pub music: BusNode,
    pub sfx: BusNode,
    pub ui: BusNode,
}

impl BusesNode {
    fn get(&self, bus: Bus) -> &BusNode {
        match bus {
            Bus::Music => &self.music,
            Bus::Sfx => &self.sfx,
            Bus::Ui => &self.ui,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BusName {
    Music,
    #[default]
    Sfx,
    Ui,
}

impl From<BusName> for Bus {
    fn from(b: BusName) -> Self {
        match b {
            BusName::Music => Bus::Music,
            BusName::Sfx => Bus::Sfx,
            BusName::Ui => Bus::Ui,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioNode {
    /// 循环播放的背景音乐（`asset:/` 路径）。
    pub music: Option<String>,
    pub music_volume: f32,
    pub volume: f32,
    pub muted: bool,
    /// 提前加载的声音，第一次播放时不用等。
    pub preload: Vec<String>,
    /// 混音分组：音量、低通、混响。
    pub buses: BusesNode,
    /// 按位置的声音多远就听不见，默认 1000。
    pub range: Option<f32>,
    /// 声明式的声源：场景里有就响，没了就停（见 docs/audio-3d.md）。
    #[serde(deserialize_with = "crate::lenient::audio_emitters")]
    pub emitters: Vec<SoundEmitterNode>,
    /// 听的人放在哪个精灵 / 模型上；不写时在相机上。
    pub listener: Option<crate::audio3d::ListenerNode>,
    /// 多普勒用的声速（point / 秒），默认 34300。
    pub speed_of_sound: Option<f32>,
    /// 混响总线上的效果器链；不写是一个全湿的混响。
    pub send_effects: Option<Vec<EffectNode>>,
}

impl Default for AudioNode {
    fn default() -> Self {
        Self { music: None, music_volume: 1.0, volume: 1.0, muted: false, preload: Vec::new(), buses: BusesNode::default(), range: None, emitters: Vec::new(), listener: None, speed_of_sound: None, send_effects: None }
    }
}

/// 播一次音效。
#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct PlaySound {
    pub src: String,
    pub volume: f32,
    pub pitch: f32,
    pub pan: f32,
    pub bus: BusName,
    /// 在世界里的位置：两个都写了才按位置播（`space: "3d"` 或者写了 `follow` 时没写的当 0）。
    pub x: Option<f32>,
    pub y: Option<f32>,
    /// 3D 的位置、跟着谁、衰减、多普勒、遮挡（见 docs/audio-3d.md）。
    #[serde(flatten)]
    pub space: SpaceNode,
}

impl Default for PlaySound {
    fn default() -> Self {
        Self { src: String::new(), volume: 1.0, pitch: 1.0, pan: 0.0, bus: BusName::Sfx, x: None, y: None, space: SpaceNode::default() }
    }
}

/// 正在放的背景音乐。
#[derive(Default)]
pub(crate) struct Music {
    declared: AudioNode,
    voice: Option<Voice>,
}

/// `scene.audio` 变了才动：换音乐时旧的淡出、新的淡入，只改音量时平滑过渡。
pub(crate) fn sync_audio(
    scene: Res<Scene>,
    mut audio: ResMut<Audio>,
    mut server: ResMut<AssetServer>,
    mut sounds: ResMut<Assets<Sound>>,
    mut music: Local<Music>,
) {
    // 没有音频设备时连加载都不做，免得白白解码
    if !audio.enabled() {
        return;
    }
    // 声源每帧都可能变（跟着算出来的位置写的），由 `audio3d` 自己同步：这里不看它
    let node = AudioNode { emitters: Vec::new(), ..scene.audio.clone().unwrap_or_default() };
    let before = &music.declared;
    if *before == node {
        return;
    }
    for uri in &node.preload {
        server.load(&mut sounds, uri);
    }
    if (before.volume, before.muted) != (node.volume, node.muted) {
        audio.set_volume(node.volume);
        audio.set_muted(node.muted);
    }
    for bus in Bus::ALL {
        let (old, new) = (before.buses.get(bus), node.buses.get(bus));
        let fade = new.fade.unwrap_or(BUS_FADE);
        if old.volume != new.volume {
            audio.set_bus_volume(bus, new.volume, fade);
        }
        if old.lowpass != new.lowpass {
            audio.set_lowpass(bus, new.lowpass, fade);
        }
        if old.reverb != new.reverb {
            audio.set_reverb(bus, new.reverb, fade);
        }
        if old.effects != new.effects {
            audio.set_effects(bus, &specs(&new.effects), fade);
        }
        if old.send != new.send {
            audio.set_send(bus, new.send, fade);
        }
    }
    if before.send_effects != node.send_effects {
        match &node.send_effects {
            Some(effects) => audio.set_send_effects(&specs(effects), BUS_FADE),
            // 去掉了：回到默认的那个混响
            None => audio.set_send_effects(Audio::default_send_effects().as_slice(), BUS_FADE),
        }
    }
    if before.speed_of_sound != node.speed_of_sound {
        // 不写（或者写得不对）时回到默认
        audio.set_speed_of_sound(node.speed_of_sound.unwrap_or(f32::NAN));
    }
    if before.range != node.range {
        audio.set_range(node.range.unwrap_or(DEFAULT_RANGE));
    }
    if before.music != node.music {
        if let Some(old) = music.voice.take() {
            audio.stop(old, MUSIC_FADE);
        }
        music.voice = node.music.as_deref().map(|uri| {
            let sound = server.load(&mut sounds, uri);
            audio.play(sound, Play { volume: node.music_volume, fade_in: MUSIC_FADE, bus: Bus::Music, ..Play::looped() })
        });
    } else if before.music_volume != node.music_volume
        && let Some(voice) = music.voice
    {
        audio.set_voice_volume(voice, node.music_volume, VOLUME_FADE);
    }
    music.declared = node;
}

pub(crate) fn play_sound(
    mut reader: MessageReader<PlaySound>,
    mut audio: ResMut<Audio>,
    mut server: ResMut<AssetServer>,
    mut sounds: ResMut<Assets<Sound>>,
    mut emitters: ResMut<SoundEmitters>,
) {
    if !audio.enabled() {
        reader.clear();
        return;
    }
    for s in reader.read() {
        let sound = server.load(&mut sounds, &s.src);
        let play = Play { volume: s.volume, pitch: s.pitch, pan: s.pan, bus: s.bus.into(), emit: s.space.emit(None), ..Play::default() };
        if s.space.follow.is_some() {
            // 跟着实体走的：等这一帧的位置都定了（物理、跟随之后）再开始
            emitters.queue(sound, play, s.clone());
        } else if s.space.space == SpaceKind::D3 {
            let at3 = glam::Vec3::new(s.x.unwrap_or(0.0), s.y.unwrap_or(0.0), s.space.z.unwrap_or(0.0));
            audio.play(sound, Play { at3: Some(at3), ..play });
        } else {
            let at = s.x.zip(s.y).map(|(x, y)| Vec2::new(x, y));
            audio.play(sound, Play { at, ..play });
        }
    }
}

/// 听的人放在 2D 相机的位置上（画面中心，不含震屏：震屏时声音不跟着抖）。
pub(crate) fn listen(camera: Query<&Camera2d, With<SceneCamera>>, rig: Res<crate::camera::CameraRig>, mut audio: ResMut<Audio>) {
    if let Ok(camera) = camera.single() {
        let at = rig.unshaken(camera.position);
        if audio.listener() != at {
            audio.set_listener(at);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::{BEEP, app, frame};

    const OTHER: &str = "asset:/@t/missing.wav";

    fn settle(app: &mut nge_ecs::App) {
        AssetServer::apply(app.world_mut(), true);
        frame(app, None);
    }

    fn voices(app: &nge_ecs::App) -> usize {
        app.world().resource::<Audio>().voices()
    }

    #[test]
    fn music_starts_changes_and_stops() {
        let mut app = app();
        frame(&mut app, Some(json!({ "audio": { "music": BEEP, "musicVolume": 0.5 } })));
        settle(&mut app);
        assert_eq!(voices(&app), 1);

        frame(&mut app, Some(json!({ "audio": { "music": BEEP, "musicVolume": 0.8, "volume": 0.3, "muted": true } })));
        let audio = app.world().resource::<Audio>();
        assert_eq!((audio.voices(), audio.volume(), audio.muted()), (1, 0.3, true), "只改音量不重新开始");

        frame(&mut app, Some(json!({ "audio": { "music": OTHER } })));
        settle(&mut app);
        assert_eq!(voices(&app), 0, "换成加载失败的音乐：旧的停掉，新的不播");
        let audio = app.world().resource::<Audio>();
        assert_eq!((audio.volume(), audio.muted()), (1.0, false));

        frame(&mut app, Some(json!({ "audio": { "music": BEEP } })));
        settle(&mut app);
        frame(&mut app, Some(json!({})));
        assert_eq!(voices(&app), 0, "去掉 audio 就停");
    }

    #[test]
    fn sound_command_and_preload() {
        let mut app = app();
        frame(&mut app, Some(json!({ "audio": { "preload": [BEEP] } })));
        AssetServer::apply(app.world_mut(), true);
        assert_eq!(app.world().resource::<Assets<Sound>>().len(), 1, "提前加载了");

        app.command("sound", &json!({ "src": BEEP, "volume": 0.5, "pitch": 1.5 }));
        frame(&mut app, None);
        assert_eq!(voices(&app), 1, "已经加载好的这一帧就开始");
    }

    #[test]
    fn buses_follow_the_scene_and_music_goes_to_its_bus() {
        let mut app = app();
        frame(&mut app, Some(json!({ "audio": { "music": BEEP, "range": 500,
            "buses": { "music": { "volume": 0.4 }, "sfx": { "lowpass": 600, "reverb": 0.3 }, "ui": { "volume": 0.7 } } } })));
        settle(&mut app);
        let audio = app.world().resource::<Audio>();
        assert_eq!((audio.bus_volume(Bus::Music), audio.lowpass(Bus::Sfx), audio.reverb(Bus::Sfx), audio.bus_volume(Bus::Ui)), (0.4, Some(600.0), 0.3, 0.7));
        assert_eq!(audio.range(), 500.0);
        assert_eq!(Bus::ALL.map(|b| audio.bus_sounds(b)), [1, 0, 0], "音乐进 music 分组");
        frame(&mut app, Some(json!({ "audio": {} })));
        let audio = app.world().resource::<Audio>();
        assert_eq!((audio.bus_volume(Bus::Music), audio.lowpass(Bus::Sfx), audio.reverb(Bus::Sfx), audio.bus_volume(Bus::Ui)), (1.0, None, 0.0, 1.0), "去掉回到默认");
        assert_eq!(audio.range(), DEFAULT_RANGE);
    }

    #[test]
    fn sounds_with_a_place_and_a_bus_and_the_listener_follows_the_camera() {
        let mut app = app();
        frame(&mut app, Some(json!({ "audio": { "preload": [BEEP] }, "camera": { "x": 200, "y": 0 } })));
        AssetServer::apply(app.world_mut(), true);
        frame(&mut app, None);
        assert_eq!(app.world().resource::<Audio>().listener(), Vec2::new(200.0, 0.0), "听的人在相机那儿");
        app.command("shake", &json!({ "strength": 20, "duration": 5 }));
        frame(&mut app, None);
        frame(&mut app, None);
        assert_eq!(app.world().resource::<Audio>().listener(), Vec2::new(200.0, 0.0), "震屏时不跟着抖");
        app.command("sound", &json!({ "src": BEEP, "x": -50, "y": 0, "volume": 0.8 }));
        app.command("sound", &json!({ "src": BEEP, "bus": "ui", "x": 10 }));
        frame(&mut app, None);
        let audio = app.world().resource::<Audio>();
        let mut mixes: Vec<(f32, f32)> = audio.playing().filter_map(|v| audio.mix(v)).collect();
        mixes.sort_by(|a, b| a.0.total_cmp(&b.0));
        // 左边 250：音量 0.8 × 0.75，声像 -0.5；只写了 x 的不按位置
        assert_eq!(mixes.len(), 2);
        assert!((mixes[0].0 - 0.6).abs() < 1e-6 && (mixes[0].1 + 0.5).abs() < 1e-6, "{mixes:?}");
        assert_eq!(mixes[1], (1.0, 0.0));
        assert_eq!(Bus::ALL.map(|b| audio.bus_sounds(b)), [0, 1, 1]);
    }
}
