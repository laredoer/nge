use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use bevy_ecs::prelude::*;
use kira::sound::PlaybackState;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::track::MainTrackHandle;
use kira::{Decibels, Tween};
use nge_assets::{Assets, Handle, LoadState};

use crate::{Manager, Sound};

/// 同时发声的上限（循环的声音不算）。超出时新的音效直接丢掉。
pub const MAX_VOICES: usize = 32;
/// 一帧里同一个声音最多开始几次：碰撞多的时候不叠成一片。
pub const MAX_STARTS_PER_FRAME: u8 = 4;

/// 一次播放。音量是线性的（0 = 静音，1 = 原音量），`pitch` 是播放速度（2 = 高八度，限制在 0.1 到 8），`pan` 从 -1（左）到 1（右）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Play {
    pub volume: f32,
    pub pitch: f32,
    pub pan: f32,
    pub looped: bool,
    /// 淡入的秒数。
    pub fade_in: f32,
}

impl Default for Play {
    fn default() -> Self {
        Self { volume: 1.0, pitch: 1.0, pan: 0.0, looped: false, fade_in: 0.0 }
    }
}

impl Play {
    pub fn looped() -> Self {
        Self { looped: true, ..Self::default() }
    }
}

/// 正在播（或排队等加载）的一个声音。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Voice(u64);

struct Playing {
    handle: StaticSoundHandle,
    looped: bool,
}

#[derive(Resource)]
pub struct Audio {
    manager: Option<Mutex<Manager>>,
    next: u64,
    pending: Vec<(Voice, Handle<Sound>, Play)>,
    playing: HashMap<Voice, Playing>,
    /// 已经报过加载失败的声音，每个只 warn 一次。
    failed: HashSet<Handle<Sound>>,
    volume: f32,
    muted: bool,
}

impl Audio {
    pub(crate) fn new(manager: Option<Mutex<Manager>>) -> Self {
        Self { manager, next: 0, pending: Vec::new(), playing: HashMap::new(), failed: HashSet::new(), volume: 1.0, muted: false }
    }

    /// 播放一个声音。还没加载完就先排队，加载完的那一帧开始；加载失败记 warn 丢掉。没有音频设备时什么都不做。
    pub fn play(&mut self, sound: Handle<Sound>, play: Play) -> Voice {
        let voice = Voice(self.next);
        self.next += 1;
        if self.enabled() {
            self.pending.push((voice, sound, play));
        }
        voice
    }

    /// 有没有音频设备（或者 mock 后端）。
    pub fn enabled(&self) -> bool {
        self.manager.is_some()
    }

    /// 在 `fade` 秒里淡出并停止。
    pub fn stop(&mut self, voice: Voice, fade: f32) {
        self.pending.retain(|(v, ..)| *v != voice);
        if let Some(mut p) = self.playing.remove(&voice) {
            p.handle.stop(tween(fade));
        }
    }

    pub fn set_voice_volume(&mut self, voice: Voice, volume: f32, fade: f32) {
        if let Some((.., play)) = self.pending.iter_mut().find(|(v, ..)| *v == voice) {
            play.volume = volume;
        } else if let Some(p) = self.playing.get_mut(&voice) {
            p.handle.set_volume(decibels(volume), tween(fade));
        }
    }

    /// 是不是还在播（或者排队等加载）。`stop` 之后正在淡出的不算。
    pub fn is_playing(&self, voice: Voice) -> bool {
        self.playing.contains_key(&voice) || self.pending.iter().any(|(v, ..)| *v == voice)
    }

    /// 正在发声的数量（不含正在淡出的）。
    pub fn voices(&self) -> usize {
        self.playing.len()
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.max(0.0);
        self.apply_master();
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        self.apply_master();
    }

    fn apply_master(&mut self) {
        let volume = if self.muted { 0.0 } else { self.volume };
        with_main_track(&self.manager, |track| track.set_volume(decibels(volume), tween(0.05)));
    }

    /// 测试用：排队中的声音的音量。
    #[cfg(test)]
    pub(crate) fn queued_volume(&self, voice: Voice) -> Option<f32> {
        self.pending.iter().find(|(v, ..)| *v == voice).map(|(.., play)| play.volume)
    }

    /// 测试用：让不出声的后端往前走 `frames` 帧（采样率是 1，一帧 = 一秒）。
    #[cfg(test)]
    pub(crate) fn advance_mock(&mut self, frames: usize) {
        if let Some(Manager::Mock(m)) = self.manager.as_mut().map(|m| m.get_mut().unwrap_or_else(|e| e.into_inner())) {
            for _ in 0..frames {
                m.backend_mut().on_start_processing();
                m.backend_mut().process();
            }
        }
    }
}

fn with_main_track(manager: &Option<Mutex<Manager>>, f: impl FnOnce(&mut MainTrackHandle)) {
    let Some(manager) = manager else { return };
    match &mut *manager.lock().unwrap_or_else(|e| e.into_inner()) {
        Manager::Device(m) => f(m.main_track()),
        Manager::Mock(m) => f(m.main_track()),
    }
}

/// 线性音量 → 分贝（0 和负数是静音）。
pub(crate) fn decibels(volume: f32) -> Decibels {
    if volume > 0.0 && volume.is_finite() { Decibels((20.0 * volume.log10()).max(Decibels::SILENCE.0)) } else { Decibels::SILENCE }
}

/// 过渡时间最长 60 秒（太大的数 `Duration::from_secs_f32` 会 panic）。
pub(crate) fn tween(seconds: f32) -> Tween {
    let seconds = if seconds.is_finite() { seconds.clamp(0.0, 60.0) } else { 0.0 };
    // kira 的默认是 10 ms，太短会有爆音
    Tween { duration: Duration::from_secs_f32(seconds).max(Duration::from_millis(10)), ..Default::default() }
}

/// 太快会卡住音频线程，太慢的声音会一直占着上限；不是正数时用 1。
pub(crate) fn clamp_pitch(pitch: f32) -> f32 {
    if pitch.is_finite() && pitch > 0.0 { pitch.clamp(0.1, 8.0) } else { 1.0 }
}

fn settings(data: &StaticSoundData, play: &Play) -> StaticSoundData {
    let pan = if play.pan.is_nan() { 0.0 } else { play.pan.clamp(-1.0, 1.0) };
    let data = data
        .volume(decibels(play.volume))
        .playback_rate(f64::from(clamp_pitch(play.pitch)))
        .panning(pan)
        .fade_in_tween((play.fade_in > 0.0).then(|| tween(play.fade_in)));
    if play.looped { data.loop_region(..) } else { data }
}

/// 开始加载好了的声音（按上限丢掉多的），清理播完的。
pub(crate) fn update(mut audio: ResMut<Audio>, sounds: Res<Assets<Sound>>) {
    let audio = &mut *audio;
    audio.playing.retain(|_, p| p.handle.state() != PlaybackState::Stopped);
    let mut starts: HashMap<Handle<Sound>, u8> = HashMap::new();
    let mut voices = audio.playing.values().filter(|p| !p.looped).count();
    let pending = std::mem::take(&mut audio.pending);
    for (voice, sound, play) in pending {
        let data = match sounds.state(sound) {
            LoadState::Loading => {
                audio.pending.push((voice, sound, play));
                continue;
            }
            LoadState::Failed(e) => {
                if audio.failed.insert(sound) {
                    log::warn!("声音加载失败，不播了: {e}");
                }
                continue;
            }
            LoadState::Loaded => match sounds.get(sound) {
                Some(s) => s.0.clone(),
                None => continue,
            },
        };
        // 循环的（音乐）不占上限，也不占一帧里的次数
        let count = starts.entry(sound).or_default();
        if !play.looped && (voices >= MAX_VOICES || *count >= MAX_STARTS_PER_FRAME) {
            continue;
        }
        let Some(manager) = &audio.manager else { continue };
        let data = settings(&data, &play);
        let started = match &mut *manager.lock().unwrap_or_else(|e| e.into_inner()) {
            Manager::Device(m) => m.play(data).map_err(|e| e.to_string()),
            Manager::Mock(m) => m.play(data).map_err(|e| e.to_string()),
        };
        match started {
            Ok(handle) => {
                if !play.looped {
                    *count += 1;
                    voices += 1;
                }
                audio.playing.insert(voice, Playing { handle, looped: play.looped });
            }
            Err(e) => log::warn!("播放失败: {e}"),
        }
    }
}
