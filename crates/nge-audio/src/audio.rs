use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use bevy_ecs::prelude::*;
use glam::{Vec2, Vec3};
use kira::sound::PlaybackState;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::track::MainTrackHandle;
use kira::{Decibels, Panning, Tween};
use nge_assets::{Assets, Handle, LoadState};
use nge_ecs::Time;
use web_time::Instant;

use crate::buses::{Bus, BusState, BusTrack, SendBus, default_send_effects, own_track};
use crate::effects::EffectSpec;
use crate::spatial3d::{Emit, doppler};
use crate::voice3d::{DOPPLER_FADE, Ears, MAX_OCCLUSION_TRACKS, Motion, OCCLUSION_BUDGET, Occlusion, OwnTrack, RATE_EPSILON, SPEED_OF_SOUND};
use crate::{Manager, Sound};

/// 同时发声的上限（循环的声音不算）。超出时新的音效直接丢掉。
pub const MAX_VOICES: usize = 32;
/// 一帧里同一个声音最多开始几次：碰撞多的时候不叠成一片。
pub const MAX_STARTS_PER_FRAME: u8 = 4;

/// 按位置的声音多远就听不见（世界单位）。
pub const DEFAULT_RANGE: f32 = 1000.0;
/// 听的人或声音挪了以后，音量和声像用多久过渡过去。
const SPATIAL_FADE: f32 = 0.05;
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
    /// 3D 的位置：按 3D 的听者算衰减和左右（见 docs/audio-3d.md）。和 `at` 都写了时用这个。
    pub at3: Option<Vec3>,
    /// 衰减、锥形（只对 3D 的）、多普勒、遮挡（2D、3D 都管用）。
    pub emit: Emit,
}

impl Default for Play {
    fn default() -> Self {
        Self { volume: 1.0, pitch: 1.0, pan: 0.0, looped: false, fade_in: 0.0, bus: Bus::Sfx, at: None, at3: None, emit: Emit::default() }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Voice(u64);

pub(crate) struct Playing {
    handle: StaticSoundHandle,
    looped: bool,
    pub(crate) bus: Bus,
    /// 自己的音量、声像（没按位置算的）。
    volume: f32,
    pan: f32,
    pub(crate) at: Option<Vec2>,
    pub(crate) at3: Option<Vec3>,
    pub(crate) emit: Emit,
    /// 现在的音量和声像（按位置、遮挡算过的）。
    mix: (f32, f32),
    /// 自己的音高，和现在给 kira 的播放速度（乘过多普勒的）。
    pitch: f32,
    pub(crate) rate: f32,
    motion: Motion,
    pub(crate) occlusion: Occlusion,
    /// 开了遮挡的声音自己的子轨道（带低通）；声音没了跟着没。
    pub(crate) own: Option<OwnTrack>,
    /// 游戏设的音量淡变什么时候结束：按位置更新时不把它缩短。
    fade_until: Option<Instant>,
    /// 测试用：上一次按位置更新用的过渡秒数。
    #[cfg(test)]
    last_fade: f32,
}

impl Playing {
    /// 在世界里的位置（2D 的 z 是 0）；不带位置时是 `None`。
    pub(crate) fn position(&self) -> Option<Vec3> {
        self.at3.or(self.at.map(|at| at.extend(0.0)))
    }

    /// 按位置和遮挡算出来的音量、声像。
    fn placed(&self, ears: &Ears) -> (f32, f32) {
        let (volume, pan) = ears.mix(self.volume, self.pan, self.at, self.at3, &self.emit);
        (volume * self.occlusion.mix().0, pan)
    }
}

#[derive(Resource)]
pub struct Audio {
    pub(crate) manager: Option<Mutex<Manager>>,
    next: u64,
    pub(crate) pending: Vec<(Voice, Handle<Sound>, Play)>,
    pub(crate) playing: HashMap<Voice, Playing>,
    /// 已经报过加载失败的声音，每个只 warn 一次。
    failed: HashSet<Handle<Sound>>,
    volume: f32,
    muted: bool,
    /// 画面没了、App 进了后台：分组轨道停下，总输出静音（[`Audio::set_suspended`]）。
    suspended: bool,
    /// 三个分组的轨道（没有音频设备时没有）。
    pub(crate) tracks: Option<Vec<BusTrack>>,
    pub(crate) buses: [BusState; 3],
    /// 混响总线和它上面的链。
    pub(crate) send_bus: Option<SendBus>,
    pub(crate) send_effects: Vec<EffectSpec>,
    /// 听的人：2D 的位置和范围，3D 的位置和右手方向。
    pub(crate) ears: Ears,
    /// 2D、3D 的听者各自的速度（多普勒用）。
    motion: Motion,
    motion3: Motion,
    pub(crate) speed_of_sound: f32,
    /// 这一帧还能给几个声音查遮挡。
    pub(crate) probes_left: usize,
    /// 等解锁（浏览器）：还没有音频设备，[`crate::unlock`] 之后接上。
    locked: bool,
    /// 接上了解锁来的设备（算在 [`crate::LIVE`] 里）。
    unlocked: bool,
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
            suspended: false,
            tracks: None,
            buses: Default::default(),
            send_bus: None,
            send_effects: default_send_effects(),
            ears: Ears { at: Vec2::ZERO, range: DEFAULT_RANGE, at3: Vec3::ZERO, right: Vec3::X },
            motion: Motion::default(),
            motion3: Motion::default(),
            speed_of_sound: SPEED_OF_SOUND,
            probes_left: OCCLUSION_BUDGET,
            locked: false,
            unlocked: false,
        }
    }

    pub(crate) fn locked() -> Self {
        let mut audio = Self::new(None);
        audio.locked = true;
        audio
    }

    /// 解锁了：接上音频设备，之前设的音量、分组设置补上去。
    fn attach(&mut self, manager: Manager) {
        self.manager = Some(Mutex::new(manager));
        self.locked = false;
        self.unlocked = true;
        crate::LIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // 效果器链和送出量建轨道时就带上了
        self.add_buses();
        self.apply_master();
        for bus in Bus::ALL {
            let (volume, lowpass, reverb) = (self.buses[bus.index()].volume, self.buses[bus.index()].lowpass, self.buses[bus.index()].reverb);
            self.set_bus_volume(bus, volume, 0.0);
            self.set_lowpass(bus, lowpass, 0.0);
            self.set_reverb(bus, reverb, 0.0);
        }
    }

    pub fn listener(&self) -> Vec2 {
        self.ears.at
    }

    /// 听的人在哪（按位置的声音下一帧跟着变）。
    pub fn set_listener(&mut self, at: Vec2) {
        if at.is_finite() {
            self.ears.at = at;
        }
    }

    pub fn range(&self) -> f32 {
        self.ears.range
    }

    /// 按位置的声音多远就听不见；不是正数时用默认的 1000。
    pub fn set_range(&mut self, range: f32) {
        self.ears.range = if range.is_finite() && range > 0.0 { range } else { DEFAULT_RANGE };
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
    /// 等解锁时（浏览器）循环的排着，别的丢掉。
    pub fn play(&mut self, sound: Handle<Sound>, play: Play) -> Voice {
        let voice = Voice(self.next);
        self.next += 1;
        if self.enabled() || (self.locked && play.looped) {
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
            p.mix.0 = p.placed(&self.ears).0;
            p.handle.set_volume(decibels(p.mix.0), tween(fade));
        }
    }

    /// 改一个声音的音高（播放速度，限制在 0.1 到 8）；多普勒在它上面再乘。
    pub fn set_voice_pitch(&mut self, voice: Voice, pitch: f32) {
        if let Some((.., play)) = self.pending.iter_mut().find(|(v, ..)| *v == voice) {
            play.pitch = pitch;
        } else if let Some(p) = self.playing.get_mut(&voice) {
            let pitch = clamp_pitch(pitch);
            if p.pitch != pitch {
                // 不带位置的声音没有每帧的更新：这里直接设
                p.rate = clamp_pitch(p.rate / p.pitch * pitch);
                p.pitch = pitch;
                p.handle.set_playback_rate(f64::from(p.rate), tween(DOPPLER_FADE));
            }
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

    pub fn suspended(&self) -> bool {
        self.suspended
    }

    /// 画面没了、App 进了后台时停下（分组轨道暂停，直接进总输出的静音），回来时接着放。
    pub fn set_suspended(&mut self, suspended: bool) {
        if self.suspended == suspended {
            return;
        }
        self.suspended = suspended;
        self.pause_buses(suspended);
        self.apply_master();
    }

    /// 主轨道实际的音量：静音或者暂停着（[`Audio::set_suspended`]）时是 0。
    pub fn master_level(&self) -> f32 {
        if self.muted || self.suspended { 0.0 } else { self.volume }
    }

    fn apply_master(&mut self) {
        let volume = self.master_level();
        with_main_track(&self.manager, |track| track.set_volume(decibels(volume), tween(0.05)));
    }

    /// 测试用：声音放到了哪儿（秒）。
    #[cfg(test)]
    pub(crate) fn position(&self, voice: Voice) -> Option<f64> {
        self.playing.get(&voice).map(|p| p.handle.position())
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

impl Drop for Audio {
    fn drop(&mut self) {
        if self.unlocked {
            crate::LIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
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

/// 开始加载好了的声音（按上限丢掉多的），清理播完的，带位置的声音跟着听的人和自己的位置更新。
pub(crate) fn update(mut audio: ResMut<Audio>, sounds: Res<Assets<Sound>>, time: Res<Time>) {
    let audio = &mut *audio;
    if audio.locked {
        match crate::take_unlocked() {
            Some(manager) => audio.attach(manager),
            None => return,
        }
    }
    audio.collect_chains();
    audio.probes_left = OCCLUSION_BUDGET;
    audio.playing.retain(|_, p| p.handle.state() != PlaybackState::Stopped);
    // 声音不受时间缩放影响：按真实时间算速度和遮挡的过渡
    let dt = time.real_delta;
    let ears = audio.ears;
    audio.motion.step(ears.at.extend(0.0), dt);
    audio.motion3.step(ears.at3, dt);
    let (velocity, velocity3, speed_of_sound) = (audio.motion.velocity, audio.motion3.velocity, audio.speed_of_sound);
    for p in audio.playing.values_mut() {
        let Some(at) = p.position() else { continue };
        p.motion.step(at, dt);
        p.occlusion.step(dt);
        // 音量和声像：听的人或声音挪了、遮挡变了就更新
        let mix = p.placed(&ears);
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
        // 多普勒：3D 的声音对 3D 的听者，2D 的对 2D 的
        let (listener, listener_velocity) = if p.at3.is_some() { (ears.at3, velocity3) } else { (ears.at.extend(0.0), velocity) };
        let rate = clamp_pitch(p.pitch * doppler(p.motion.velocity, listener_velocity, at, listener, speed_of_sound, p.emit.doppler));
        if (rate - p.rate).abs() > RATE_EPSILON || (rate == p.pitch && p.rate != p.pitch) {
            p.handle.set_playback_rate(f64::from(rate), tween(DOPPLER_FADE));
            p.rate = rate;
        }
        // 遮挡的低通
        if let Some(own) = &mut p.own {
            let cutoff = p.occlusion.mix().1;
            if (cutoff - own.cutoff).abs() > 1.0 || (cutoff != own.cutoff && p.occlusion.amount == 0.0) {
                own.filter.set_cutoff(f64::from(cutoff), tween(SPATIAL_FADE));
                own.cutoff = cutoff;
            }
        }
    }
    let mut starts: HashMap<Handle<Sound>, u8> = HashMap::new();
    let mut voices = audio.playing.values().filter(|p| !p.looped).count();
    let mut own_tracks = audio.playing.values().filter(|p| p.own.is_some()).count();
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
        let play = Play { volume: sane_volume(play.volume), pitch: clamp_pitch(play.pitch), emit: play.emit.sane(), ..play };
        let mix = ears.mix(play.volume, pan, play.at, play.at3, &play.emit);
        let data = settings(&data, &play, mix);
        let placed = play.at.is_some() || play.at3.is_some();
        // 进分组的轨道（开了遮挡、带位置的进自己的子轨道）；没有分组时直接进总输出
        let (started, own) = match audio.tracks.as_mut().and_then(|t| t.get_mut(play.bus.index())) {
            Some(t) => {
                let mut own = (play.emit.occlude && placed && own_tracks < MAX_OCCLUSION_TRACKS).then(|| own_track(t)).flatten();
                let started = match &mut own {
                    Some(own) => own.track.play(data),
                    None => t.track.play(data),
                };
                (started.map_err(|e| e.to_string()), own)
            }
            None => {
                let Some(manager) = &audio.manager else { continue };
                let started = match &mut *manager.lock().unwrap_or_else(|e| e.into_inner()) {
                    Manager::Device(m) => m.play(data).map_err(|e| e.to_string()),
                    Manager::Mock(m) => m.play(data).map_err(|e| e.to_string()),
                };
                (started, None)
            }
        };
        match started {
            Ok(handle) => {
                if !play.looped {
                    *count += 1;
                    voices += 1;
                }
                own_tracks += usize::from(own.is_some());
                audio.playing.insert(
                    voice,
                    Playing {
                        handle,
                        looped: play.looped,
                        bus: play.bus,
                        volume: play.volume,
                        pan,
                        at: play.at,
                        at3: play.at3,
                        emit: play.emit,
                        mix,
                        pitch: play.pitch,
                        rate: play.pitch,
                        motion: Motion::default(),
                        occlusion: Occlusion::default(),
                        own,
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
