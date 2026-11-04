//! 音频（kira）：按 `asset:/` 加载 [`Sound`]（wav / ogg / mp3），用 [`Audio`] 播放、循环、调音量。
//! 没有音频设备时所有调用都是空操作。

mod audio;

pub use audio::{Audio, MAX_STARTS_PER_FRAME, MAX_VOICES, Play, Voice};

use std::io::Cursor;
use std::sync::Mutex;

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
}

impl AudioPlugin {
    /// 不出声的后端（测试用），可以数正在播的声音。
    pub fn mock() -> Self {
        Self { mock: true }
    }
}

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        nge_assets::ensure_plugin(app);
        let manager = if self.mock {
            AudioManager::<MockBackend>::new(AudioManagerSettings::default()).map(Manager::Mock).map_err(|()| "mock 后端创建失败".to_owned())
        } else {
            AudioManager::<DefaultBackend>::new(AudioManagerSettings::default()).map(Manager::Device).map_err(|e| e.to_string())
        };
        let manager = manager.inspect_err(|e| log::warn!("没有可用的音频设备，声音关掉: {e}")).ok();
        app.init_asset::<Sound>()
            .insert_resource(Audio::new(manager.map(Mutex::new)))
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
    pub use crate::{Audio, AudioPlugin, Play, Sound, Voice};
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
