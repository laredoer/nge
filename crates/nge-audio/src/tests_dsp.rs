//! 效果器的 DSP：冲激响应、频响、参数校验、参数过渡、换链（见 docs/audio-3d.md）。

use crate::effects::*;

const RATE: u32 = 48000;

/// 喂一段正弦，量稳定以后输出的幅度（均方根 × √2）。
fn gain_at(chain: &mut Chain, freq: f32, amplitude: f32) -> f32 {
    let n = RATE as usize;
    let mut sum = 0.0f64;
    for i in 0..n {
        let x = (i as f32 / RATE as f32 * freq * std::f32::consts::TAU).sin() * amplitude;
        let [l, r] = chain.process([x, x]);
        assert!(l.is_finite() && r.is_finite());
        if i >= n / 2 {
            sum += f64::from(l) * f64::from(l);
        }
    }
    ((sum / (n / 2) as f64).sqrt() * std::f64::consts::SQRT_2) as f32
}

fn impulse(chain: &mut Chain, n: usize) -> Vec<[f32; 2]> {
    (0..n).map(|i| chain.process(if i == 0 { [1.0, 1.0] } else { [0.0, 0.0] })).collect()
}

fn one(spec: EffectSpec) -> Chain {
    Chain::new(&[spec], RATE)
}

#[test]
fn lowpass_and_highpass_frequency_response() {
    let butter = std::f32::consts::FRAC_1_SQRT_2;
    let mut low = one(EffectSpec::Lowpass { cutoff: 1000.0, q: butter });
    assert!((gain_at(&mut low, 100.0, 1.0) - 1.0).abs() < 0.01, "通带不变");
    assert!((gain_at(&mut low, 1000.0, 1.0) - butter).abs() < 0.01, "截止频率上 -3 dB");
    assert!((gain_at(&mut low, 4000.0, 1.0) - 0.0624).abs() < 0.01, "两个倍频程 -24 dB");
    assert!(gain_at(&mut low, 16000.0, 1.0) < 0.004);
    let mut high = one(EffectSpec::Highpass { cutoff: 1000.0, q: butter });
    assert!((gain_at(&mut high, 10000.0, 1.0) - 1.0).abs() < 0.02);
    assert!((gain_at(&mut high, 1000.0, 1.0) - butter).abs() < 0.01);
    assert!((gain_at(&mut high, 250.0, 1.0) - 0.0624).abs() < 0.01);
    // q 大了在截止频率上鼓起来
    let mut peaky = one(EffectSpec::Lowpass { cutoff: 1000.0, q: 4.0 });
    assert!((gain_at(&mut peaky, 1000.0, 0.1) / 0.1 - 4.0).abs() < 0.2);
}

#[test]
fn filter_impulse_responses_sum_to_their_dc_gain() {
    let low: f32 = impulse(&mut one(EffectSpec::Lowpass { cutoff: 2000.0, q: 0.707 }), 4000).iter().map(|f| f[0]).sum();
    assert!((low - 1.0).abs() < 1e-3, "低通直流增益 1：{low}");
    let high: f32 = impulse(&mut one(EffectSpec::Highpass { cutoff: 2000.0, q: 0.707 }), 4000).iter().map(|f| f[0]).sum();
    assert!(high.abs() < 1e-3, "高通挡掉直流：{high}");
    let out = impulse(&mut one(EffectSpec::Lowpass { cutoff: 2000.0, q: 0.707 }), 4000);
    assert!(out[3999][0].abs() < 1e-6, "稳定，响应收得回去");
    assert!(out.iter().all(|f| f[0] == f[1]), "左右一样");
}

#[test]
fn delay_echoes_with_feedback() {
    // 1000 Hz 的采样率，延迟 10 毫秒 = 10 个采样
    let mut delay = Chain::new(&[EffectSpec::Delay { time: 0.01, feedback: 0.5, mix: 0.5 }], 1000);
    let out = impulse(&mut delay, 45);
    for (i, f) in out.iter().enumerate() {
        let want = match i {
            0 => 0.5,
            10 => 0.5,
            20 => 0.25,
            30 => 0.125,
            40 => 0.0625,
            _ => 0.0,
        };
        assert!((f[0] - want).abs() < 1e-5 && f[0] == f[1], "第 {i} 个采样 {f:?}，应该是 {want}");
    }
    let mut dry = Chain::new(&[EffectSpec::Delay { time: 0.01, feedback: 0.9, mix: 0.0 }], 1000);
    assert!(impulse(&mut dry, 30).iter().skip(1).all(|f| *f == [0.0, 0.0]), "mix 0 没有回声");
}

#[test]
fn reverb_has_a_tail_that_decays() {
    let room = |room: f32| EffectSpec::Reverb { room, damping: 0.5, mix: 1.0 };
    let energy = |out: &[[f32; 2]]| out.iter().map(|f| f64::from(f[0] * f[0] + f[1] * f[1])).sum::<f64>();
    let n = 44100 * 2;
    let out = impulse(&mut Chain::new(&[room(0.5)], 44100), n);
    assert!(out[..1000].iter().all(|f| *f == [0.0, 0.0]), "全湿：最短的梳状滤波（1116 个采样）之前没有声音");
    assert!(energy(&out[1000..3000]) > 0.0);
    assert!(out.iter().any(|f| (f[0] - f[1]).abs() > 1e-6), "左右错开");
    let (early, late) = (energy(&out[..n / 2]), energy(&out[n / 2..]));
    assert!(late < early * 0.05, "尾巴在衰减 {early} {late}");
    let big = impulse(&mut Chain::new(&[room(1.0)], 44100), n);
    assert!(energy(&big[n / 2..]) > late * 10.0, "房间越大尾巴越长");
    assert!(big.iter().all(|f| f[0].abs() < 1.0 && f[1].abs() < 1.0), "不发散");
    // 干湿：mix 0 原样过去
    let mut dry = Chain::new(&[EffectSpec::Reverb { room: 1.0, damping: 0.0, mix: 0.0 }], 44100);
    assert!(impulse(&mut dry, 5000).iter().enumerate().all(|(i, f)| *f == if i == 0 { [1.0, 1.0] } else { [0.0, 0.0] }));
    // 最满的参数喂满幅的方波也不炸
    let mut loud = Chain::new(&[EffectSpec::Reverb { room: 1.0, damping: 0.0, mix: 1.0 }], 44100);
    let peak = (0..n).map(|i| loud.process(if i / 50 % 2 == 0 { [1.0, 1.0] } else { [-1.0, -1.0] })[0].abs()).fold(0.0f32, f32::max);
    assert!(peak.is_finite() && peak < 20.0, "{peak}");
}

#[test]
fn compressor_squeezes_what_is_over_the_threshold() {
    let comp = |ratio: f32, makeup: f32| one(EffectSpec::Compressor { threshold: -20.0, ratio, attack: 0.001, release: 0.5, makeup });
    let db = |x: f32| 20.0 * x.log10();
    // 0 dB 的正弦，阈值 -20、4:1：超出的 20 dB 压成 5 dB
    let loud = db(gain_at(&mut comp(4.0, 0.0), 1000.0, 1.0));
    assert!((loud + 15.0).abs() < 1.0, "{loud} dB");
    let quiet = gain_at(&mut comp(4.0, 0.0), 1000.0, 0.05);
    assert!((quiet - 0.05).abs() < 1e-4, "没过阈值的不动 {quiet}");
    assert!((gain_at(&mut comp(1.0, 0.0), 1000.0, 1.0) - 1.0).abs() < 1e-3, "1:1 不压");
    assert!((gain_at(&mut comp(4.0, 6.0), 1000.0, 0.05) - 0.05 * 1.9953).abs() < 1e-3, "补偿增益");
    let harder = db(gain_at(&mut comp(20.0, 0.0), 1000.0, 1.0));
    assert!((harder + 19.0).abs() < 1.0, "20:1 {harder} dB");
}

#[test]
fn limiter_never_lets_anything_over_the_ceiling() {
    let ceiling = 10f32.powf(-6.0 / 20.0);
    let mut limiter = one(EffectSpec::Limiter { ceiling: -6.0, release: 0.05 });
    let mut peak = 0.0f32;
    for i in 0..RATE as usize {
        let x = (i as f32 / RATE as f32 * 440.0 * std::f32::consts::TAU).sin() * if i < 24000 { 1.5 } else { 0.1 };
        let [l, r] = limiter.process([x, -x * 0.5]);
        peak = peak.max(l.abs()).max(r.abs());
        if i > 24000 + 4800 * 3 {
            assert!((l - x).abs() < 1e-3, "响的过去以后放开，小声的原样 {i}: {l} vs {x}");
        }
    }
    assert!(peak <= ceiling + 1e-6 && peak > ceiling * 0.98, "{peak} vs {ceiling}");
    // 一上来就是满幅：第一个采样也不超
    let mut limiter = one(EffectSpec::Limiter { ceiling: -6.0, release: 0.05 });
    assert!(limiter.process([4.0, -4.0])[0] <= ceiling + 1e-6);
    let mut quiet = one(EffectSpec::Limiter { ceiling: -6.0, release: 0.05 });
    assert_eq!(quiet.process([0.3, -0.2]), [0.3, -0.2], "没到上限的原样");
}

#[test]
fn settings_are_checked() {
    use EffectSpec::*;
    let sane = |s: EffectSpec| s.sanitize();
    assert_eq!(sane(Lowpass { cutoff: 5.0, q: 0.0 }), Lowpass { cutoff: 20.0, q: 0.1 });
    assert_eq!(sane(Lowpass { cutoff: 1e9, q: 99.0 }), Lowpass { cutoff: 20000.0, q: 10.0 });
    assert_eq!(sane(Lowpass { cutoff: f32::NAN, q: f32::NAN }), Lowpass { cutoff: 20000.0, q: DEFAULT_Q }, "不是数：低通开到头");
    assert_eq!(sane(Highpass { cutoff: f32::NAN, q: 1.0 }), Highpass { cutoff: 20.0, q: 1.0 }, "不是数：高通关到底");
    assert_eq!(sane(Reverb { room: 2.0, damping: -1.0, mix: f32::NAN }), Reverb { room: 1.0, damping: 0.0, mix: 0.3 });
    assert_eq!(sane(Delay { time: 0.0, feedback: 3.0, mix: 2.0 }), Delay { time: 0.001, feedback: 0.95, mix: 1.0 });
    assert_eq!(sane(Delay { time: 60.0, feedback: -1.0, mix: -1.0 }), Delay { time: 2.0, feedback: 0.0, mix: 0.0 });
    assert_eq!(sane(Delay { time: f32::NAN, feedback: f32::INFINITY, mix: f32::NAN }), Delay { time: 0.25, feedback: 0.35, mix: 0.3 });
    assert_eq!(
        sane(Compressor { threshold: 10.0, ratio: 0.5, attack: 0.0, release: 100.0, makeup: 99.0 }),
        Compressor { threshold: 0.0, ratio: 1.0, attack: 0.0001, release: 5.0, makeup: 24.0 }
    );
    assert_eq!(
        sane(Compressor { threshold: -200.0, ratio: 1e6, attack: 9.0, release: 0.0, makeup: -99.0 }),
        Compressor { threshold: -60.0, ratio: 20.0, attack: 1.0, release: 0.001, makeup: -24.0 }
    );
    assert_eq!(
        sane(Compressor { threshold: f32::NAN, ratio: f32::NAN, attack: f32::NAN, release: f32::NAN, makeup: f32::NAN }),
        Compressor { threshold: -18.0, ratio: 4.0, attack: 0.01, release: 0.15, makeup: 0.0 }
    );
    assert_eq!(sane(Limiter { ceiling: 6.0, release: 0.0 }), Limiter { ceiling: 0.0, release: 0.001 });
    assert_eq!(sane(Limiter { ceiling: -99.0, release: f32::NAN }), Limiter { ceiling: -24.0, release: 0.1 });
    // 合法的不动
    let ok = Delay { time: 0.3, feedback: 0.4, mix: 0.5 };
    assert_eq!(sane(ok), ok);
    // 一条链最多 8 个
    let long = vec![Lowpass { cutoff: 5.0, q: 1.0 }; 12];
    let chain = sanitize_chain(&long);
    assert_eq!(chain.len(), MAX_EFFECTS);
    assert_eq!(chain[0], Lowpass { cutoff: 20.0, q: 1.0 });
    assert!(same_shape(&[ok, Lowpass { cutoff: 1.0, q: 1.0 }], &[Delay { time: 1.0, feedback: 0.0, mix: 0.0 }, Lowpass { cutoff: 9.0, q: 2.0 }]));
    assert!(!same_shape(&[ok], &[ok, ok]));
    assert!(!same_shape(&[Lowpass { cutoff: 1.0, q: 1.0 }], &[Highpass { cutoff: 1.0, q: 1.0 }]));
    assert!(same_shape(&[], &[]));
}

#[test]
fn an_empty_chain_passes_audio_through_and_effects_run_in_order() {
    let mut empty = Chain::new(&[], RATE);
    assert_eq!(empty.process([0.25, -0.5]), [0.25, -0.5]);
    // 先限幅到 -6 dB 再补 6 dB，和反过来不一样：顺序是声明的顺序
    let limit = EffectSpec::Limiter { ceiling: -6.0, release: 0.05 };
    let boost = EffectSpec::Compressor { threshold: 0.0, ratio: 1.0, attack: 0.01, release: 0.1, makeup: 6.0 };
    let a = gain_at(&mut Chain::new(&[limit, boost], RATE), 440.0, 1.0);
    let b = gain_at(&mut Chain::new(&[boost, limit], RATE), 440.0, 1.0);
    assert!(a > 0.9 && b < 0.55, "{a} {b}");
}

#[test]
fn settings_glide_to_the_new_values() {
    let low = |cutoff: f32| EffectSpec::Lowpass { cutoff, q: 0.707 };
    let mut chain = Chain::new(&[low(20000.0)], RATE);
    assert!(chain.retarget(&[low(200.0)], 1.0));
    for _ in 0..RATE / 2 {
        chain.process([0.0, 0.0]);
    }
    let EffectSpec::Lowpass { cutoff, .. } = chain.specs()[0] else { panic!() };
    assert!((cutoff - 2000.0).abs() < 60.0, "走到一半：截止频率按对数走 {cutoff}");
    for _ in 0..RATE / 2 + 64 {
        chain.process([0.0, 0.0]);
    }
    assert_eq!(chain.specs(), vec![low(200.0)], "走完正好是目标");
    assert!((gain_at(&mut chain, 200.0, 1.0) - 0.707).abs() < 0.01, "系数跟着变了");
    // 线性的参数线性地走
    let delay = |mix: f32| EffectSpec::Delay { time: 0.1, feedback: 0.0, mix };
    let mut chain = Chain::new(&[delay(0.0)], RATE);
    assert!(chain.retarget(&[delay(1.0)], 1.0));
    for _ in 0..RATE / 4 {
        chain.process([0.0, 0.0]);
    }
    let EffectSpec::Delay { mix, .. } = chain.specs()[0] else { panic!() };
    assert!((mix - 0.25).abs() < 0.01, "{mix}");
    // 过渡时间 0：马上到
    assert!(chain.retarget(&[delay(0.5)], 0.0));
    chain.process([0.0, 0.0]);
    assert_eq!(chain.specs(), vec![delay(0.5)]);
    // 形状不一样的不接
    assert!(!chain.retarget(&[low(100.0)], 0.0));
    assert!(!chain.retarget(&[], 0.0));
    assert_eq!(chain.specs(), vec![delay(0.5)]);
    // 中途改主意：从现在的值接着走
    let mut chain = Chain::new(&[delay(0.0)], RATE);
    chain.retarget(&[delay(1.0)], 1.0);
    for _ in 0..RATE / 2 {
        chain.process([0.0, 0.0]);
    }
    chain.retarget(&[delay(0.0)], 1.0);
    for _ in 0..RATE / 2 {
        chain.process([0.0, 0.0]);
    }
    let EffectSpec::Delay { mix, .. } = chain.specs()[0] else { panic!() };
    assert!((mix - 0.25).abs() < 0.01, "{mix}");
}

#[test]
fn tiny_sample_rates_do_not_blow_up() {
    use EffectSpec::*;
    let all = [
        Lowpass { cutoff: 800.0, q: 0.707 },
        Highpass { cutoff: 200.0, q: 0.707 },
        Reverb { room: 0.8, damping: 0.5, mix: 0.5 },
        Delay { time: 0.25, feedback: 0.5, mix: 0.5 },
        Compressor { threshold: -18.0, ratio: 4.0, attack: 0.01, release: 0.15, makeup: 0.0 },
        Limiter { ceiling: -1.0, release: 0.1 },
    ];
    for rate in [0, 1, 2, 7, 100] {
        let mut chain = Chain::new(&all, rate);
        chain.retarget(&all.map(|s| s.sanitize()), 0.5);
        for i in 0..500 {
            let [l, r] = chain.process([if i % 3 == 0 { 1.0 } else { -0.5 }, 0.3]);
            assert!(l.is_finite() && r.is_finite() && l.abs() < 50.0, "采样率 {rate} 第 {i} 个 {l} {r}");
        }
    }
    // 不是数的输入不留在反馈里
    let mut chain = Chain::new(&all, RATE);
    chain.process([f32::NAN, f32::INFINITY]);
    for _ in 0..RATE {
        chain.process([0.0, 0.0]);
    }
    let [l, r] = chain.process([0.1, 0.1]);
    assert!(l.is_finite() && r.is_finite());
}

#[test]
fn swapping_chains_crossfades() {
    // 旧链是空的（原样过去），新链是全湿的延迟（一开始没声）：换的时候输出从 1 平滑走到 0，不跳
    let mut swap = Swapper::new(Chain::new(&[], RATE));
    assert_eq!(swap.process([1.0, 1.0]), [1.0, 1.0]);
    let old = swap.replace(Chain::new(&[EffectSpec::Delay { time: 1.0, feedback: 0.0, mix: 1.0 }], RATE));
    assert!(old.is_none(), "旧链还在淡出，先不还");
    let n = (RATE as f32 * SWAP_FADE) as usize;
    let mut last = 1.0f32;
    let mut returned = 0;
    for i in 0..n + 10 {
        let out = swap.process([1.0, 1.0])[0];
        assert!(out <= last + 1e-6 && last - out < 2.0 / n as f32, "第 {i} 个采样 {last} → {out}");
        last = out;
        returned += usize::from(swap.take_retired().is_some());
    }
    assert_eq!(last, 0.0);
    assert_eq!(returned, 1, "淡完以后旧链交出来（回游戏线程释放）");
    // 淡到一半又换：正在淡出的那条马上交出来，从现在的新链接着淡
    let mut swap = Swapper::new(Chain::new(&[], RATE));
    swap.replace(Chain::new(&[], RATE));
    swap.process([1.0, 1.0]);
    assert!(swap.replace(Chain::new(&[], RATE)).is_some());
    assert_eq!(swap.process([0.5, 0.5]), [0.5, 0.5]);
}
