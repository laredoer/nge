//! `scene.audio`（背景音乐、总音量、预加载、混音分组）、`sound` command，听的人跟着相机（见 docs/audio-more.md）。

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_assets::{AssetServer, Assets};
use nge_audio::{Audio, Bus, DEFAULT_RANGE, Play, Sound, Voice};
use nge_render2d::Camera2d;
use nge_core::CommandKey;
use serde::Deserialize;

use crate::SceneCamera;
use crate::protocol::Scene;

pub const SOUND: CommandKey<PlaySound> = CommandKey::new("sound");

/// 换音乐、停音乐时淡入淡出的秒数。
const MUSIC_FADE: f32 = 0.5;
/// 只改音乐音量时过渡的秒数。
const VOLUME_FADE: f32 = 0.2;
/// 分组的设置变了时过渡的秒数。
const BUS_FADE: f32 = 0.3;

/// 一个混音分组的设置。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default)]
pub struct BusNode {
    pub volume: f32,
    /// 低通的截止频率（赫兹），不写是关。
    pub lowpass: Option<f32>,
    /// 混响的湿声比例，0 到 1。
    pub reverb: f32,
}

impl Default for BusNode {
    fn default() -> Self {
        Self { volume: 1.0, lowpass: None, reverb: 0.0 }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct BusesNode {
    pub music: BusNode,
    pub sfx: BusNode,
    pub ui: BusNode,
}

impl BusesNode {
    fn get(&self, bus: Bus) -> BusNode {
        match bus {
            Bus::Music => self.music,
            Bus::Sfx => self.sfx,
            Bus::Ui => self.ui,
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
}

impl Default for AudioNode {
    fn default() -> Self {
        Self { music: None, music_volume: 1.0, volume: 1.0, muted: false, preload: Vec::new(), buses: BusesNode::default(), range: None }
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
    /// 在世界里的位置：两个都写了才按位置播。
    pub x: Option<f32>,
    pub y: Option<f32>,
}

impl Default for PlaySound {
    fn default() -> Self {
        Self { src: String::new(), volume: 1.0, pitch: 1.0, pan: 0.0, bus: BusName::Sfx, x: None, y: None }
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
    let node = scene.audio.clone().unwrap_or_default();
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
        if old.volume != new.volume {
            audio.set_bus_volume(bus, new.volume, BUS_FADE);
        }
        if old.lowpass != new.lowpass {
            audio.set_lowpass(bus, new.lowpass, BUS_FADE);
        }
        if old.reverb != new.reverb {
            audio.set_reverb(bus, new.reverb, BUS_FADE);
        }
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

pub(crate) fn play_sound(mut reader: MessageReader<PlaySound>, mut audio: ResMut<Audio>, mut server: ResMut<AssetServer>, mut sounds: ResMut<Assets<Sound>>) {
    if !audio.enabled() {
        reader.clear();
        return;
    }
    for s in reader.read() {
        let sound = server.load(&mut sounds, &s.src);
        let at = s.x.zip(s.y).map(|(x, y)| Vec2::new(x, y));
        audio.play(sound, Play { volume: s.volume, pitch: s.pitch, pan: s.pan, bus: s.bus.into(), at, ..Play::default() });
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
