//! 音频（kira）：按 `asset:/` 加载 [`Sound`]（wav / ogg / mp3），用 [`Audio`] 播放、循环、调音量。
//! 没有音频设备时所有调用都是空操作。
//! 浏览器上页面被按过、点过之前不能出声：先不建音频设备，平台层在第一次按键 / 点按的回调里调 [`unlock`]，
//! 下一帧接上；之前的音效丢掉，循环的（音乐）排着等（见 docs/web.md）。

mod audio;
mod buses;
mod chain;
mod dsp;
pub mod effects;
pub mod spatial3d;
pub mod voice3d;

pub use audio::{Audio, DEFAULT_RANGE, MAX_STARTS_PER_FRAME, MAX_VOICES, Play, Voice, spatial};
pub use buses::Bus;
pub use effects::EffectSpec;
pub use spatial3d::{Cone, Emit, Rolloff};
pub use voice3d::Probe;

use std::io::Cursor;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use bevy_ecs::prelude::*;
use kira::backend::mock::MockBackend;
use kira::sound::static_sound::StaticSoundData;
use kira::{AudioManager, AudioManagerSettings, DefaultBackend};
use nge_assets::{Asset, AssetApp, LoadContext};
use nge_ecs::{App, Plugin, PostUpdate};

/// 解码好的一段声音（PCM 都在内存里）。
#[derive(Clone)]
pub struct Sound(pub(crate) StaticSoundData);

impl Sound {
    pub fn duration(&self) -> std::time::Duration {
        self.0.duration()
    }
}

impl Asset for Sound {
    fn decode(bytes: Vec<u8>, _ctx: &LoadContext) -> Result<Self, String> {
        StaticSoundData::from_cursor(Cursor::new(bytes)).map(Sound).map_err(|e| format!("解码声音失败: {e}"))
    }
}

/// 注册 [`Sound`] 资源和 [`Audio`]。要在 `AssetPlugin` 之后（没有时自己补一个默认的）。
#[derive(Default)]
pub struct AudioPlugin {
    mock: bool,
    /// 测试用：像浏览器那样等解锁。
    #[cfg(test)]
    locked: bool,
}

impl AudioPlugin {
    /// 不出声的后端（测试用），可以数正在播的声音。
    pub fn mock() -> Self {
        Self { mock: true, ..Self::default() }
    }

    #[cfg(test)]
    pub(crate) fn locked_mock() -> Self {
        Self { mock: true, locked: true }
    }

    fn locked(&self) -> bool {
        #[cfg(test)]
        if self.locked {
            return true;
        }
        cfg!(target_arch = "wasm32") && !self.mock
    }
}

/// [`unlock`] 建好、等 [`Audio`] 下一帧接过去的音频设备。
static UNLOCKED: Mutex<Option<Manager>> = Mutex::new(None);
/// 接上了解锁来的设备、还活着的 [`Audio`] 有几个：都没了（游戏视图重建）时下一次手势再建。
pub(crate) static LIVE: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn take_unlocked() -> Option<Manager> {
    UNLOCKED.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// 没有活着的、也没有等着接的设备时用 `make` 建一个。
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn unlock_with(make: impl FnOnce() -> Option<Manager>) {
    if LIVE.load(Ordering::SeqCst) > 0 {
        return;
    }
    let mut slot = UNLOCKED.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot = make();
    }
}

/// 浏览器：在用户按键、点按的事件回调里（同步）调用，建音频设备（WebAudio 只能在这时候开始出声）。
/// 已经有设备时什么都不做；游戏视图重建（旧的 [`Audio`] 没了）后的下一次手势再建。
/// 设备建好以后浏览器把它挂起了（切走标签页、系统打断）由平台层恢复。
#[cfg(target_arch = "wasm32")]
pub fn unlock() {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    unlock_with(|| match AudioManager::<DefaultBackend>::new(AudioManagerSettings::default()) {
        Ok(m) => Some(Manager::Device(m)),
        Err(e) => {
            if !WARNED.swap(true, Ordering::Relaxed) {
                log::warn!("没有可用的音频设备，声音关掉: {e}");
            }
            None
        }
    });
}

#[cfg(test)]
pub(crate) fn mock_manager() -> Manager {
    Manager::Mock(AudioManager::<MockBackend>::new(AudioManagerSettings::default()).expect("mock 后端"))
}

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        nge_assets::ensure_plugin(app);
        // 画面没了、App 进了后台（平台层发 `nge.suspend`）：停下声音，回来接着放
        app.on_suspend(|world, suspended| {
            if let Some(mut audio) = world.get_resource_mut::<Audio>() {
                audio.set_suspended(suspended);
            }
        });
        if self.locked() {
            app.init_asset::<Sound>()
                .insert_resource(Audio::locked())
                .add_systems(PostUpdate, audio::update.in_set(AudioUpdated));
            return;
        }
        let manager = if self.mock {
            AudioManager::<MockBackend>::new(AudioManagerSettings::default()).map(Manager::Mock).map_err(|()| "mock 后端创建失败".to_owned())
        } else {
            AudioManager::<DefaultBackend>::new(AudioManagerSettings::default()).map(Manager::Device).map_err(|e| e.to_string())
        };
        let manager = manager.inspect_err(|e| log::warn!("没有可用的音频设备，声音关掉: {e}")).ok();
        let mut audio = Audio::new(manager.map(Mutex::new));
        audio.add_buses();
        app.init_asset::<Sound>()
            .insert_resource(audio)
            .add_systems(PostUpdate, audio::update.in_set(AudioUpdated));
    }
}

/// 音频系统（PostUpdate）：开始排队的声音、清理播完的。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct AudioUpdated;

pub(crate) enum Manager {
    Device(AudioManager<DefaultBackend>),
    Mock(AudioManager<MockBackend>),
}

pub mod prelude {
    pub use crate::{Audio, AudioPlugin, Bus, EffectSpec, Emit, Play, Rolloff, Sound, Voice};
}

/// 测试用的工具。
#[doc(hidden)]
pub mod testing {
    /// `seconds` 秒的 16 位单声道 wav（采样率 8000，440 Hz 正弦）。
    pub fn wav(seconds: f32) -> Vec<u8> {
        let rate = 8000u32;
        let samples: Vec<i16> = (0..(rate as f32 * seconds) as u32)
            .map(|i| ((i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin() * 8000.0) as i16)
            .collect();
        let data = (samples.len() * 2) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // 单声道
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data.to_le_bytes());
        for s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_3d;
#[cfg(test)]
mod tests_dsp;
#[cfg(test)]
mod tests_spatial;
