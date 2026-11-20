//! 3D 的声音、多普勒、遮挡、效果器链接在 `Audio` 上（不出声的后端，见 docs/audio-3d.md）。

use glam::{Vec2, Vec3};

use crate::chain::chain;
use crate::effects::EffectSpec;
use crate::spatial3d::{Emit, OCCLUDED_CUTOFF, OCCLUDED_GAIN, OPEN_CUTOFF, Rolloff};
use crate::tests::{BEEP, VIEWPORT, app, audio, load, settle};
use crate::voice3d::{MAX_OCCLUSION_TRACKS, OCCLUSION_BUDGET, OCCLUSION_FADE, OCCLUSION_INTERVAL};
use crate::{Audio, Bus, Play};

const LINEAR: Emit = Emit { rolloff: Rolloff::Linear, min: 100.0, max: 300.0, factor: 1.0, cone: None, forward: Vec3::NEG_Z, doppler: 0.0, occlude: false };

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn voices_in_3d_follow_the_listener_and_which_way_it_faces() {
    let mut app = app();
    let beep = load(&mut app, BEEP);
    let left = audio(&mut app).play(beep, Play { at3: Some(Vec3::new(-200.0, 0.0, 0.0)), emit: LINEAR, volume: 0.8, ..Play::looped() });
    let queued = audio(&mut app).play(beep, Play { at3: Some(Vec3::ZERO), emit: LINEAR, ..Play::looped() });
    let flat = audio(&mut app).play(beep, Play { at: Some(Vec2::new(-500.0, 0.0)), ..Play::looped() });
    // 两个都写了：按 3D 的
    let both = audio(&mut app).play(beep, Play { at: Some(Vec2::new(900.0, 0.0)), at3: Some(Vec3::new(0.0, 0.0, -150.0)), emit: LINEAR, ..Play::looped() });
    audio(&mut app).set_voice_position3(queued, Vec3::new(0.0, 250.0, 0.0));
    settle(&mut app);
    let a = audio(&mut app);
    let (v, p) = a.mix(left).unwrap();
    assert!(near(v, 0.4) && near(p, -1.0), "左边 200：线性衰减一半，全偏左 {v} {p}");
    let (v, p) = a.mix(queued).unwrap();
    assert!(near(v, 0.25) && near(p, 0.0), "排队时挪到了头顶 250 {v} {p}");
    assert_eq!(a.mix(flat), Some((0.5, -1.0)), "2D 的照旧按 range 算，不看 3D 的听者");
    let (v, p) = a.mix(both).unwrap();
    assert!(near(v, 0.75) && near(p, 0.0), "{v} {p}");

    // 听的人走过去、转过身（右手方向成了 -x）：左边的声音到了右边
    audio(&mut app).set_listener3(Vec3::new(-50.0, 0.0, 0.0), Vec3::NEG_X);
    audio(&mut app).set_listener3(Vec3::NAN, Vec3::X);
    audio(&mut app).set_listener3(Vec3::ZERO, Vec3::ZERO);
    assert_eq!(audio(&mut app).listener3(), (Vec3::new(-50.0, 0.0, 0.0), Vec3::NEG_X), "不是有限数的、没有方向的不要");
    app.tick(VIEWPORT, 0.016);
    let (v, p) = audio(&mut app).mix(left).unwrap();
    assert!(near(v, 0.8 * 0.75) && near(p, 1.0), "{v} {p}");
    assert_eq!(audio(&mut app).mix(flat), Some((0.5, -1.0)));

    // 挪声音；2D 的声音不听 3D 的挪法，反过来也是
    audio(&mut app).set_voice_position3(left, Vec3::new(-50.0, 0.0, -200.0));
    audio(&mut app).set_voice_position3(flat, Vec3::ZERO);
    audio(&mut app).set_voice_position(left, Vec2::ZERO);
    app.tick(VIEWPORT, 0.016);
    let (v, p) = audio(&mut app).mix(left).unwrap();
    assert!(near(v, 0.4) && near(p, 0.0), "{v} {p}");
    assert_eq!(audio(&mut app).mix(flat), Some((0.5, -1.0)));
    audio(&mut app).set_voice_volume(left, 0.2, 0.0);
    assert!(near(audio(&mut app).mix(left).unwrap().0, 0.1), "改音量也按位置算");

    // 锥形跟着朝向变
    let cone = crate::spatial3d::Cone { inner: 1.0, outer: 1.0, outer_gain: 0.25 };
    audio(&mut app).set_voice_emit(left, Emit { cone: Some(cone), forward: Vec3::Z, ..LINEAR });
    app.tick(VIEWPORT, 0.016);
    assert!(near(audio(&mut app).mix(left).unwrap().0, 0.1), "朝着听的人");
    audio(&mut app).set_voice_forward(left, Vec3::NEG_Z);
    app.tick(VIEWPORT, 0.016);
    assert!(near(audio(&mut app).mix(left).unwrap().0, 0.025), "背过去");
}

#[test]
fn doppler_bends_the_pitch_and_teleports_do_not() {
    let mut app = app();
    let beep = load(&mut app, BEEP);
    let emit = Emit { doppler: 20.0, max: 1e6, ..Emit::default() };
    let moving = audio(&mut app).play(beep, Play { at3: Some(Vec3::new(5000.0, 0.0, 0.0)), emit, pitch: 1.5, ..Play::looped() });
    let off = audio(&mut app).play(beep, Play { at3: Some(Vec3::new(5000.0, 0.0, 0.0)), emit: Emit { doppler: 0.0, ..emit }, pitch: 1.5, ..Play::looped() });
    let flat = audio(&mut app).play(beep, Play { at: Some(Vec2::new(500.0, 0.0)), emit, ..Play::looped() });
    settle(&mut app);
    assert_eq!(audio(&mut app).rate(moving), Some(1.5), "刚开始没有速度");
    // 每秒 300 朝听的人飞过来（夸张 20 倍 = 6000，声速 34300）
    let mut x = 5000.0;
    for _ in 0..60 {
        x -= 5.0;
        audio(&mut app).set_voice_position3(moving, Vec3::new(x, 0.0, 0.0));
        audio(&mut app).set_voice_position3(off, Vec3::new(x, 0.0, 0.0));
        app.tick(VIEWPORT, 1.0 / 60.0);
    }
    let want = 1.5 * 34300.0 / (34300.0 - 6000.0);
    let rate = audio(&mut app).rate(moving).unwrap();
    assert!((rate - want).abs() < 0.01, "靠近时变高 {rate} vs {want}");
    assert_eq!(audio(&mut app).rate(off), Some(1.5), "系数 0 不变调");
    // 停下来：回到原来的音高
    for _ in 0..60 {
        app.tick(VIEWPORT, 1.0 / 60.0);
    }
    assert!((audio(&mut app).rate(moving).unwrap() - 1.5).abs() < 0.01);
    // 瞬移：一帧挪了 3000（每秒 18 万），不当成速度
    audio(&mut app).set_voice_position3(moving, Vec3::new(x - 3000.0, 0.0, 0.0));
    app.tick(VIEWPORT, 1.0 / 60.0);
    assert!((audio(&mut app).rate(moving).unwrap() - 1.5).abs() < 0.01, "{:?}", audio(&mut app).rate(moving));
    // 听的人动也算，离开时变低；暂停（dt 0）时不算
    let mut lx = 0.0;
    for _ in 0..60 {
        lx -= 5.0;
        audio(&mut app).set_listener3(Vec3::new(lx, 0.0, 0.0), Vec3::X);
        app.tick(VIEWPORT, 1.0 / 60.0);
    }
    let rate = audio(&mut app).rate(moving).unwrap();
    assert!((rate - 1.5 * (34300.0 - 6000.0) / 34300.0).abs() < 0.01, "{rate}");
    audio(&mut app).set_listener3(Vec3::new(lx - 50.0, 0.0, 0.0), Vec3::X);
    app.tick(VIEWPORT, 0.0);
    assert!((audio(&mut app).rate(moving).unwrap() - rate).abs() < 1e-6, "暂停的这一帧不重算速度");
    // 声速能改；2D 的声音用 2D 的听者
    audio(&mut app).set_speed_of_sound(-1.0);
    assert_eq!(audio(&mut app).speed_of_sound(), crate::voice3d::SPEED_OF_SOUND);
    audio(&mut app).set_speed_of_sound(12000.0);
    let mut x = 500.0;
    for _ in 0..60 {
        x -= 2.0;
        audio(&mut app).set_voice_position(flat, Vec2::new(x, 0.0));
        app.tick(VIEWPORT, 1.0 / 60.0);
    }
    let rate = audio(&mut app).rate(flat).unwrap();
    assert!((rate - 12000.0 / (12000.0 - 2400.0)).abs() < 0.01, "2D 的也变调 {rate}");
    // 变调以后也不超出播放速度的范围：音高 6、靠近时再乘 2 也只到 8
    let fast = audio(&mut app).play(beep, Play { at3: Some(Vec3::new(9000.0, 0.0, 0.0)), emit: Emit { doppler: 100.0, ..emit }, pitch: 6.0, ..Play::looped() });
    app.tick(VIEWPORT, 1.0 / 60.0);
    for i in 1..30 {
        audio(&mut app).set_voice_position3(fast, Vec3::new(9000.0 - 100.0 * i as f32, 0.0, 0.0));
        app.tick(VIEWPORT, 1.0 / 60.0);
    }
    assert_eq!(audio(&mut app).rate(fast), Some(8.0));
}

#[test]
fn occlusion_probes_are_throttled_and_the_result_fades_in() {
    let mut app = app();
    let beep = load(&mut app, BEEP);
    let emit = Emit { occlude: true, ..LINEAR };
    let voices: Vec<_> = (0..6).map(|i| audio(&mut app).play(beep, Play { at3: Some(Vec3::new(0.0, 0.0, -110.0 - i as f32)), emit, ..Play::looped() })).collect();
    let flat = audio(&mut app).play(beep, Play { at: Some(Vec2::new(30.0, 40.0)), emit, ..Play::looped() });
    let plain = audio(&mut app).play(beep, Play { at3: Some(Vec3::ZERO), emit: LINEAR, ..Play::looped() });
    let nowhere = audio(&mut app).play(beep, Play { emit, ..Play::looped() });
    assert!(audio(&mut app).occlusion_probes().is_empty(), "还没开始播的不查");
    audio(&mut app).set_listener(Vec2::new(1.0, 2.0));
    audio(&mut app).set_listener3(Vec3::new(0.0, 5.0, 0.0), Vec3::X);
    settle(&mut app);
    let first = audio(&mut app).occlusion_probes();
    assert_eq!(first.len(), OCCLUSION_BUDGET, "一帧最多查这么多");
    assert!(audio(&mut app).occlusion_probes().is_empty(), "同一帧再问：剩下的没到时间（还没过一帧）");
    app.tick(VIEWPORT, 0.016);
    let second = audio(&mut app).occlusion_probes();
    assert_eq!(second.len(), 3, "剩下的三个；查过的没到间隔");
    let mut all: Vec<_> = first.iter().chain(&second).map(|p| p.voice).collect();
    all.sort_by_key(|v| format!("{v:?}"));
    all.dedup();
    assert_eq!(all.len(), 7, "每个开了遮挡的都查到一次");
    assert!(!all.contains(&plain) && !all.contains(&nowhere), "没开遮挡的、不带位置的不查");
    let probe = first.iter().chain(&second).find(|p| p.voice == voices[2]).unwrap();
    assert_eq!((probe.from, probe.to, probe.three_d), (Vec3::new(0.0, 5.0, 0.0), Vec3::new(0.0, 0.0, -112.0), true));
    let probe = first.iter().chain(&second).find(|p| p.voice == flat).unwrap();
    assert_eq!((probe.from, probe.to, probe.three_d), (Vec3::new(1.0, 2.0, 0.0), Vec3::new(30.0, 40.0, 0.0), false), "2D 的从 2D 的听者查");
    // 过了间隔才再查
    let mut frames = 0;
    while audio(&mut app).occlusion_probes().is_empty() {
        app.tick(VIEWPORT, 0.016);
        frames += 1;
        assert!(frames < 20);
    }
    assert!((frames as f32 * 0.016 - OCCLUSION_INTERVAL).abs() < 0.033, "隔了 {frames} 帧");

    // 第一次的结果直接用（墙后面的声音不会先响一下）；之后的平滑过渡
    let (v, hidden) = (voices[0], voices[1]);
    let open = audio(&mut app).mix(v).unwrap().0;
    audio(&mut app).set_occluded(hidden, true);
    app.tick(VIEWPORT, 0.016);
    assert_eq!(audio(&mut app).occlusion(hidden), Some(1.0));
    audio(&mut app).set_occluded(v, false);
    audio(&mut app).set_occluded(v, true);
    app.tick(VIEWPORT, OCCLUSION_FADE / 2.0);
    let a = audio(&mut app);
    assert!(near(a.occlusion(v).unwrap(), 0.5), "{:?}", a.occlusion(v));
    assert!(near(a.mix(v).unwrap().0, open * (1.0 + OCCLUDED_GAIN) / 2.0), "音量跟着压");
    assert!((a.voice_cutoff(v).unwrap() - (OPEN_CUTOFF * OCCLUDED_CUTOFF).sqrt()).abs() < 1.0, "低通跟着关");
    app.tick(VIEWPORT, OCCLUSION_FADE);
    let a = audio(&mut app);
    assert_eq!(a.occlusion(v), Some(1.0));
    assert!(near(a.mix(v).unwrap().0, open * OCCLUDED_GAIN) && (a.voice_cutoff(v).unwrap() - OCCLUDED_CUTOFF).abs() < 1.0);
    audio(&mut app).set_occluded(v, false);
    app.tick(VIEWPORT, 10.0);
    let a = audio(&mut app);
    assert_eq!((a.occlusion(v), a.voice_cutoff(v)), (Some(0.0), Some(OPEN_CUTOFF)));
    assert!(near(a.mix(v).unwrap().0, open));
    assert_eq!(a.occlusion(plain), Some(0.0));
    assert_eq!(a.voice_cutoff(plain), None, "没开遮挡的没有自己的低通");
    audio(&mut app).set_occluded(plain, true);
    app.tick(VIEWPORT, 1.0);
    assert_eq!(audio(&mut app).occlusion(plain), Some(0.0), "没开遮挡的不理");
}

#[test]
fn occluded_voices_get_their_own_lowpass_up_to_a_limit_and_stay_on_their_bus() {
    let mut app = app();
    let beep = load(&mut app, BEEP);
    let emit = Emit { occlude: true, ..LINEAR };
    let voices: Vec<_> = (0..MAX_OCCLUSION_TRACKS + 3).map(|_| audio(&mut app).play(beep, Play { at3: Some(Vec3::ZERO), emit, bus: Bus::Ui, ..Play::looped() })).collect();
    AssetServer::apply(app.world_mut(), true);
    // 一帧里同一个声音最多开始 4 次，循环的不占：一帧全开始
    app.tick(VIEWPORT, 0.016);
    let a = audio(&mut app);
    assert_eq!(voices.iter().filter(|v| a.voice_cutoff(**v).is_some()).count(), MAX_OCCLUSION_TRACKS, "多出来的只压音量");
    assert_eq!(a.voices(), voices.len());
    assert_eq!(Bus::ALL.map(|b| a.bus_sounds(b)), [0, 0, voices.len()], "有自己轨道的也算在分组里");
    // 停掉一个，位置让出来
    let with_track = *voices.iter().find(|v| a.voice_cutoff(**v).is_some()).unwrap();
    audio(&mut app).stop(with_track, 0.0);
    let more = audio(&mut app).play(beep, Play { at3: Some(Vec3::ZERO), emit, ..Play::looped() });
    app.tick(VIEWPORT, 0.016);
    assert!(audio(&mut app).voice_cutoff(more).is_some());
    // 没带位置的不建轨道
    let nowhere = audio(&mut app).play(beep, Play { emit, ..Play::looped() });
    app.tick(VIEWPORT, 0.016);
    assert_eq!(audio(&mut app).voice_cutoff(nowhere), None);
    audio(&mut app).advance_mock(3);
    app.tick(VIEWPORT, 0.016);
}

use nge_assets::AssetServer;

#[test]
fn bus_effect_chains_and_sends_are_checked_and_read_back() {
    use EffectSpec::*;
    let mut app = app();
    let mut a = audio(&mut app);
    assert!(Bus::ALL.iter().all(|b| a.effects(*b).is_empty() && a.send(*b) == 0.0), "默认没有效果器、不往混响总线送");
    assert_eq!(a.send_effects(), &[Reverb { room: 0.7, damping: 0.5, mix: 1.0 }], "混响总线上默认一个全湿的混响");
    a.set_effects(Bus::Sfx, &[Lowpass { cutoff: 5.0, q: f32::NAN }, Delay { time: 0.3, feedback: 9.0, mix: 0.5 }], 0.3);
    assert_eq!(a.effects(Bus::Sfx), &[Lowpass { cutoff: 20.0, q: crate::effects::DEFAULT_Q }, Delay { time: 0.3, feedback: 0.95, mix: 0.5 }]);
    assert!(a.effects(Bus::Music).is_empty(), "别的组不变");
    a.set_effects(Bus::Music, &[Limiter { ceiling: -3.0, release: 0.1 }; 20], 0.0);
    assert_eq!(a.effects(Bus::Music).len(), crate::effects::MAX_EFFECTS);
    a.set_send(Bus::Sfx, 0.4, 0.1);
    a.set_send(Bus::Music, 7.0, 0.0);
    a.set_send(Bus::Ui, f32::NAN, 0.0);
    assert_eq!(Bus::ALL.map(|b| a.send(b)), [1.0, 0.4, 0.0]);
    a.set_send_effects(&[Reverb { room: 5.0, damping: 0.2, mix: 1.0 }, Highpass { cutoff: 300.0, q: 1.0 }], 0.2);
    assert_eq!(a.send_effects(), &[Reverb { room: 1.0, damping: 0.2, mix: 1.0 }, Highpass { cutoff: 300.0, q: 1.0 }]);
    a.set_effects(Bus::Sfx, &[], 0.0);
    assert!(a.effects(Bus::Sfx).is_empty());
    // 原来的低通、混响照旧，和链子互不影响
    a.set_lowpass(Bus::Sfx, Some(700.0), 0.0);
    a.set_reverb(Bus::Sfx, 0.4, 0.0);
    a.set_effects(Bus::Sfx, &[Highpass { cutoff: 100.0, q: 1.0 }], 0.0);
    assert_eq!((a.lowpass(Bus::Sfx), a.reverb(Bus::Sfx)), (Some(700.0), 0.4));
    // 链子挂在轨道上跑起来不出事（不出声的后端采样率是 1）
    let beep = load(&mut app, BEEP);
    audio(&mut app).play(beep, Play::looped());
    settle(&mut app);
    audio(&mut app).advance_mock(5);
    audio(&mut app).set_effects(Bus::Sfx, &[Reverb { room: 0.5, damping: 0.5, mix: 0.5 }], 0.1);
    audio(&mut app).advance_mock(5);
    app.tick(VIEWPORT, 0.016);
    assert_eq!(audio(&mut app).voices(), 1);
}

#[test]
fn without_a_device_the_new_calls_are_no_ops_but_settings_stick() {
    use EffectSpec::*;
    let mut a = Audio::new(None);
    a.add_buses();
    a.set_effects(Bus::Sfx, &[Lowpass { cutoff: 500.0, q: 1.0 }], 0.1);
    a.set_send(Bus::Sfx, 0.5, 0.1);
    a.set_send_effects(&[], 0.0);
    a.set_listener3(Vec3::ONE, Vec3::Z);
    a.set_speed_of_sound(100.0);
    assert_eq!((a.effects(Bus::Sfx).len(), a.send(Bus::Sfx), a.send_effects().len(), a.listener3(), a.speed_of_sound()), (1, 0.5, 0, (Vec3::ONE, Vec3::Z), 100.0));
    assert!(a.occlusion_probes().is_empty());
}

#[test]
fn the_chain_on_a_track_swaps_and_glides_and_hands_old_chains_back() {
    use EffectSpec::*;
    let (mut effect, mut handle) = chain(&[]);
    effect.set_rate(48000);
    assert_eq!(effect.run([0.5, -0.5]), [0.5, -0.5], "空链原样过去");
    // 形状变了：换链，交叉淡变以后是新链的声音，旧链送回来
    handle.set(&[Delay { time: 0.5, feedback: 0.0, mix: 1.0 }], 0.0);
    effect.pump();
    let mut last = [1.0, 1.0];
    for _ in 0..2000 {
        last = effect.run([1.0, 1.0]);
    }
    assert_eq!(last, [0.0, 0.0], "全湿的延迟：半秒以内没声");
    effect.pump();
    assert_eq!(handle.retired(), 1, "旧链在队列里等游戏线程释放");
    handle.collect();
    assert_eq!(handle.retired(), 0);
    // 形状没变：原地过渡参数，不换链
    handle.set(&[Delay { time: 0.5, feedback: 0.0, mix: 0.0 }], 0.0);
    effect.pump();
    assert_eq!(effect.run([1.0, 1.0]), [1.0, 1.0], "mix 改成 0：干声");
    effect.pump();
    assert_eq!(handle.retired(), 0);
    assert_eq!(handle.specs(), &[Delay { time: 0.5, feedback: 0.0, mix: 0.0 }]);
    // 采样率变了：按新的重建，声明留着
    effect.set_rate(1);
    assert_eq!(effect.run([1.0, 1.0]), [1.0, 1.0]);
    // 把手先设、采样率后到（建轨道之前）：也用得上
    let (mut effect, _handle) = chain(&[Limiter { ceiling: -6.0, release: 0.1 }]);
    effect.set_rate(44100);
    assert!(effect.run([1.0, 1.0])[0] < 0.51);
}
