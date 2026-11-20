//! 效果器的 DSP 节点：二阶滤波、延迟、混响、压缩、限幅（见 docs/audio-3d.md）。
//! 都是立体声、逐个采样处理，建的时候把缓冲分配好，处理时不分配内存。

use std::f32::consts::TAU;

/// 很小的数当 0：反馈里的数衰减到非规格化浮点会拖慢 CPU。
#[inline]
fn flush(x: f32) -> f32 {
    if x.abs() < 1e-20 { 0.0 } else { x }
}

pub(crate) fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// 一阶平滑的系数：`seconds` 秒的时间常数。
fn smoothing(seconds: f32, rate: f32) -> f32 {
    (-1.0 / (seconds * rate).max(1e-6)).exp()
}

/// 二阶滤波（RBJ 的低通 / 高通），转置直接 II 型。
#[derive(Debug, Clone, Default)]
pub(crate) struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [[f32; 2]; 2],
}

impl Biquad {
    /// 截止频率截到采样率的 0.45 倍以内（奈奎斯特附近系数会坏掉）。
    pub fn set(&mut self, highpass: bool, cutoff: f32, q: f32, rate: f32) {
        let w = TAU * cutoff.min(rate * 0.45).max(rate * 1e-4) / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q.max(0.01));
        let a0 = 1.0 + alpha;
        let b = if highpass { [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0] } else { [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0] };
        self.b = b.map(|v| v / a0);
        self.a = [-2.0 * cos / a0, (1.0 - alpha) / a0];
    }

    #[inline]
    pub fn process(&mut self, frame: [f32; 2]) -> [f32; 2] {
        let mut out = [0.0; 2];
        for c in 0..2 {
            let x = frame[c];
            let y = self.b[0] * x + self.z[c][0];
            self.z[c][0] = flush(self.b[1] * x - self.a[0] * y + self.z[c][1]);
            self.z[c][1] = flush(self.b[2] * x - self.a[1] * y);
            out[c] = y;
        }
        out
    }
}

/// 最长的延迟（秒）：缓冲按它开，改 `time` 不用重新分配。
pub(crate) const MAX_DELAY: f32 = 2.0;
/// 改延迟时间时，读的位置用多久挪过去（秒）。
const DELAY_GLIDE: f32 = 0.05;

/// 带反馈的延迟。读的位置带小数（线性插值），改时间时平滑地挪。
#[derive(Debug, Clone)]
pub(crate) struct Delay {
    buf: Vec<[f32; 2]>,
    write: usize,
    /// 现在延迟多少个采样（平滑过的）、要到多少。
    len: f32,
    target: f32,
    glide: f32,
    pub feedback: f32,
    pub mix: f32,
}

impl Delay {
    pub fn new(time: f32, feedback: f32, mix: f32, rate: f32) -> Self {
        let size = (MAX_DELAY * rate).ceil() as usize + 2;
        let mut delay = Self { buf: vec![[0.0; 2]; size], write: 0, len: 1.0, target: 1.0, glide: 1.0 - smoothing(DELAY_GLIDE, rate), feedback, mix };
        delay.set_time(time, rate);
        delay.len = delay.target;
        delay
    }

    pub fn set_time(&mut self, time: f32, rate: f32) {
        self.target = (time * rate).clamp(1.0, (self.buf.len() - 2) as f32);
    }

    #[inline]
    pub fn process(&mut self, frame: [f32; 2]) -> [f32; 2] {
        if self.len != self.target {
            self.len += (self.target - self.len) * self.glide;
            if (self.target - self.len).abs() < 1e-3 {
                self.len = self.target;
            }
        }
        let size = self.buf.len();
        let whole = self.len as usize;
        let frac = self.len - whole as f32;
        let (a, b) = (self.buf[(self.write + size - whole) % size], self.buf[(self.write + size - whole - 1) % size]);
        let mut out = [0.0; 2];
        for c in 0..2 {
            let wet = a[c] + (b[c] - a[c]) * frac;
            self.buf[self.write][c] = flush(frame[c] + wet * self.feedback);
            out[c] = frame[c] + (wet - frame[c]) * self.mix;
        }
        self.write = (self.write + 1) % size;
        out
    }
}

/// Freeverb 的梳状、全通滤波的长度（44100 Hz 下的采样数），右声道各多 [`SPREAD`] 个。
const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
const SPREAD: usize = 23;
const TUNED_RATE: f32 = 44100.0;
/// 输入的增益、湿声的增益（Freeverb 原来的数）。
const REVERB_INPUT: f32 = 0.015;
const REVERB_WET: f32 = 3.0;

#[derive(Debug, Clone)]
struct Comb {
    buf: Vec<f32>,
    at: usize,
    store: f32,
}

impl Comb {
    #[inline]
    fn process(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let out = self.buf[self.at];
        self.store = flush(out * (1.0 - damp) + self.store * damp);
        self.buf[self.at] = flush(x + self.store * feedback);
        self.at = (self.at + 1) % self.buf.len();
        out
    }
}

#[derive(Debug, Clone)]
struct Allpass {
    buf: Vec<f32>,
    at: usize,
}

impl Allpass {
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let delayed = self.buf[self.at];
        self.buf[self.at] = flush(x + delayed * 0.5);
        self.at = (self.at + 1) % self.buf.len();
        delayed - x
    }
}

/// 混响（Freeverb）：左右各 8 个梳状滤波并联、再串 4 个全通；输入是左右的和。
#[derive(Debug, Clone)]
pub(crate) struct Reverb {
    combs: [Vec<Comb>; 2],
    allpasses: [Vec<Allpass>; 2],
    pub feedback: f32,
    pub damp: f32,
    pub mix: f32,
}

impl Reverb {
    pub fn new(room: f32, damping: f32, mix: f32, rate: f32) -> Self {
        let len = |samples: usize| ((samples as f32 * rate / TUNED_RATE).round() as usize).max(1);
        let side = |spread: usize| COMBS.iter().map(|&n| Comb { buf: vec![0.0; len(n + spread)], at: 0, store: 0.0 }).collect::<Vec<_>>();
        let pass = |spread: usize| ALLPASSES.iter().map(|&n| Allpass { buf: vec![0.0; len(n + spread)], at: 0 }).collect::<Vec<_>>();
        let mut reverb = Self { combs: [side(0), side(SPREAD)], allpasses: [pass(0), pass(SPREAD)], feedback: 0.0, damp: 0.0, mix };
        reverb.set(room, damping);
        reverb
    }

    pub fn set(&mut self, room: f32, damping: f32) {
        self.feedback = room * 0.28 + 0.7;
        self.damp = damping * 0.4;
    }

    #[inline]
    pub fn process(&mut self, frame: [f32; 2]) -> [f32; 2] {
        let input = (frame[0] + frame[1]) * REVERB_INPUT;
        let mut out = [0.0; 2];
        for c in 0..2 {
            let mut wet = 0.0;
            for comb in &mut self.combs[c] {
                wet += comb.process(input, self.feedback, self.damp);
            }
            for allpass in &mut self.allpasses[c] {
                wet = allpass.process(wet);
            }
            out[c] = frame[c] + (wet * REVERB_WET - frame[c]) * self.mix;
        }
        out
    }
}

/// 前馈压缩：包络按左右里大的那个算，硬拐点。
#[derive(Debug, Clone, Default)]
pub(crate) struct Compressor {
    env: f32,
    attack: f32,
    release: f32,
    pub threshold: f32,
    pub ratio: f32,
    makeup: f32,
}

impl Compressor {
    pub fn set(&mut self, threshold: f32, ratio: f32, attack: f32, release: f32, makeup: f32, rate: f32) {
        self.threshold = threshold;
        self.ratio = ratio.max(1.0);
        self.attack = smoothing(attack, rate);
        self.release = smoothing(release, rate);
        self.makeup = db_to_gain(makeup);
    }

    #[inline]
    pub fn process(&mut self, frame: [f32; 2]) -> [f32; 2] {
        let peak = frame[0].abs().max(frame[1].abs());
        let k = if peak > self.env { self.attack } else { self.release };
        self.env = flush(peak + (self.env - peak) * k);
        let over = 20.0 * self.env.max(1e-9).log10() - self.threshold;
        let gain = if over > 0.0 { db_to_gain(-over * (1.0 - 1.0 / self.ratio)) * self.makeup } else { self.makeup };
        [frame[0] * gain, frame[1] * gain]
    }
}

/// 限幅：包络马上跟到峰值、按 `release` 放开，输出不超过 `ceiling`。
#[derive(Debug, Clone, Default)]
pub(crate) struct Limiter {
    env: f32,
    release: f32,
    ceiling: f32,
}

impl Limiter {
    pub fn set(&mut self, ceiling: f32, release: f32, rate: f32) {
        self.ceiling = db_to_gain(ceiling);
        self.release = smoothing(release, rate);
    }

    #[inline]
    pub fn process(&mut self, frame: [f32; 2]) -> [f32; 2] {
        let peak = frame[0].abs().max(frame[1].abs());
        self.env = flush(peak.max(self.env * self.release));
        if self.env > self.ceiling {
            let gain = self.ceiling / self.env;
            [frame[0] * gain, frame[1] * gain]
        } else {
            frame
        }
    }
}
