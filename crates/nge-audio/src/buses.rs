//! 混音分组：每组一条子轨道，上面串着原来的低通和混响（见 docs/audio-more.md），再加一条可配置的效果器链；
//! 还有一条公用的混响总线（kira 的 send 轨道），各组按比例往上送（见 docs/audio-3d.md）。

use kira::effect::filter::{FilterBuilder, FilterHandle};
use kira::effect::reverb::{ReverbBuilder, ReverbHandle};
use kira::track::{SendTrackBuilder, SendTrackHandle, TrackBuilder, TrackHandle};
use kira::Mix;

use crate::Manager;
use crate::audio::{Audio, decibels, tween};
use crate::chain::{ChainBuilder, ChainHandle};
use crate::effects::{EffectSpec, sanitize_chain};

/// 低通滤波的截止频率范围（赫兹）。
pub(crate) const CUTOFF: (f32, f32) = (20.0, 20000.0);
/// 混响总线上默认的链：一个全湿的混响。
const SEND_REVERB: EffectSpec = EffectSpec::Reverb { room: 0.7, damping: 0.5, mix: 1.0 };

/// 混音分组。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Bus {
    Music,
    #[default]
    Sfx,
    Ui,
}

impl Bus {
    pub const ALL: [Bus; 3] = [Bus::Music, Bus::Sfx, Bus::Ui];

    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

/// 一组的设置：音量（线性）、低通的截止频率（`None` 是关）、混响的湿声比例、效果器链、往混响总线送多少。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BusState {
    pub volume: f32,
    pub lowpass: Option<f32>,
    pub reverb: f32,
    pub effects: Vec<EffectSpec>,
    pub send: f32,
}

impl Default for BusState {
    fn default() -> Self {
        Self { volume: 1.0, lowpass: None, reverb: 0.0, effects: Vec::new(), send: 0.0 }
    }
}

pub(crate) struct BusTrack {
    pub track: TrackHandle,
    filter: FilterHandle,
    /// 不出声的后端（采样率 1）没有：kira 的混响按采样率开缓冲，太小会越界。
    reverb: Option<ReverbHandle>,
    chain: ChainHandle,
}

/// 混响总线：一条 send 轨道，上面一条效果器链。
pub(crate) struct SendBus {
    track: SendTrackHandle,
    chain: ChainHandle,
}

/// 混响总线上的链，默认是一个全湿的混响。
pub(crate) fn default_send_effects() -> Vec<EffectSpec> {
    vec![SEND_REVERB]
}

impl Audio {
    /// 建混响总线，再给每个分组建一条子轨道：低通、混响、效果器链，连到混响总线上（一开始不送）。
    /// 已经设过的效果器链、送出量直接带上。
    pub(crate) fn add_buses(&mut self) {
        let Some(manager) = &self.manager else { return };
        let mut manager = manager.lock().unwrap_or_else(|e| e.into_inner());
        let mock = matches!(*manager, Manager::Mock(_));
        let mut send_builder = SendTrackBuilder::new();
        let send_chain = send_builder.add_effect(ChainBuilder(self.send_effects.clone()));
        let send = match &mut *manager {
            Manager::Device(m) => m.add_send_track(send_builder),
            Manager::Mock(m) => m.add_send_track(send_builder),
        };
        let send = send.inspect_err(|e| log::warn!("建混响总线失败，分组往上送的都没声: {e}")).ok();
        let mut tracks = Vec::new();
        for bus in Bus::ALL {
            let state = &self.buses[bus.index()];
            let mut builder = TrackBuilder::new();
            // 低通一直是全湿，关掉时截止频率在 20000（听不出来）：开关时干湿交叉淡变中间会变响
            let filter = builder.add_effect(FilterBuilder::new().cutoff(f64::from(CUTOFF.1)).mix(Mix::WET));
            let reverb = (!mock).then(|| builder.add_effect(ReverbBuilder::new().mix(Mix::DRY)));
            let chain = builder.add_effect(ChainBuilder(state.effects.clone()));
            if let Some(send) = &send {
                builder = builder.with_send(send.id(), decibels(state.send));
            }
            let track = match &mut *manager {
                Manager::Device(m) => m.add_sub_track(builder),
                Manager::Mock(m) => m.add_sub_track(builder),
            };
            match track {
                Ok(track) => tracks.push(BusTrack { track, filter, reverb, chain }),
                Err(e) => {
                    log::warn!("建混音分组失败，声音直接进总输出: {e}");
                    return;
                }
            }
        }
        drop(manager);
        self.tracks = Some(tracks);
        self.send_bus = send.map(|track| SendBus { track, chain: send_chain });
    }

    pub(crate) fn track(&mut self, bus: Bus) -> Option<&mut BusTrack> {
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

    /// 这个分组的效果器链（校验过的）。
    pub fn effects(&self, bus: Bus) -> &[EffectSpec] {
        &self.buses[bus.index()].effects
    }

    /// 换这个分组的效果器链（接在原来的低通、混响后面，按顺序处理）。种类和顺序没变时参数在 `fade` 秒里过渡过去；
    /// 变了就换一条新的，30 毫秒交叉淡变。参数先校验（[`EffectSpec::sanitize`]），最多 8 个。
    pub fn set_effects(&mut self, bus: Bus, effects: &[EffectSpec], fade: f32) {
        let effects = sanitize_chain(effects);
        if let Some(t) = self.track(bus) {
            t.chain.set(&effects, fade);
        }
        self.buses[bus.index()].effects = effects;
    }

    /// 这个分组往混响总线送多少（线性，0 到 1）。
    pub fn send(&self, bus: Bus) -> f32 {
        self.buses[bus.index()].send
    }

    pub fn set_send(&mut self, bus: Bus, amount: f32, fade: f32) {
        let amount = if amount.is_finite() { amount.clamp(0.0, 1.0) } else { 0.0 };
        self.buses[bus.index()].send = amount;
        let Some(id) = self.send_bus.as_ref().map(|s| s.track.id()) else { return };
        if let Some(t) = self.track(bus) {
            // 建轨道时就连好了，不会没有这条路
            let _ = t.track.set_send(id, decibels(amount), tween(fade));
        }
    }

    /// 混响总线上的效果器链。
    pub fn send_effects(&self) -> &[EffectSpec] {
        &self.send_effects
    }

    /// 混响总线上默认的链：一个全湿的混响。
    pub fn default_send_effects() -> Vec<EffectSpec> {
        default_send_effects()
    }

    /// 换混响总线上的链（默认一个全湿的混响）；规则和 [`Audio::set_effects`] 一样。
    pub fn set_send_effects(&mut self, effects: &[EffectSpec], fade: f32) {
        let effects = sanitize_chain(effects);
        if let Some(s) = &mut self.send_bus {
            s.chain.set(&effects, fade);
        }
        self.send_effects = effects;
    }

    /// 释放音频线程换下来的旧效果器链。
    pub(crate) fn collect_chains(&mut self) {
        for t in self.tracks.iter_mut().flatten() {
            t.chain.collect();
        }
        if let Some(s) = &mut self.send_bus {
            s.chain.collect();
        }
    }

    /// 这个分组的轨道上有几个声音（含正在淡出的；没有音频设备时是 0）。有自己子轨道的（开了遮挡的）也算。
    pub fn bus_sounds(&self, bus: Bus) -> usize {
        let own = self.playing.values().filter(|p| p.bus == bus && p.own.is_some()).count();
        self.tracks.as_ref().map_or(0, |t| t[bus.index()].track.num_sounds() + own)
    }

    /// 分组轨道暂停 / 继续（App 进后台）。
    pub(crate) fn pause_buses(&mut self, paused: bool) {
        for bus in self.tracks.iter_mut().flatten() {
            if paused {
                bus.track.pause(tween(0.05));
            } else {
                bus.track.resume(tween(0.05));
            }
        }
    }
}

/// 给一个开了遮挡的声音在分组轨道下面建一条子轨道：只放它一个，上面一个全湿的低通。声音播完轨道自己没。
pub(crate) fn own_track(bus: &mut BusTrack) -> Option<crate::voice3d::OwnTrack> {
    let mut builder = TrackBuilder::new().sound_capacity(1).sub_track_capacity(0).persist_until_sounds_finish(true);
    let filter = builder.add_effect(FilterBuilder::new().cutoff(f64::from(CUTOFF.1)).mix(Mix::WET));
    let track = bus.track.add_sub_track(builder).ok()?;
    Some(crate::voice3d::OwnTrack { track, filter, cutoff: CUTOFF.1 })
}

