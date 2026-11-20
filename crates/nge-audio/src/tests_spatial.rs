//! 3D 空间音频的纯函数（见 docs/audio-3d.md）。

use glam::Vec3;

use crate::spatial3d::*;

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn linear_inverse_and_exponential_rolloff() {
    for model in [Rolloff::Linear, Rolloff::Inverse, Rolloff::Exponential] {
        assert_eq!(attenuation(model, 0.0, 100.0, 300.0, 1.0), 1.0, "{model:?} 最近距离以内不衰减");
        assert_eq!(attenuation(model, 100.0, 100.0, 300.0, 1.0), 1.0, "{model:?}");
        assert_eq!(attenuation(model, 300.0, 100.0, 300.0, 1.0), 0.0, "{model:?} 最远距离上是 0");
        assert_eq!(attenuation(model, 5000.0, 100.0, 300.0, 1.0), 0.0, "{model:?}");
        assert_eq!(attenuation(model, f32::NAN, 100.0, 300.0, 1.0), 0.0, "{model:?} 距离不是数");
        let mut last = 1.0;
        for d in 0..400 {
            let g = attenuation(model, d as f32, 100.0, 300.0, 1.5);
            assert!(g <= last + 1e-6 && (0.0..=1.0).contains(&g), "{model:?} 越远越小 {d} {g}");
            last = g;
        }
    }
    assert!(close(attenuation(Rolloff::Linear, 200.0, 100.0, 300.0, 1.0), 0.5));
    assert!(close(attenuation(Rolloff::Linear, 150.0, 100.0, 300.0, 7.0), 0.75), "线性的不看 factor");
    assert!(close(attenuation(Rolloff::Inverse, 200.0, 100.0, 2000.0, 1.0), 0.5));
    assert!(close(attenuation(Rolloff::Inverse, 300.0, 100.0, 2000.0, 1.0), 1.0 / 3.0));
    assert!(close(attenuation(Rolloff::Inverse, 200.0, 100.0, 2000.0, 2.0), 1.0 / 3.0));
    assert!(close(attenuation(Rolloff::Exponential, 200.0, 100.0, 2000.0, 1.0), 0.5));
    assert!(close(attenuation(Rolloff::Exponential, 200.0, 100.0, 2000.0, 2.0), 0.25));
    assert!(close(attenuation(Rolloff::Exponential, 400.0, 100.0, 2000.0, 0.5), 0.5));
}

#[test]
fn inverse_and_exponential_fade_out_over_the_last_tenth() {
    // 最远 1000：900 以内不收，950 收一半，1000 是 0
    assert!(close(attenuation(Rolloff::Inverse, 900.0, 100.0, 1000.0, 1.0), 100.0 / 900.0));
    assert!(close(attenuation(Rolloff::Inverse, 950.0, 100.0, 1000.0, 1.0), 100.0 / 950.0 * 0.5));
    assert!(close(attenuation(Rolloff::Exponential, 950.0, 100.0, 1000.0, 1.0), 100.0 / 950.0 * 0.5));
    assert!(attenuation(Rolloff::Inverse, 999.0, 100.0, 1000.0, 1.0) < 0.002);
}

#[test]
fn bad_rolloff_settings_fall_back_to_defaults() {
    let want = attenuation(Rolloff::Inverse, 400.0, DEFAULT_MIN, DEFAULT_MAX, 1.0);
    assert!(close(want, 0.25));
    for (min, max, factor) in [(0.0, 2000.0, 1.0), (-5.0, 2000.0, 1.0), (f32::NAN, 2000.0, 1.0), (100.0, 100.0, 1.0), (100.0, 50.0, 1.0), (100.0, f32::NAN, 1.0), (100.0, 2000.0, f32::NAN), (100.0, 2000.0, -1.0)] {
        assert!(close(attenuation(Rolloff::Inverse, 400.0, min, max, factor), want), "{min} {max} {factor}");
    }
    let emit = Emit { min: -1.0, max: 0.0, factor: f32::INFINITY, doppler: f32::NAN, ..Emit::default() }.sane();
    assert_eq!((emit.min, emit.max, emit.factor, emit.doppler), (DEFAULT_MIN, DEFAULT_MAX, 1.0, 0.0));
    assert_eq!(Emit { doppler: 1e9, ..Emit::default() }.sane().doppler, MAX_DOPPLER_FACTOR);
    assert_eq!(Emit { min: 50.0, max: 20.0, ..Emit::default() }.sane().max, 1000.0, "最远不比最近大：按最近的 20 倍");
}

#[test]
fn pan_follows_the_listeners_right_hand() {
    let (o, right) = (Vec3::new(5.0, 1.0, -2.0), Vec3::X);
    assert!(close(pan3(o, right, o + Vec3::X * 10.0), 1.0));
    assert!(close(pan3(o, right, o - Vec3::X * 3.0), -1.0));
    assert!(close(pan3(o, right, o - Vec3::Z * 10.0), 0.0), "正前方");
    assert!(close(pan3(o, right, o + Vec3::Y * 10.0), 0.0), "正上方");
    assert!(close(pan3(o, right, o + Vec3::new(10.0, 0.0, -10.0)), std::f32::consts::FRAC_1_SQRT_2), "右前 45°");
    assert_eq!(pan3(o, right, o), 0.0, "重合时居中");
    // 相机往左转了 90°：右手方向成了 -z，原来正前方的声音到了右边
    assert!(close(pan3(o, Vec3::NEG_Z, o - Vec3::Z * 10.0), 1.0));
    assert!(close(pan3(o, Vec3::X * 7.0, o + Vec3::X), 1.0), "右手方向不用归一化");
    for (l, r, s) in [(Vec3::NAN, right, o), (o, Vec3::NAN, o + Vec3::X), (o, Vec3::ZERO, o + Vec3::X), (o, right, Vec3::NAN)] {
        assert_eq!(pan3(l, r, s), 0.0, "坏输入居中");
    }
}

#[test]
fn cone_is_full_inside_and_quiet_outside() {
    let cone = Cone { inner: 60f32.to_radians(), outer: 120f32.to_radians(), outer_gain: 0.2 };
    let at = |degrees: f32| {
        let a = degrees.to_radians();
        Vec3::new(a.sin(), 0.0, -a.cos()) * 50.0
    };
    assert!(close(cone_gain(cone, Vec3::NEG_Z, Vec3::ZERO, at(0.0)), 1.0));
    assert!(close(cone_gain(cone, Vec3::NEG_Z, Vec3::ZERO, at(29.0)), 1.0), "内锥里（半角 30°）");
    assert!(close(cone_gain(cone, Vec3::NEG_Z, Vec3::ZERO, at(45.0)), 0.6), "内外锥正中间");
    assert!(close(cone_gain(cone, Vec3::NEG_Z, Vec3::ZERO, at(61.0)), 0.2));
    assert!(close(cone_gain(cone, Vec3::NEG_Z, Vec3::ZERO, at(180.0)), 0.2), "正后方");
    assert!(close(cone_gain(cone, Vec3::X, Vec3::ONE, Vec3::ONE + Vec3::X * 3.0), 1.0), "朝向和位置都算");
    assert_eq!(cone_gain(cone, Vec3::NEG_Z, Vec3::ZERO, Vec3::ZERO), 1.0, "重合时不衰减");
    assert_eq!(cone_gain(cone, Vec3::ZERO, Vec3::ZERO, at(180.0)), 1.0, "没有朝向时不衰减");
    assert_eq!(cone_gain(cone, Vec3::NAN, Vec3::ZERO, at(180.0)), 1.0);
    // 内外一样大：一刀切，不除以 0
    let hard = Cone { inner: 1.0, outer: 1.0, outer_gain: 0.0 };
    assert_eq!(cone_gain(hard, Vec3::NEG_Z, Vec3::ZERO, at(10.0)), 1.0);
    assert_eq!(cone_gain(hard, Vec3::NEG_Z, Vec3::ZERO, at(40.0)), 0.0);
    let sane = Cone { inner: 9.0, outer: -1.0, outer_gain: 3.0 }.sane();
    assert_eq!((sane.inner, sane.outer, sane.outer_gain), (std::f32::consts::TAU, std::f32::consts::TAU, 1.0));
    let sane = Cone { inner: f32::NAN, outer: f32::NAN, outer_gain: f32::NAN }.sane();
    assert_eq!((sane.inner, sane.outer, sane.outer_gain), (std::f32::consts::TAU, std::f32::consts::TAU, 0.0));
}

#[test]
fn doppler_raises_the_pitch_when_closing_in() {
    const C: f32 = 340.0;
    let (source, listener) = (Vec3::ZERO, Vec3::X * 100.0);
    let d = |sv: Vec3, lv: Vec3, factor: f32| doppler(sv, lv, source, listener, C, factor);
    assert_eq!(d(Vec3::ZERO, Vec3::ZERO, 1.0), 1.0);
    assert!(close(d(Vec3::X * 34.0, Vec3::ZERO, 1.0), 340.0 / 306.0), "声源靠近");
    assert!(close(d(Vec3::X * -34.0, Vec3::ZERO, 1.0), 340.0 / 374.0), "声源远离");
    assert!(close(d(Vec3::ZERO, Vec3::X * -34.0, 1.0), 374.0 / 340.0), "听的人靠近");
    assert!(close(d(Vec3::ZERO, Vec3::X * 34.0, 1.0), 306.0 / 340.0), "听的人远离");
    assert!(close(d(Vec3::X * 34.0, Vec3::X * 34.0, 1.0), 1.0), "一起走不变调");
    assert!(close(d(Vec3::new(0.0, 80.0, 30.0), Vec3::Z * -50.0, 1.0), 1.0), "横着走不变调");
    assert_eq!(d(Vec3::X * 34.0, Vec3::ZERO, 0.0), 1.0, "系数 0 是关");
    assert!(close(d(Vec3::X * 34.0, Vec3::ZERO, 2.0), 340.0 / 272.0), "系数放大速度");
    assert_eq!(d(Vec3::X * 5000.0, Vec3::ZERO, 1.0), MAX_DOPPLER, "有上限");
    assert_eq!(d(Vec3::ZERO, Vec3::X * 5000.0, 1.0), MIN_DOPPLER, "有下限");
    assert!(close(d(Vec3::X * -5000.0, Vec3::ZERO, 1.0), 1.0 / 1.9), "速度分量截到 0.9 倍声速");
    assert_eq!(doppler(Vec3::X, Vec3::ZERO, source, source, C, 1.0), 1.0, "重合时不变调");
    for c in [0.0, -1.0, f32::NAN] {
        assert_eq!(doppler(Vec3::X * 34.0, Vec3::ZERO, source, listener, c, 1.0), 1.0, "声速不合法 {c}");
    }
    assert_eq!(d(Vec3::NAN, Vec3::ZERO, 1.0), 1.0);
    assert_eq!(d(Vec3::X, Vec3::ZERO, f32::NAN), 1.0);
}

#[test]
fn velocity_from_two_frames_and_teleports_do_not_count() {
    assert_eq!(velocity(Vec3::ZERO, Vec3::new(1.0, 0.0, -2.0), 0.1, 1000.0), Vec3::new(10.0, 0.0, -20.0));
    assert_eq!(velocity(Vec3::ZERO, Vec3::X * 99.0, 0.1, 1000.0), Vec3::X * 990.0);
    assert_eq!(velocity(Vec3::ZERO, Vec3::X * 101.0, 0.1, 1000.0), Vec3::ZERO, "比上限快：瞬移");
    assert_eq!(velocity(Vec3::ZERO, Vec3::X, 0.0, 1000.0), Vec3::ZERO, "暂停");
    assert_eq!(velocity(Vec3::ZERO, Vec3::X, -1.0, 1000.0), Vec3::ZERO);
    assert_eq!(velocity(Vec3::ZERO, Vec3::X, f32::NAN, 1000.0), Vec3::ZERO);
    assert_eq!(velocity(Vec3::NAN, Vec3::X, 0.1, 1000.0), Vec3::ZERO);
    // 平滑：0.1 秒的时间常数，走一个时间常数靠近 63%
    let v = smooth(Vec3::ZERO, Vec3::X * 100.0, VELOCITY_SMOOTHING);
    assert!(close(v.x, 100.0 * (1.0 - (-1f32).exp())), "{v}");
    assert_eq!(smooth(Vec3::X, Vec3::X * 100.0, 0.0), Vec3::X, "没过时间不动");
    assert!(close(smooth(Vec3::ZERO, Vec3::X * 100.0, 100.0).x, 100.0));
}

#[test]
fn occlusion_moves_smoothly_and_maps_to_gain_and_cutoff() {
    assert!(close(approach(0.0, 1.0, 0.1, 0.25), 0.4));
    assert!(close(approach(0.9, 1.0, 0.1, 0.25), 1.0), "不冲过头");
    assert!(close(approach(1.0, 0.0, 0.05, 0.25), 0.8));
    assert!(close(approach(0.1, 0.0, 0.1, 0.25), 0.0));
    assert_eq!(approach(0.3, 1.0, 0.1, 0.0), 1.0, "时间是 0：直接到");
    assert_eq!(approach(0.3, 1.0, f32::NAN, 0.25), 0.3);
    assert_eq!(approach(0.3, 1.0, -1.0, 0.25), 0.3);
    assert_eq!(occlusion_mix(0.0), (1.0, OPEN_CUTOFF));
    let (gain, cutoff) = occlusion_mix(1.0);
    assert!(close(gain, OCCLUDED_GAIN) && (cutoff - OCCLUDED_CUTOFF).abs() < 0.5);
    let (gain, cutoff) = occlusion_mix(0.5);
    assert!(close(gain, (1.0 + OCCLUDED_GAIN) / 2.0), "音量线性");
    assert!((cutoff - (OPEN_CUTOFF * OCCLUDED_CUTOFF).sqrt()).abs() < 1.0, "截止频率按对数 {cutoff}");
    assert_eq!(occlusion_mix(-3.0), occlusion_mix(0.0));
    assert_eq!(occlusion_mix(7.0), occlusion_mix(1.0));
    assert_eq!(occlusion_mix(f32::NAN), occlusion_mix(0.0));
}

#[test]
fn probes_are_throttled_stalest_first() {
    // 没到间隔的不查；到了的里面等得最久的先查，一帧最多 budget 个
    assert_eq!(pick_probes(&[0.05, 0.5, 0.2, 0.09, 0.3], 2, 0.1), vec![1, 4]);
    assert_eq!(pick_probes(&[0.05, 0.5, 0.2, 0.09, 0.3], 9, 0.1), vec![1, 4, 2]);
    assert_eq!(pick_probes(&[0.1], 1, 0.1), vec![0], "正好到间隔");
    assert!(pick_probes(&[0.5, 0.5], 0, 0.1).is_empty());
    assert!(pick_probes(&[], 4, 0.1).is_empty());
    assert_eq!(pick_probes(&[f32::NAN, 0.2], 4, 0.1), vec![1], "不是数的不查");
}

#[test]
fn a_3d_voice_mixes_distance_cone_and_pan() {
    let emit = Emit { rolloff: Rolloff::Linear, min: 100.0, max: 300.0, cone: Some(Cone { inner: 1.0, outer: 1.0, outer_gain: 0.5 }), forward: Vec3::NEG_X, ..Emit::default() };
    // 声源在听的人左边 200，背对着听的人：距离 0.5 × 锥外 0.5，声像 -1
    let (gain, pan) = mix3(0.8, 0.0, Vec3::new(-200.0, 0.0, 0.0), &emit, Vec3::ZERO, Vec3::X);
    assert!(close(gain, 0.8 * 0.5 * 0.5) && close(pan, -1.0), "{gain} {pan}");
    let (gain, pan) = mix3(1.0, 0.5, Vec3::new(0.0, 0.0, -150.0), &Emit { cone: None, ..emit }, Vec3::ZERO, Vec3::X);
    assert!(close(gain, 0.75) && close(pan, 0.5), "自己的声像叠上去 {gain} {pan}");
    let (_, pan) = mix3(1.0, 0.5, Vec3::new(150.0, 0.0, 0.0), &emit, Vec3::ZERO, Vec3::X);
    assert_eq!(pan, 1.0, "截到 1");
    assert_eq!(mix3(1.0, f32::NAN, Vec3::NAN, &emit, Vec3::ZERO, Vec3::X), (0.0, 0.0), "位置不是数：静音");
}
