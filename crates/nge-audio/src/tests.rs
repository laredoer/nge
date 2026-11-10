use std::sync::Arc;

use nge_assets::{AssetPlugin, AssetServer, Assets, Handle, MemorySource};
use nge_core::Viewport;
use nge_ecs::App;

use super::*;
use crate::audio::decibels;
use crate::testing::wav;

const VIEWPORT: Viewport = Viewport { size: [100.0, 100.0], pixels: [100, 100] };
const BEEP: &str = "asset:/@t/beep.wav";
const BOOP: &str = "asset:/@t/boop.wav";
const BAD: &str = "asset:/@t/bad.wav";

fn app() -> App {
    let source = MemorySource::default().with(BEEP, wav(2.5)).with(BOOP, wav(0.1)).with(BAD, b"not a wav".to_vec());
    let mut app = App::new();
    app.add_plugins(AssetPlugin::new().source(Arc::new(source))).add_plugins(AudioPlugin::mock());
    app
}

fn load(app: &mut App, uri: &str) -> Handle<Sound> {
    let world = app.world_mut();
    world.resource_scope(|world, mut server: Mut<AssetServer>| server.load(&mut world.resource_mut::<Assets<Sound>>(), uri))
}

/// 等后台加载完，再走一帧。
fn settle(app: &mut App) {
    AssetServer::apply(app.world_mut(), true);
    app.tick(VIEWPORT, 0.016);
}

fn audio(app: &mut App) -> Mut<'_, Audio> {
    app.world_mut().resource_mut::<Audio>()
}

#[test]
fn decodes_wav_and_rejects_garbage() {
    let mut app = app();
    let (beep, bad) = (load(&mut app, BEEP), load(&mut app, BAD));
    settle(&mut app);
    let sounds = app.world().resource::<Assets<Sound>>();
    assert!((sounds.get(beep).unwrap().duration().as_secs_f32() - 2.5).abs() < 0.01);
    assert!(matches!(sounds.state(bad), nge_assets::LoadState::Failed(_)));
}

#[test]
fn queued_until_loaded_and_failed_ones_are_dropped() {
    let mut app = app();
    let (beep, bad) = (load(&mut app, BEEP), load(&mut app, BAD));
    let (a, b) = (audio(&mut app).play(beep, Play::default()), audio(&mut app).play(bad, Play::default()));
    assert!(audio(&mut app).is_playing(a), "排队中也算");
    settle(&mut app);
    let audio = audio(&mut app);
    assert_eq!(audio.voices(), 1);
    assert!(audio.is_playing(a) && !audio.is_playing(b));
}

#[test]
fn finished_sounds_are_cleaned_up_but_loops_keep_going() {
    let mut app = app();
    let beep = load(&mut app, BEEP);
    settle(&mut app);
    let once = audio(&mut app).play(beep, Play::default());
    let music = audio(&mut app).play(beep, Play::looped());
    app.tick(VIEWPORT, 0.016);
    assert_eq!(audio(&mut app).voices(), 2);
    audio(&mut app).advance_mock(4);
    app.tick(VIEWPORT, 0.016);
    let a = audio(&mut app);
    assert!(!a.is_playing(once), "2.5 秒的声音 4 秒后播完");
    assert!(a.is_playing(music), "循环的一直在");
}

#[test]
fn stop_fades_out_and_forgets_the_voice() {
    let mut app = app();
    let beep = load(&mut app, BEEP);
    let queued = audio(&mut app).play(beep, Play::default());
    audio(&mut app).stop(queued, 0.0);
    settle(&mut app);
    assert_eq!(audio(&mut app).voices(), 0, "还在排队时停掉就不播了");

    let v = audio(&mut app).play(beep, Play::looped());
    app.tick(VIEWPORT, 0.016);
    audio(&mut app).stop(v, 0.5);
    assert!(!audio(&mut app).is_playing(v));
}

#[test]
fn voice_limits() {
    let mut app = app();
    let (beep, boop) = (load(&mut app, BEEP), load(&mut app, BOOP));
    settle(&mut app);
    for _ in 0..10 {
        audio(&mut app).play(boop, Play::default());
    }
    app.tick(VIEWPORT, 0.016);
    assert_eq!(audio(&mut app).voices(), MAX_STARTS_PER_FRAME as usize, "一帧里同一个声音最多开始几次");

    for _ in 0..20 {
        for _ in 0..MAX_STARTS_PER_FRAME {
            audio(&mut app).play(beep, Play::default());
        }
        app.tick(VIEWPORT, 0.016);
    }
    assert_eq!(audio(&mut app).voices(), MAX_VOICES);
    audio(&mut app).play(beep, Play::looped());
    app.tick(VIEWPORT, 0.016);
    assert_eq!(audio(&mut app).voices(), MAX_VOICES + 1, "循环的不受上限限制");
}

#[test]
fn volume_conversion_and_master() {
    assert_eq!(decibels(1.0).0, 0.0);
    assert!((decibels(0.5).0 + 6.02).abs() < 0.01);
    for silent in [0.0, -1.0, f32::NAN, 1e-9] {
        assert_eq!(decibels(silent), kira::Decibels::SILENCE, "{silent}");
    }
    let mut app = app();
    audio(&mut app).set_volume(0.3);
    audio(&mut app).set_muted(true);
    let a = audio(&mut app);
    assert_eq!((a.volume(), a.muted()), (0.3, true));
}

#[test]
fn without_a_device_everything_is_a_no_op() {
    let mut app = App::new();
    app.add_plugins(AssetPlugin::new().source(Arc::new(MemorySource::default().with(BEEP, wav(0.1)))));
    app.insert_resource(Audio::new(None)).init_asset::<Sound>().add_systems(nge_ecs::PostUpdate, audio::update);
    let beep = load(&mut app, BEEP);
    let v = audio(&mut app).play(beep, Play::default());
    assert!(!audio(&mut app).is_playing(v) && !audio(&mut app).enabled(), "没有设备时不排队，也就不去解码");
    settle(&mut app);
    audio(&mut app).set_volume(0.5);
    assert!(!audio(&mut app).is_playing(v));
    assert_eq!(audio(&mut app).voices(), 0);
}

#[test]
fn play_settings_are_clamped() {
    use crate::audio::{clamp_pitch, tween};
    assert_eq!([0.5, 1e30, 1e-30, -2.0, f32::NAN].map(clamp_pitch), [0.5, 8.0, 0.1, 1.0, 1.0]);
    assert_eq!(tween(1e20).duration.as_secs(), 60, "很大的淡出时间不 panic");
    assert_eq!(tween(f32::NAN).duration.as_millis(), 10);
}

#[test]
fn volume_of_a_queued_voice_is_kept_until_it_starts() {
    let mut app = app();
    let beep = load(&mut app, BEEP);
    let v = audio(&mut app).play(beep, Play::looped());
    audio(&mut app).set_voice_volume(v, 0.2, 0.0);
    assert_eq!(audio(&mut app).queued_volume(v), Some(0.2));
    settle(&mut app);
    assert!(audio(&mut app).is_playing(v));
}

#[test]
fn loops_do_not_use_up_the_per_frame_starts() {
    let mut app = app();
    let boop = load(&mut app, BOOP);
    settle(&mut app);
    for _ in 0..MAX_STARTS_PER_FRAME {
        audio(&mut app).play(boop, Play::looped());
    }
    audio(&mut app).play(boop, Play::default());
    app.tick(VIEWPORT, 0.016);
    assert_eq!(audio(&mut app).voices(), MAX_STARTS_PER_FRAME as usize + 1);
}

#[test]
fn spatial_volume_and_pan() {
    use glam::Vec2;
    let l = Vec2::new(100.0, 50.0);
    assert_eq!(spatial(0.8, 0.0, l, l, 1000.0), (0.8, 0.0), "正中");
    let (v, p) = spatial(1.0, 0.0, l + Vec2::new(-250.0, 0.0), l, 1000.0);
    assert!((v - 0.75).abs() < 1e-6 && (p + 0.5).abs() < 1e-6, "左边 250：{v} {p}");
    let (v, p) = spatial(1.0, 0.0, l + Vec2::new(0.0, 500.0), l, 1000.0);
    assert!((v - 0.5).abs() < 1e-6 && p == 0.0, "正上方不偏 {v} {p}");
    assert_eq!(spatial(1.0, 0.0, l + Vec2::new(1500.0, 0.0), l, 1000.0), (0.0, 1.0), "超出范围静音、声像截到 1");
    let (_, p) = spatial(1.0, 0.5, l + Vec2::new(100.0, 0.0), l, 1000.0);
    assert!((p - 0.7).abs() < 1e-6, "叠加自己的声像 {p}");
    let (v, _) = spatial(1.0, 0.0, l + Vec2::new(100.0, 0.0), l, -1.0);
    assert!((v - 0.9).abs() < 1e-6, "范围不合法按 1000 {v}");
    assert_eq!(spatial(1.0, 0.2, Vec2::NAN, l, 1000.0), (0.0, 0.2));
}

#[test]
fn bus_settings_are_clamped_and_read_back() {
    let mut app = app();
    let mut audio = audio(&mut app);
    assert_eq!((audio.bus_volume(Bus::Music), audio.lowpass(Bus::Sfx), audio.reverb(Bus::Ui)), (1.0, None, 0.0), "默认");
    audio.set_bus_volume(Bus::Music, 0.4, 0.1);
    audio.set_bus_volume(Bus::Ui, -2.0, 0.0);
    audio.set_lowpass(Bus::Sfx, Some(800.0), 0.3);
    audio.set_lowpass(Bus::Music, Some(5.0), 0.0);
    audio.set_reverb(Bus::Sfx, 1.5, 0.0);
    audio.set_reverb(Bus::Music, f32::NAN, 0.0);
    assert_eq!(audio.bus_volume(Bus::Music), 0.4);
    assert_eq!(audio.bus_volume(Bus::Ui), 0.0);
    assert_eq!(audio.bus_volume(Bus::Sfx), 1.0, "别的组不变");
    assert_eq!((audio.lowpass(Bus::Sfx), audio.lowpass(Bus::Music)), (Some(800.0), Some(20.0)));
    assert_eq!((audio.reverb(Bus::Sfx), audio.reverb(Bus::Music)), (1.0, 0.0));
    audio.set_lowpass(Bus::Sfx, None, 0.0);
    assert_eq!(audio.lowpass(Bus::Sfx), None);
    audio.set_range(0.0);
    assert_eq!(audio.range(), DEFAULT_RANGE);
    audio.set_range(300.0);
    assert_eq!(audio.range(), 300.0);
}

#[test]
fn positional_voices_follow_the_listener() {
    use glam::Vec2;
    let mut app = app();
    let beep = load(&mut app, BEEP);
    let placed = audio(&mut app).play(beep, Play { at: Some(Vec2::new(-500.0, 0.0)), volume: 0.8, ..Play::looped() });
    let plain = audio(&mut app).play(beep, Play { volume: 0.6, pan: 0.3, bus: Bus::Ui, ..Play::looped() });
    let queued = audio(&mut app).play(beep, Play { at: Some(Vec2::ZERO), ..Play::looped() });
    // 还在排队时挪：开始时按新位置
    audio(&mut app).set_voice_position(queued, Vec2::new(500.0, 0.0));
    settle(&mut app);
    assert_eq!(audio(&mut app).mix(queued), Some((0.5, 1.0)));
    audio(&mut app).stop(queued, 0.0);
    let (v, p) = audio(&mut app).mix(placed).unwrap();
    assert!((v - 0.4).abs() < 1e-6 && (p + 1.0).abs() < 1e-6, "开始时按位置 {v} {p}");
    assert_eq!(audio(&mut app).mix(plain), Some((0.6, 0.3)));
    audio(&mut app).advance_mock(1);
    let audio_ref = app.world().resource::<Audio>();
    assert_eq!(Bus::ALL.map(|b| audio_ref.bus_sounds(b)), [0, 2, 1], "各进各的分组");
    audio(&mut app).set_listener(Vec2::new(-500.0, 0.0));
    app.tick(VIEWPORT, 0.016);
    assert_eq!(audio(&mut app).mix(placed), Some((0.8, 0.0)), "听的人走过去了");
    assert_eq!(audio(&mut app).mix(plain), Some((0.6, 0.3)), "不按位置的不变");
    audio(&mut app).set_voice_position(placed, Vec2::new(-300.0, 0.0));
    audio(&mut app).set_voice_position(plain, Vec2::new(-300.0, 0.0));
    app.tick(VIEWPORT, 0.016);
    let (v, p) = audio(&mut app).mix(placed).unwrap();
    assert!((v - 0.64).abs() < 1e-6 && (p - 0.4).abs() < 1e-6, "声音挪了 {v} {p}");
    assert_eq!(audio(&mut app).mix(plain), Some((0.6, 0.3)), "不按位置的不会变成按位置");
    audio(&mut app).set_voice_volume(placed, 0.4, 0.0);
    let (v, _) = audio(&mut app).mix(placed).unwrap();
    assert!((v - 0.32).abs() < 1e-6, "改音量也按位置算 {v}");
}

#[test]
fn without_a_device_bus_calls_are_no_ops() {
    let mut audio = Audio::new(None);
    audio.add_buses();
    audio.set_bus_volume(Bus::Sfx, 0.5, 0.1);
    audio.set_lowpass(Bus::Sfx, Some(500.0), 0.1);
    audio.set_reverb(Bus::Sfx, 0.5, 0.1);
    audio.set_listener(glam::Vec2::new(1.0, 2.0));
    audio.set_listener(glam::Vec2::NAN);
    assert_eq!(audio.listener(), glam::Vec2::new(1.0, 2.0), "不是有限数的不要");
    assert_eq!((audio.bus_volume(Bus::Sfx), audio.lowpass(Bus::Sfx), audio.reverb(Bus::Sfx)), (0.5, Some(500.0), 0.5));
}

#[test]
fn moving_does_not_cut_a_long_fade_short_and_nan_volumes_are_silent() {
    use glam::Vec2;
    let mut app = app();
    let beep = load(&mut app, BEEP);
    let v = audio(&mut app).play(beep, Play { at: Some(Vec2::ZERO), ..Play::looped() });
    let bad = audio(&mut app).play(beep, Play { volume: f32::NAN, at: Some(Vec2::ZERO), ..Play::looped() });
    settle(&mut app);
    assert_eq!(audio(&mut app).mix(bad), Some((0.0, 0.0)), "NaN 音量当 0");
    audio(&mut app).set_voice_volume(v, 0.0, 2.0);
    audio(&mut app).set_listener(Vec2::new(100.0, 0.0));
    app.tick(VIEWPORT, 0.016);
    let fade = audio(&mut app).last_fade(v).unwrap();
    assert!(fade > 1.5, "还剩将近 2 秒的淡出 {fade}");
    assert_eq!(spatial(1.0, f32::NAN, Vec2::ZERO, Vec2::ZERO, 100.0), (1.0, 0.0));
}

#[test]
fn locked_until_unlocked_then_music_starts_and_settings_carry_over() {
    let source = MemorySource::default().with(BEEP, wav(2.5)).with(BOOP, wav(0.1));
    let mut app = App::new();
    app.add_plugins(AssetPlugin::new().source(Arc::new(source))).add_plugins(AudioPlugin::locked_mock());
    let (beep, boop) = (load(&mut app, BEEP), load(&mut app, BOOP));
    let music = audio(&mut app).play(beep, Play { bus: Bus::Music, ..Play::looped() });
    let hit = audio(&mut app).play(boop, Play::default());
    audio(&mut app).set_bus_volume(Bus::Music, 0.5, 0.0);
    assert!(!audio(&mut app).enabled());
    assert!(audio(&mut app).is_playing(music), "音乐排着等解锁");
    assert!(!audio(&mut app).is_playing(hit), "解锁前的音效直接丢掉");
    settle(&mut app);
    assert_eq!(audio(&mut app).voices(), 0, "没解锁不出声");
    assert!(audio(&mut app).is_playing(music));

    crate::hand_over_mock();
    settle(&mut app);
    let audio = audio(&mut app);
    assert!(audio.enabled());
    assert!(audio.mix(music).is_some(), "解锁后音乐开始");
    assert_eq!(audio.bus_volume(Bus::Music), 0.5);
    assert_eq!(audio.bus_sounds(Bus::Music), 1, "进了音乐分组的轨道");
}
