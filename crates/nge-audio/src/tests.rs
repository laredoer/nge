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
