//! 效果器链：声明（[`EffectSpec`]）、参数校验、一串 DSP 节点（[`Chain`]）、换链时的交叉淡变（[`Swapper`]）。
//! 都是纯计算，不碰声卡（见 docs/audio-3d.md）；接到 kira 上的部分在 `chain.rs`。

use crate::dsp::{Biquad, Compressor, Delay, Limiter, MAX_DELAY, Reverb};

/// 一条链最多几个效果器。
pub const MAX_EFFECTS: usize = 8;
/// 滤波默认的 Q（没有鼓包的二阶滤波）。
pub const DEFAULT_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// 换链时新旧两条交叉淡变多久（秒）。
pub const SWAP_FADE: f32 = 0.03;
/// 参数每多少个采样走一小步。
const CONTROL: u32 = 32;

/// 一个效果器的声明。频率是赫兹，时间是秒，`threshold` / `makeup` / `ceiling` 是分贝，`mix` 是湿声比例。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EffectSpec {
    Lowpass { cutoff: f32, q: f32 },
    Highpass { cutoff: f32, q: f32 },
    Reverb { room: f32, damping: f32, mix: f32 },
    Delay { time: f32, feedback: f32, mix: f32 },
    Compressor { threshold: f32, ratio: f32, attack: f32, release: f32, makeup: f32 },
    Limiter { ceiling: f32, release: f32 },
}

/// 截到范围里；不是有限数的用默认值。
fn fit(value: f32, min: f32, max: f32, default: f32) -> f32 {
    if value.is_finite() { value.clamp(min, max) } else { default }
}

type Params = [f32; 5];

impl EffectSpec {
    /// 每个参数截到合法范围，不是有限数的用默认值（范围见 docs/audio-3d.md 的表）。
    pub fn sanitize(self) -> Self {
        use EffectSpec::*;
        match self {
            // 不是数时不滤：低通开到头，高通关到底
            Lowpass { cutoff, q } => Lowpass { cutoff: fit(cutoff, 20.0, 20000.0, 20000.0), q: fit(q, 0.1, 10.0, DEFAULT_Q) },
            Highpass { cutoff, q } => Highpass { cutoff: fit(cutoff, 20.0, 20000.0, 20.0), q: fit(q, 0.1, 10.0, DEFAULT_Q) },
            Reverb { room, damping, mix } => Reverb { room: fit(room, 0.0, 1.0, 0.5), damping: fit(damping, 0.0, 1.0, 0.5), mix: fit(mix, 0.0, 1.0, 0.3) },
            Delay { time, feedback, mix } => Delay { time: fit(time, 0.001, MAX_DELAY, 0.25), feedback: fit(feedback, 0.0, 0.95, 0.35), mix: fit(mix, 0.0, 1.0, 0.3) },
            Compressor { threshold, ratio, attack, release, makeup } => Compressor {
                threshold: fit(threshold, -60.0, 0.0, -18.0),
                ratio: fit(ratio, 1.0, 20.0, 4.0),
                attack: fit(attack, 0.0001, 1.0, 0.01),
                release: fit(release, 0.001, 5.0, 0.15),
                makeup: fit(makeup, -24.0, 24.0, 0.0),
            },
            Limiter { ceiling, release } => Limiter { ceiling: fit(ceiling, -24.0, 0.0, -1.0), release: fit(release, 0.001, 5.0, 0.1) },
        }
    }

    fn params(self) -> Params {
        use EffectSpec::*;
        match self {
            Lowpass { cutoff, q } | Highpass { cutoff, q } => [cutoff, q, 0.0, 0.0, 0.0],
            Reverb { room, damping, mix } => [room, damping, mix, 0.0, 0.0],
            Delay { time, feedback, mix } => [time, feedback, mix, 0.0, 0.0],
            Compressor { threshold, ratio, attack, release, makeup } => [threshold, ratio, attack, release, makeup],
            Limiter { ceiling, release } => [ceiling, release, 0.0, 0.0, 0.0],
        }
    }

    /// 同一种效果器，换上这组参数。
    fn with(self, p: Params) -> Self {
        use EffectSpec::*;
        match self {
            Lowpass { .. } => Lowpass { cutoff: p[0], q: p[1] },
            Highpass { .. } => Highpass { cutoff: p[0], q: p[1] },
            Reverb { .. } => Reverb { room: p[0], damping: p[1], mix: p[2] },
            Delay { .. } => Delay { time: p[0], feedback: p[1], mix: p[2] },
            Compressor { .. } => Compressor { threshold: p[0], ratio: p[1], attack: p[2], release: p[3], makeup: p[4] },
            Limiter { .. } => Limiter { ceiling: p[0], release: p[1] },
        }
    }

    /// 截止频率按对数过渡（听起来才均匀），别的线性。
    fn glides_in_log(self, param: usize) -> bool {
        param == 0 && matches!(self, EffectSpec::Lowpass { .. } | EffectSpec::Highpass { .. })
    }
}

/// 整条链校验：每个效果器 [`EffectSpec::sanitize`]，超过 [`MAX_EFFECTS`] 个的丢掉。
pub fn sanitize_chain(specs: &[EffectSpec]) -> Vec<EffectSpec> {
    if specs.len() > MAX_EFFECTS {
        log::warn!("效果器链最多 {MAX_EFFECTS} 个，后面的 {} 个不要了", specs.len() - MAX_EFFECTS);
    }
    specs.iter().take(MAX_EFFECTS).map(|s| s.sanitize()).collect()
}

/// 两条链是不是同一个形状（种类和顺序一样，只有参数不同）：是的话可以原地过渡参数。
pub fn same_shape(a: &[EffectSpec], b: &[EffectSpec]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| std::mem::discriminant(a) == std::mem::discriminant(b))
}

#[derive(Debug, Clone)]
enum Dsp {
    Filter(Biquad),
    Reverb(Box<Reverb>),
    Delay(Delay),
    Compressor(Compressor),
    Limiter(Limiter),
}

#[derive(Debug, Clone)]
struct Node {
    /// 现在的参数（过渡中的值）。
    spec: EffectSpec,
    from: Params,
    to: Params,
    /// 过渡一共多少个采样、走了多少；相等就是走完了。
    total: u32,
    done: u32,
    dsp: Dsp,
}

impl Node {
    fn new(spec: EffectSpec, rate: f32) -> Self {
        let dsp = match spec {
            EffectSpec::Lowpass { .. } | EffectSpec::Highpass { .. } => Dsp::Filter(Biquad::default()),
            EffectSpec::Reverb { room, damping, mix } => Dsp::Reverb(Box::new(Reverb::new(room, damping, mix, rate))),
            EffectSpec::Delay { time, feedback, mix } => Dsp::Delay(Delay::new(time, feedback, mix, rate)),
            EffectSpec::Compressor { .. } => Dsp::Compressor(Compressor::default()),
            EffectSpec::Limiter { .. } => Dsp::Limiter(Limiter::default()),
        };
        let mut node = Self { spec, from: spec.params(), to: spec.params(), total: 0, done: 0, dsp };
        node.apply(rate);
        node
    }

    /// 把现在的参数写进 DSP（重算系数）。
    fn apply(&mut self, rate: f32) {
        match (&mut self.dsp, self.spec) {
            (Dsp::Filter(f), EffectSpec::Lowpass { cutoff, q }) => f.set(false, cutoff, q, rate),
            (Dsp::Filter(f), EffectSpec::Highpass { cutoff, q }) => f.set(true, cutoff, q, rate),
            (Dsp::Reverb(r), EffectSpec::Reverb { room, damping, mix }) => {
                r.set(room, damping);
                r.mix = mix;
            }
            (Dsp::Delay(d), EffectSpec::Delay { time, feedback, mix }) => {
                d.set_time(time, rate);
                (d.feedback, d.mix) = (feedback, mix);
            }
            (Dsp::Compressor(c), EffectSpec::Compressor { threshold, ratio, attack, release, makeup }) => c.set(threshold, ratio, attack, release, makeup, rate),
            (Dsp::Limiter(l), EffectSpec::Limiter { ceiling, release }) => l.set(ceiling, release, rate),
            _ => {}
        }
    }

    /// 过渡往前走 `samples` 个采样。
    fn glide(&mut self, samples: u32, rate: f32) {
        if self.done >= self.total {
            return;
        }
        self.done = (self.done + samples).min(self.total);
        let mut now = self.to;
        if self.done < self.total {
            let t = self.done as f32 / self.total as f32;
            for (i, v) in now.iter_mut().enumerate() {
                let (a, b) = (self.from[i], self.to[i]);
                *v = if a == b {
                    a
                } else if self.spec.glides_in_log(i) {
                    a * (b / a).powf(t)
                } else {
                    a + (b - a) * t
                };
            }
        }
        self.spec = self.spec.with(now);
        self.apply(rate);
    }

    #[inline]
    fn process(&mut self, frame: [f32; 2]) -> [f32; 2] {
        match &mut self.dsp {
            Dsp::Filter(f) => f.process(frame),
            Dsp::Reverb(r) => r.process(frame),
            Dsp::Delay(d) => d.process(frame),
            Dsp::Compressor(c) => c.process(frame),
            Dsp::Limiter(l) => l.process(frame),
        }
    }
}

/// 一串效果器，按声明的顺序处理。缓冲在 [`Chain::new`] 里分配好，[`Chain::process`] 不分配内存。
#[derive(Debug, Clone)]
pub struct Chain {
    rate: f32,
    nodes: Vec<Node>,
    /// 有参数还在过渡。
    gliding: bool,
    count: u32,
}

impl Chain {
    /// 按采样率建一条链（声明先校验）。采样率是 0 时按 1。
    pub fn new(specs: &[EffectSpec], rate: u32) -> Self {
        let rate = rate.max(1) as f32;
        Self { rate, nodes: sanitize_chain(specs).into_iter().map(|s| Node::new(s, rate)).collect(), gliding: false, count: 0 }
    }

    pub fn rate(&self) -> u32 {
        self.rate as u32
    }

    /// 现在的参数（过渡中的是走到一半的值）。
    pub fn specs(&self) -> Vec<EffectSpec> {
        self.nodes.iter().map(|n| n.spec).collect()
    }

    /// 形状一样（[`same_shape`]）时在 `seconds` 秒里把参数过渡到 `specs`，返回 true；不一样时什么都不做，返回 false。
    /// 不分配内存（音频线程上也调）。
    pub fn retarget(&mut self, specs: &[EffectSpec], seconds: f32) -> bool {
        let specs = &specs[..specs.len().min(MAX_EFFECTS)];
        if specs.len() != self.nodes.len() || !self.nodes.iter().zip(specs).all(|(n, s)| std::mem::discriminant(&n.spec) == std::mem::discriminant(s)) {
            return false;
        }
        let seconds = if seconds.is_finite() { seconds.clamp(0.0, 60.0) } else { 0.0 };
        let total = (seconds * self.rate) as u32;
        for (node, spec) in self.nodes.iter_mut().zip(specs) {
            (node.from, node.to, node.total, node.done) = (node.spec.params(), spec.sanitize().params(), total.max(1), 0);
            if total == 0 {
                node.glide(1, self.rate);
            }
        }
        self.gliding = total > 0;
        self.count = 0;
        true
    }

    /// 处理一个立体声采样。不是有限数的输入当 0（不让它留在反馈里）。
    #[inline]
    pub fn process(&mut self, frame: [f32; 2]) -> [f32; 2] {
        if self.gliding {
            self.count += 1;
            if self.count >= CONTROL {
                self.count = 0;
                let mut gliding = false;
                for node in &mut self.nodes {
                    node.glide(CONTROL, self.rate);
                    gliding |= node.done < node.total;
                }
                self.gliding = gliding;
            }
        }
        let mut frame = frame.map(|x| if x.is_finite() { x } else { 0.0 });
        for node in &mut self.nodes {
            frame = node.process(frame);
        }
        frame
    }
}

/// 换链：新链接上来时，新旧两条同时算、在 [`SWAP_FADE`] 秒里交叉淡变；淡完的旧链放在一边等人拿走
/// （音频线程上不释放内存，送回游戏线程释放）。
pub struct Swapper {
    chain: Chain,
    /// 正在淡出的旧链，淡了多少个采样、一共多少。
    fading: Option<(Chain, u32, u32)>,
    retired: Option<Chain>,
}

impl Swapper {
    pub fn new(chain: Chain) -> Self {
        Self { chain, fading: None, retired: None }
    }

    pub fn chain(&mut self) -> &mut Chain {
        &mut self.chain
    }

    /// 换成新链。上一条还没淡完时把它马上交出来（返回值），从现在这条接着淡。
    pub fn replace(&mut self, chain: Chain) -> Option<Chain> {
        let cut = self.fading.take().map(|(old, ..)| old);
        let total = ((chain.rate() as f32 * SWAP_FADE) as u32).max(1);
        let old = std::mem::replace(&mut self.chain, chain);
        self.fading = Some((old, 0, total));
        cut
    }

    /// 淡完的旧链（拿去别的线程释放）。
    pub fn take_retired(&mut self) -> Option<Chain> {
        self.retired.take()
    }

    #[inline]
    pub fn process(&mut self, frame: [f32; 2]) -> [f32; 2] {
        let new = self.chain.process(frame);
        let Some((old, done, total)) = &mut self.fading else { return new };
        let before = old.process(frame);
        let t = *done as f32 / *total as f32;
        *done += 1;
        if *done >= *total {
            self.retired = self.fading.take().map(|(old, ..)| old);
        }
        [before[0] + (new[0] - before[0]) * t, before[1] + (new[1] - before[1]) * t]
    }
}
