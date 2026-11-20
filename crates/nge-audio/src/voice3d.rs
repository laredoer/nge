//! 带位置的声音每帧的更新：3D 的衰减和声像、多普勒、遮挡（见 docs/audio-3d.md）。算法在 `spatial3d.rs`，这里把它们接到正在播的声音上。

use glam::{Vec2, Vec3};
use kira::effect::filter::FilterHandle;
use kira::track::TrackHandle;

use crate::audio::{Audio, Voice, spatial};
use crate::spatial3d::{Emit, approach, mix3, occlusion_mix, pick_probes, smooth, velocity};

/// 声速（point / 秒，一米 100）。
pub const SPEED_OF_SOUND: f32 = 34300.0;
/// 一帧里挪得比这还快（point / 秒）的当瞬移，不算进多普勒。
pub const TELEPORT_SPEED: f32 = 12000.0;
/// 多普勒变调用多久过渡给 kira（秒）。
pub(crate) const DOPPLER_FADE: f32 = 0.1;
/// 播放速度变了这么多才告诉 kira。
pub(crate) const RATE_EPSILON: f32 = 1e-3;
/// 遮挡量从没挡到全挡走多久（秒）。
pub const OCCLUSION_FADE: f32 = 0.25;
/// 一帧最多给几个声音打遮挡的射线；同一个声音至少隔多久查一次（秒）。
pub const OCCLUSION_BUDGET: usize = 4;
pub const OCCLUSION_INTERVAL: f32 = 0.1;
/// 开了遮挡的声音各有一条带低通的子轨道，同时最多这么多条；多出来的只压音量。
pub const MAX_OCCLUSION_TRACKS: usize = 16;

/// 这一帧该查遮挡的一个声音：从听的人（`from`）到声源（`to`）之间有没有东西挡着。
/// 2D 的声音（`three_d` 为假）z 是 0，用 2D 物理查。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probe {
    pub voice: Voice,
    pub from: Vec3,
    pub to: Vec3,
    pub three_d: bool,
}

/// 按两帧的位置估出来的速度（平滑过的）。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Motion {
    last: Option<Vec3>,
    pub velocity: Vec3,
}

impl Motion {
    /// 暂停（`dt` 不是正数）的帧不算。
    pub fn step(&mut self, now: Vec3, dt: f32) {
        if !(dt > 0.0) {
            return;
        }
        if let Some(last) = self.last {
            self.velocity = smooth(self.velocity, velocity(last, now, dt, TELEPORT_SPEED), dt);
        }
        self.last = Some(now);
    }
}

/// 听的人：2D 的位置和范围，3D 的位置和右手方向。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ears {
    pub at: Vec2,
    pub range: f32,
    pub at3: Vec3,
    pub right: Vec3,
}

impl Ears {
    /// 一个声音按位置算出来的音量和声像；不带位置的原样。3D 的位置优先。
    pub fn mix(&self, volume: f32, pan: f32, at: Option<Vec2>, at3: Option<Vec3>, emit: &Emit) -> (f32, f32) {
        match (at3, at) {
            (Some(at3), _) => mix3(volume, pan, at3, emit, self.at3, self.right),
            (None, Some(at)) => spatial(volume, pan, at, self.at, self.range),
            (None, None) => (volume, pan),
        }
    }
}

/// 开了遮挡的声音自己的子轨道：上面一个低通。
pub(crate) struct OwnTrack {
    pub track: TrackHandle,
    pub filter: FilterHandle,
    pub cutoff: f32,
}

/// 一个声音的遮挡状态。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Occlusion {
    /// 现在的遮挡量（0 到 1）和物理说的结果。
    pub amount: f32,
    pub blocked: bool,
    /// 离上一次打射线多久了；还没查过是无穷大（最先查）。
    pub age: f32,
    /// 查过了没有：第一次的结果直接用，不过渡。
    pub known: bool,
}

impl Default for Occlusion {
    fn default() -> Self {
        Self { amount: 0.0, blocked: false, age: f32::INFINITY, known: false }
    }
}

impl Occlusion {
    pub fn step(&mut self, dt: f32) {
        if dt > 0.0 {
            self.age += dt;
        }
        self.amount = approach(self.amount, if self.blocked { 1.0 } else { 0.0 }, dt, OCCLUSION_FADE);
    }

    /// 音量倍数和低通的截止频率。
    pub fn mix(&self) -> (f32, f32) {
        occlusion_mix(self.amount)
    }
}

impl Audio {
    /// 3D 的听者：位置和右手方向（世界坐标）。
    pub fn listener3(&self) -> (Vec3, Vec3) {
        (self.ears.at3, self.ears.right)
    }

    /// 3D 的听者在哪、右手朝哪（一般是 3D 相机的位置和它的 +x）。不是有限数的、没有方向的不要。
    pub fn set_listener3(&mut self, at: Vec3, right: Vec3) {
        if let (true, Some(right)) = (at.is_finite(), right.try_normalize()) {
            (self.ears.at3, self.ears.right) = (at, right);
        }
    }

    pub fn speed_of_sound(&self) -> f32 {
        self.speed_of_sound
    }

    /// 多普勒用的声速（point / 秒）；不是正数时用默认的 34300。
    pub fn set_speed_of_sound(&mut self, speed: f32) {
        self.speed_of_sound = if speed.is_finite() && speed > 0.0 { speed } else { SPEED_OF_SOUND };
    }

    /// 挪一个 3D 的声音（不是 3D 的不管）。
    pub fn set_voice_position3(&mut self, voice: Voice, at: Vec3) {
        if let Some((.., play)) = self.pending.iter_mut().find(|(v, ..)| *v == voice) {
            play.at3 = play.at3.map(|_| at);
        } else if let Some(p) = self.playing.get_mut(&voice) {
            p.at3 = p.at3.map(|_| at);
        }
    }

    /// 锥形朝哪。
    pub fn set_voice_forward(&mut self, voice: Voice, forward: Vec3) {
        if let Some((.., play)) = self.pending.iter_mut().find(|(v, ..)| *v == voice) {
            play.emit.forward = forward;
        } else if let Some(p) = self.playing.get_mut(&voice) {
            p.emit.forward = forward;
        }
    }

    /// 换一个声音的衰减、锥形、多普勒设置。要不要查遮挡在开始播的时候就定了，这里不改。
    pub fn set_voice_emit(&mut self, voice: Voice, emit: Emit) {
        if let Some((.., play)) = self.pending.iter_mut().find(|(v, ..)| *v == voice) {
            play.emit = Emit { occlude: play.emit.occlude, ..emit };
        } else if let Some(p) = self.playing.get_mut(&voice) {
            p.emit = Emit { occlude: p.emit.occlude, ..emit.sane() };
        }
    }

    /// 这个声音现在在哪（2D 的 z 是 0）；没在播、不带位置时是 `None`。
    pub fn voice_position(&self, voice: Voice) -> Option<Vec3> {
        self.playing.get(&voice).and_then(|p| p.position())
    }

    /// 这个声音的衰减、锥形、多普勒设置（校验过的）。
    pub fn voice_emit(&self, voice: Voice) -> Option<Emit> {
        self.playing.get(&voice).map(|p| p.emit)
    }

    /// 这个声音现在的播放速度（音高乘多普勒）；没在播时是 `None`。
    pub fn rate(&self, voice: Voice) -> Option<f32> {
        self.playing.get(&voice).map(|p| p.rate)
    }

    /// 这个声音现在的遮挡量（0 没挡、1 全挡）。
    pub fn occlusion(&self, voice: Voice) -> Option<f32> {
        self.playing.get(&voice).map(|p| p.occlusion.amount)
    }

    /// 这个声音自己的低通现在的截止频率；没有自己的轨道（没开遮挡、超出上限）时是 `None`。
    pub fn voice_cutoff(&self, voice: Voice) -> Option<f32> {
        self.playing.get(&voice).and_then(|p| p.own.as_ref()).map(|own| own.cutoff)
    }

    /// 这一帧该查遮挡的声音：开了遮挡、带位置、正在播的里面，至少隔了 [`OCCLUSION_INTERVAL`]、等得最久的先查，
    /// 一帧（两次音频更新之间）加起来最多 [`OCCLUSION_BUDGET`] 个。查完用 [`Audio::set_occluded`] 把结果告诉它。
    pub fn occlusion_probes(&mut self) -> Vec<Probe> {
        if self.probes_left == 0 {
            return Vec::new();
        }
        let mut voices: Vec<Voice> = self.playing.iter().filter(|(_, p)| p.emit.occlude && p.position().is_some()).map(|(v, _)| *v).collect();
        if voices.is_empty() {
            return Vec::new();
        }
        voices.sort();
        let ages: Vec<f32> = voices.iter().map(|v| self.playing[v].occlusion.age).collect();
        let ears = self.ears;
        let probes: Vec<Probe> = pick_probes(&ages, self.probes_left, OCCLUSION_INTERVAL)
            .into_iter()
            .filter_map(|i| {
                let p = self.playing.get_mut(&voices[i])?;
                p.occlusion.age = 0.0;
                let three_d = p.at3.is_some();
                Some(Probe { voice: voices[i], from: if three_d { ears.at3 } else { ears.at.extend(0.0) }, to: p.position()?, three_d })
            })
            .collect();
        self.probes_left -= probes.len();
        probes
    }

    /// 物理查出来的结果：这个声音被挡住了没有。音量和低通在 [`OCCLUSION_FADE`] 秒里过渡过去；
    /// 第一次的结果直接用（一开始就在墙后面的声音不会先响一下）。没开遮挡的声音不理。
    pub fn set_occluded(&mut self, voice: Voice, blocked: bool) {
        let Some(p) = self.playing.get_mut(&voice).filter(|p| p.emit.occlude) else { return };
        p.occlusion.blocked = blocked;
        if !p.occlusion.known {
            p.occlusion.known = true;
            p.occlusion.amount = if blocked { 1.0 } else { 0.0 };
        }
    }
}
