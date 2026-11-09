use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bevy_ecs::prelude::*;
use glam::Vec2;
use kira::effect::filter::{FilterBuilder, FilterHandle};
use kira::effect::reverb::{ReverbBuilder, ReverbHandle};
use kira::sound::PlaybackState;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::track::{MainTrackHandle, TrackBuilder, TrackHandle};
use kira::{Decibels, Mix, Panning, Tween};
use nge_assets::{Assets, Handle, LoadState};

use crate::{Manager, Sound};

/// 同时发声的上限（循环的声音不算）。超出时新的音效直接丢掉。
pub const MAX_VOICES: usize = 32;
/// 一帧里同一个声音最多开始几次：碰撞多的时候不叠成一片。
pub const MAX_STARTS_PER_FRAME: u8 = 4;

/// 按位置的声音多远就听不见（世界单位）。
pub const DEFAULT_RANGE: f32 = 1000.0;
/// 听的人或声音挪了以后，音量和声像用多久过渡过去。
const SPATIAL_FADE: f32 = 0.05;
/// 低通滤波的截止频率范围（赫兹）。
const CUTOFF: (f32, f32) = (20.0, 20000.0);

/// 混音分组。每组一条子轨道，上面串着低通滤波和混响（见 docs/audio-more.md）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Bus {
    Music,
    #[default]
    Sfx,
    Ui,
}

impl Bus {
    pub const ALL: [Bus; 3] = [Bus::Music, Bus::Sfx, Bus::Ui];

    fn index(self) -> usize {
        self as usize
    }
}

/// 一组的设置：音量（线性）、低通的截止频率（`None` 是关）、混响的湿声比例。
#[derive(Debug, Clone, Copy, PartialEq)]
struct BusState {
    volume: f32,
    lowpass: Option<f32>,
    reverb: f32,
}

impl Default for BusState {
    fn default() -> Self {
        Self { volume: 1.0, lowpass: None, reverb: 0.0 }
    }
}

struct BusTrack {
    track: TrackHandle,
    filter: FilterHandle,
    /// 不出声的后端（采样率 1）没有：kira 的混响按采样率开缓冲，太小会越界。
    reverb: Option<ReverbHandle>,
}

/// 一次播放。音量是线性的（0 = 静音，1 = 原音量），`pitch` 是播放速度（2 = 高八度，限制在 0.1 到 8），`pan` 从 -1（左）到 1（右）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Play {
    pub volume: f32,
    pub pitch: f32,
    pub pan: f32,
    pub looped: bool,
    /// 淡入的秒数。
    pub fade_in: f32,
    /// 进哪个混音分组。
    pub bus: Bus,
    /// 在世界里的位置：离听的人越远越小，在左边就偏左（[`spatial`]）。`None` 不按位置。
    pub at: Option<Vec2>,
}

impl Default for Play {
    fn default() -> Self {
        Self { volume: 1.0, pitch: 1.0, pan: 0.0, looped: false, fade_in: 0.0, bus: Bus::Sfx, at: None }
    }
}

/// 按位置算音量和声像：音量乘上 `1 - 距离 / range`，声像加上 横向距离 / (range / 2)，都截到合法范围。
/// `range` 不是正数时按 [`DEFAULT_RANGE`]。
pub fn spatial(volume: f32, pan: f32, at: Vec2, listener: Vec2, range: f32) -> (f32, f32) {
    let pan = if pan.is_nan() { 0.0 } else { pan };
    let range = if range.is_finite() && range > 0.0 { range } else { DEFAULT_RANGE };
    let d = at - listener;
    if !d.is_finite() {
        return (0.0, pan);
    }
    let gain = (1.0 - d.length() / range).max(0.0);
    (volume * gain, (pan + d.x / (range / 2.0)).clamp(-1.0, 1.0))
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
    /// 自己的音量、声像（没按位置算的）。
    volume: f32,
    pan: f32,
    at: Option<Vec2>,
    /// 现在的音量和声像（按位置算过的）。
    mix: (f32, f32),
    /// 游戏设的音量淡变什么时候结束：按位置更新时不把它缩短。
    fade_until: Option<Instant>,
    /// 测试用：上一次按位置更新用的过渡秒数。
    #[cfg(test)]
    last_fade: f32,
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
    /// 三个分组的轨道（没有音频设备时没有）。
    tracks: Option<Vec<BusTrack>>,
    buses: [BusState; 3],
    listener: Vec2,
    range: f32,
}

impl Audio {
    pub(crate) fn new(manager: Option<Mutex<Manager>>) -> Self {
        Self {
            manager,
            next: 0,
            pending: Vec::new(),
            playing: HashMap::new(),
            failed: HashSet::new(),
            volume: 1.0,
            muted: false,
            tracks: None,
            buses: [BusState::default(); 3],
            listener: Vec2::ZERO,
            range: DEFAULT_RANGE,
        }
    }

    /// 给每个分组建一条子轨道：低通、混响，一开始都是干声。
    pub(crate) fn add_buses(&mut self) {
        let Some(manager) = &self.manager else { return };
        let mut manager = manager.lock().unwrap_or_else(|e| e.into_inner());
        let mock = matches!(*manager, Manager::Mock(_));
        let mut tracks = Vec::new();
        for _ in Bus::ALL {
            let mut builder = TrackBuilder::new();
            // 低通一直是全湿，关掉时截止频率在 20000（听不出来）：开关时干湿交叉淡变中间会变响
            let filter = builder.add_effect(FilterBuilder::new().cutoff(f64::from(CUTOFF.1)).mix(Mix::WET));
            let reverb = (!mock).then(|| builder.add_effect(ReverbBuilder::new().mix(Mix::DRY)));
            let track = match &mut *manager {
                Manager::Device(m) => m.add_sub_track(builder),
                Manager::Mock(m) => m.add_sub_track(builder),
            };
            match track {
                Ok(track) => tracks.push(BusTrack { track, filter, reverb }),
                Err(e) => {
                    log::warn!("建混音分组失败，声音直接进总输出: {e}");
                    return;
                }
            }
        }
        self.tracks = Some(tracks);
    }

    fn track(&mut self, bus: Bus) -> Option<&mut BusTrack> {
        self.tracks.as_mut().and_then(|t| t.get_mut(bus.index()))
    }

    pub fn bus_volume(&self, bus: Bus) -> f32 {
        self.buses[bus.index()].volume
    }

    /// 分组的音量（线性），`fade` 秒里过渡过去。
    pub fn set_bus_volume(&mut self, bus: Bus, volume: f32, fade: f32) {
        let volume = if volume.is_finite() { volume.max(0.0) } else { 1.0 };
        self.buses[bus.index()].volume = volume;
        if let Some(t) = self.track(bus) {
            t.track.set_volume(decibels(volume), tween(fade));
        }
    }

    /// 低通的截止频率（赫兹），`None` 是关。
    pub fn lowpass(&self, bus: Bus) -> Option<f32> {
        self.buses[bus.index()].lowpass
    }

    /// 打开（`Some(赫兹)`，截到 20 到 20000）或关掉低通滤波：水下、暂停时的闷声。
    pub fn set_lowpass(&mut self, bus: Bus, cutoff: Option<f32>, fade: f32) {
        let cutoff = cutoff.filter(|c| !c.is_nan()).map(|c| c.clamp(CUTOFF.0, CUTOFF.1));
        self.buses[bus.index()].lowpass = cutoff;
        if let Some(t) = self.track(bus) {
            t.filter.set_cutoff(f64::from(cutoff.unwrap_or(CUTOFF.1)), tween(fade));
        }
    }

    pub fn reverb(&self, bus: Bus) -> f32 {
        self.buses[bus.index()].reverb
    }

    /// 混响的湿声比例（截到 0 到 1），0 等于关掉。
    pub fn set_reverb(&mut self, bus: Bus, mix: f32, fade: f32) {
        let mix = if mix.is_finite() { mix.clamp(0.0, 1.0) } else { 0.0 };
        self.buses[bus.index()].reverb = mix;
        if let Some(r) = self.track(bus).and_then(|t| t.reverb.as_mut()) {
            r.set_mix(Mix(mix), tween(fade));
        }
    }

    pub fn listener(&self) -> Vec2 {
        self.listener
    }

    /// 听的人在哪（按位置的声音下一帧跟着变）。
    pub fn set_listener(&mut self, at: Vec2) {
        if at.is_finite() {
            self.listener = at;
        }
    }

    pub fn range(&self) -> f32 {
        self.range
    }

    /// 按位置的声音多远就听不见；不是正数时用默认的 1000。
    pub fn set_range(&mut self, range: f32) {
        self.range = if range.is_finite() && range > 0.0 { range } else { DEFAULT_RANGE };
    }

    /// 挪一个按位置的声音（不按位置的不管）。
    pub fn set_voice_position(&mut self, voice: Voice, at: Vec2) {
        if let Some((.., play)) = self.pending.iter_mut().find(|(v, ..)| *v == voice) {
            play.at = play.at.map(|_| at);
        } else if let Some(p) = self.playing.get_mut(&voice) {
            p.at = p.at.map(|_| at);
        }
    }

    /// 正在播的声音（不含排队和正在淡出的），顺序不固定。
    pub fn playing(&self) -> impl Iterator<Item = Voice> + '_ {
        self.playing.keys().copied()
    }

    /// 这个声音现在的音量和声像（按位置算过的）；没在播时是 `None`。
    pub fn mix(&self, voice: Voice) -> Option<(f32, f32)> {
        self.playing.get(&voice).map(|p| p.mix)
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
            let volume = sane_volume(volume);
            p.volume = volume;
            p.fade_until = Some(Instant::now() + tween(fade).duration);
            p.mix.0 = match p.at {
                Some(at) => spatial(volume, p.pan, at, self.listener, self.range).0,
                None => volume,
            };
            p.handle.set_volume(decibels(p.mix.0), tween(fade));
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

    /// 这个分组的轨道上有几个声音（含正在淡出的；没有音频设备时是 0）。
    pub fn bus_sounds(&self, bus: Bus) -> usize {
        self.tracks.as_ref().map_or(0, |t| t[bus.index()].track.num_sounds())
    }

    /// 测试用：上一次按位置更新用的过渡秒数。
    #[cfg(test)]
    pub(crate) fn last_fade(&self, voice: Voice) -> Option<f32> {
        self.playing.get(&voice).map(|p| p.last_fade)
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

/// 不是有限数的音量当 0，负数当 0。
fn sane_volume(volume: f32) -> f32 {
    if volume.is_finite() { volume.max(0.0) } else { 0.0 }
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

fn settings(data: &StaticSoundData, play: &Play, (volume, pan): (f32, f32)) -> StaticSoundData {
    let data = data
        .volume(decibels(volume))
        .playback_rate(f64::from(clamp_pitch(play.pitch)))
        .panning(pan)
        .fade_in_tween((play.fade_in > 0.0).then(|| tween(play.fade_in)));
    if play.looped { data.loop_region(..) } else { data }
}

/// 开始加载好了的声音（按上限丢掉多的），清理播完的。
pub(crate) fn update(mut audio: ResMut<Audio>, sounds: Res<Assets<Sound>>) {
    let audio = &mut *audio;
    audio.playing.retain(|_, p| p.handle.state() != PlaybackState::Stopped);
    // 按位置的声音：听的人或声音挪了就更新音量和声像
    let (listener, range) = (audio.listener, audio.range);
    for p in audio.playing.values_mut() {
        let Some(at) = p.at else { continue };
        let mix = spatial(p.volume, p.pan, at, listener, range);
        if mix != p.mix {
            // 游戏设的淡变还没走完：用剩下的时间，不把它缩成 50 ms
            let left = p.fade_until.map_or(0.0, |end| end.saturating_duration_since(Instant::now()).as_secs_f32());
            let fade = left.max(SPATIAL_FADE);
            p.handle.set_volume(decibels(mix.0), tween(fade));
            p.handle.set_panning(Panning(mix.1), tween(SPATIAL_FADE));
            p.mix = mix;
            #[cfg(test)]
            {
                p.last_fade = fade;
            }
        }
    }
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
        let pan = if play.pan.is_nan() { 0.0 } else { play.pan.clamp(-1.0, 1.0) };
        let play = Play { volume: sane_volume(play.volume), ..play };
        let mix = match play.at {
            Some(at) => spatial(play.volume, pan, at, listener, range),
            None => (play.volume, pan),
        };
        let data = settings(&data, &play, mix);
        // 进分组的轨道；没有分组时直接进总输出
        let started = match audio.tracks.as_mut().and_then(|t| t.get_mut(play.bus.index())) {
            Some(t) => t.track.play(data).map_err(|e| e.to_string()),
            None => {
                let Some(manager) = &audio.manager else { continue };
                match &mut *manager.lock().unwrap_or_else(|e| e.into_inner()) {
                    Manager::Device(m) => m.play(data).map_err(|e| e.to_string()),
                    Manager::Mock(m) => m.play(data).map_err(|e| e.to_string()),
                }
            }
        };
        match started {
            Ok(handle) => {
                if !play.looped {
                    *count += 1;
                    voices += 1;
                }
                audio.playing.insert(
                    voice,
                    Playing {
                        handle,
                        looped: play.looped,
                        volume: play.volume,
                        pan,
                        at: play.at,
                        mix,
                        fade_until: None,
                        #[cfg(test)]
                        last_fade: 0.0,
                    },
                );
            }
            Err(e) => log::warn!("播放失败: {e}"),
        }
    }
}
