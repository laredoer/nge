//! `scene.audio`（背景音乐、总音量、预加载）和 `sound` command。

use bevy_ecs::prelude::*;
use nge_assets::{AssetServer, Assets};
use nge_audio::{Audio, Play, Sound, Voice};
use nge_core::CommandKey;
use serde::Deserialize;

use crate::protocol::Scene;

pub const SOUND: CommandKey<PlaySound> = CommandKey::new("sound");

/// 换音乐、停音乐时淡入淡出的秒数。
const MUSIC_FADE: f32 = 0.5;
/// 只改音乐音量时过渡的秒数。
const VOLUME_FADE: f32 = 0.2;

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
}

impl Default for AudioNode {
    fn default() -> Self {
        Self { music: None, music_volume: 1.0, volume: 1.0, muted: false, preload: Vec::new() }
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
}

impl Default for PlaySound {
    fn default() -> Self {
        Self { src: String::new(), volume: 1.0, pitch: 1.0, pan: 0.0 }
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
    if before.music != node.music {
        if let Some(old) = music.voice.take() {
            audio.stop(old, MUSIC_FADE);
        }
        music.voice = node.music.as_deref().map(|uri| {
            let sound = server.load(&mut sounds, uri);
            audio.play(sound, Play { volume: node.music_volume, fade_in: MUSIC_FADE, ..Play::looped() })
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
        audio.play(sound, Play { volume: s.volume, pitch: s.pitch, pan: s.pan, ..Play::default() });
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
}
