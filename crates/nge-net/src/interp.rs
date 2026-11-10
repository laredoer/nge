//! 远端实体的插值：按发送方的时间戳排好收到的样本，画「现在 − 偏移 − 延迟」那一刻的位置。
//! 乱序的插到该在的位置，重复的、已经播过的丢掉；等不到新样本时按最后的速度最多外推 [`MAX_EXTRAPOLATE`] 毫秒然后停住。

use std::collections::VecDeque;

/// 一个缓冲最多留几个样本。
pub const MAX_SAMPLES: usize = 32;
/// 最多往后外推几毫秒。
pub const MAX_EXTRAPOLATE: f64 = 100.0;
/// 两个样本离得比这远（point）当成瞬移（掉坑回起点），不在中间插值。
pub const SNAP_DISTANCE: f32 = 300.0;
/// 同一个人后一个快照的时间戳比前一个多出来的，超过本地过去的时间再加这么多（毫秒）时丢掉（伪造的）。
pub const MAX_JUMP: f64 = 5000.0;
/// 往回拨这么多（毫秒）时从头来过（对方重启了）：经中继转发是按顺序的，正常不会往回走。
pub const MAX_REWIND: f64 = 1000.0;
/// 默认的显示延迟（毫秒）：缓冲大约两个包（20 Hz）。
pub const DEFAULT_DELAY: f64 = 100.0;
/// 时钟偏移每次往上跟多少（比例）。
const DRIFT: f64 = 0.02;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub seq: u32,
    /// 发送方时钟，毫秒。
    pub t: f64,
    pub pos: [f32; 2],
    /// 弧度。
    pub rotation: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct State {
    pub pos: [f32; 2],
    pub rotation: f32,
}

#[derive(Debug, Clone, Default)]
pub struct InterpBuffer {
    samples: VecDeque<Sample>,
}

impl InterpBuffer {
    /// 放进一个样本；重复的、比留着的最旧的还旧的、数不是有限的不要，返回收没收。
    pub fn push(&mut self, sample: Sample) -> bool {
        let finite = sample.t.is_finite() && sample.pos.iter().all(|v| v.is_finite()) && sample.rotation.is_finite();
        if !finite || self.samples.iter().any(|s| s.seq == sample.seq) {
            return false;
        }
        if self.samples.front().is_some_and(|first| sample.t < first.t) {
            return false;
        }
        // 按时间插到该在的位置（多数时候在最后）
        let at = self.samples.iter().rposition(|s| s.t <= sample.t).map_or(0, |i| i + 1);
        self.samples.insert(at, sample);
        while self.samples.len() > MAX_SAMPLES {
            self.samples.pop_front();
        }
        true
    }

    /// 发送方时钟 `at`（毫秒）那一刻的状态；还没有样本时是 `None`。顺带丢掉以后用不到的旧样本。
    pub fn sample(&mut self, at: f64) -> Option<State> {
        // 第二个样本也不晚于 at 时，第一个以后用不到了
        while self.samples.len() > 2 && self.samples[1].t <= at {
            self.samples.pop_front();
        }
        let first = *self.samples.front()?;
        if at <= first.t {
            return Some(first.state());
        }
        let next = self.samples.iter().position(|s| s.t > at);
        match next {
            Some(i) => {
                let (a, b) = (self.samples[i - 1], self.samples[i]);
                if self.samples.len() > 2 && i >= 2 {
                    self.samples.drain(..i - 1);
                }
                Some(between(a, b, ((at - a.t) / (b.t - a.t)) as f32))
            }
            None => {
                let last = *self.samples.back()?;
                let Some(prev) = self.samples.iter().rev().nth(1).copied() else { return Some(last.state()) };
                if last.t <= prev.t || jumped(prev, last) {
                    return Some(last.state());
                }
                // 按最后两个样本的速度往后推，最多推 MAX_EXTRAPOLATE
                let ahead = (at - last.t).min(MAX_EXTRAPOLATE);
                Some(between(prev, last, (1.0 + ahead / (last.t - prev.t)) as f32))
            }
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

/// 估计「本地时钟 − 发送方时钟」：取路上最快的那次，之后慢慢往上跟。
#[derive(Debug, Clone, Copy, Default)]
pub struct Clock {
    offset: Option<f64>,
}

impl Clock {
    /// 本地时间 `local`（毫秒）收到了发送方时间 `remote` 的包。
    pub fn observe(&mut self, local: f64, remote: f64) {
        let est = local - remote;
        if !est.is_finite() {
            return;
        }
        self.offset = Some(match self.offset {
            Some(offset) if est >= offset => offset + (est - offset) * DRIFT,
            _ => est,
        });
    }

    /// 本地时间 `local` 往前推 `delay` 毫秒，换成发送方的时钟；还没收到过包时是 `None`。
    pub fn remote_time(&self, local: f64, delay: f64) -> Option<f64> {
        self.offset.map(|offset| local - offset - delay)
    }
}

impl Sample {
    fn state(&self) -> State {
        State { pos: self.pos, rotation: self.rotation }
    }
}

fn jumped(a: Sample, b: Sample) -> bool {
    let (dx, dy) = (b.pos[0] - a.pos[0], b.pos[1] - a.pos[1]);
    dx * dx + dy * dy > SNAP_DISTANCE * SNAP_DISTANCE
}

/// a、b 之间第 `k` 处（k 可以超过 1，用来外推）；瞬移时不插值，到 b 的时间之前一直是 a。
fn between(a: Sample, b: Sample, k: f32) -> State {
    if jumped(a, b) {
        return if k >= 1.0 { b.state() } else { a.state() };
    }
    let lerp = |x: f32, y: f32| x + (y - x) * k;
    let tau = std::f32::consts::TAU;
    // 角度差折到 (−π, π]，走短的那边
    let turn = (b.rotation - a.rotation + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI;
    State { pos: [lerp(a.pos[0], b.pos[0]), lerp(a.pos[1], b.pos[1])], rotation: a.rotation + turn * k }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(seq: u32, t: f64, x: f32) -> Sample {
        Sample { seq, t, pos: [x, 0.0], rotation: 0.0 }
    }

    fn x_at(b: &mut InterpBuffer, at: f64) -> f32 {
        b.sample(at).unwrap().pos[0]
    }

    #[test]
    fn interpolates_between_samples_by_time() {
        let mut b = InterpBuffer::default();
        assert_eq!(b.sample(0.0), None);
        b.push(s(1, 0.0, 0.0));
        b.push(s(2, 50.0, 10.0));
        b.push(s(3, 100.0, 30.0));
        assert_eq!(x_at(&mut b, 25.0), 5.0);
        assert_eq!(x_at(&mut b, 75.0), 20.0);
        assert_eq!(x_at(&mut b, 100.0), 30.0);
    }

    #[test]
    fn before_the_oldest_sample_holds_it() {
        let mut b = InterpBuffer::default();
        b.push(s(1, 100.0, 7.0));
        b.push(s(2, 150.0, 9.0));
        assert_eq!(x_at(&mut b, 0.0), 7.0);
    }

    #[test]
    fn out_of_order_samples_are_put_in_place() {
        let mut b = InterpBuffer::default();
        b.push(s(1, 0.0, 0.0));
        b.push(s(3, 100.0, 20.0));
        assert!(b.push(s(2, 50.0, 100.0)), "晚到的中间那个也要");
        assert_eq!(x_at(&mut b, 50.0), 100.0);
        assert_eq!(x_at(&mut b, 75.0), 60.0);
    }

    #[test]
    fn duplicates_and_already_played_samples_are_dropped() {
        let mut b = InterpBuffer::default();
        b.push(s(1, 0.0, 0.0));
        b.push(s(2, 50.0, 10.0));
        b.push(s(3, 100.0, 20.0));
        assert!(!b.push(s(2, 50.0, 999.0)), "同一个 seq");
        assert_eq!(x_at(&mut b, 75.0), 15.0);
        // 播到 75 以后，0 那个用不到了：比留着的最旧的还旧的包不要
        assert!(!b.push(s(9, 10.0, 5.0)));
        assert_eq!(x_at(&mut b, 75.0), 15.0);
    }

    #[test]
    fn lost_packets_are_bridged() {
        let mut b = InterpBuffer::default();
        b.push(s(1, 0.0, 0.0));
        // 2、3、4 丢了
        b.push(s(5, 200.0, 40.0));
        assert_eq!(x_at(&mut b, 100.0), 20.0);
    }

    #[test]
    fn extrapolates_at_most_the_cap_then_holds() {
        let mut b = InterpBuffer::default();
        b.push(s(1, 0.0, 0.0));
        b.push(s(2, 50.0, 10.0));
        // 速度 0.2 / 毫秒
        assert!((x_at(&mut b, 100.0) - 20.0).abs() < 1e-4, "外推 50 毫秒");
        assert!((x_at(&mut b, 150.0) - 30.0).abs() < 1e-4, "外推 100 毫秒");
        assert!((x_at(&mut b, 1000.0) - 30.0).abs() < 1e-4, "再往后停住");
        // 只有一个样本时没法外推，停在它那里
        let mut one = InterpBuffer::default();
        one.push(s(1, 0.0, 3.0));
        assert_eq!(x_at(&mut one, 500.0), 3.0);
    }

    #[test]
    fn teleports_are_not_smeared() {
        let mut b = InterpBuffer::default();
        b.push(s(1, 0.0, 0.0));
        b.push(s(2, 50.0, 1000.0));
        assert_eq!(x_at(&mut b, 25.0), 0.0, "瞬移前停在原地");
        assert_eq!(x_at(&mut b, 50.0), 1000.0);
        assert_eq!(x_at(&mut b, 80.0), 1000.0, "瞬移不外推");
    }

    #[test]
    fn rotation_takes_the_short_way() {
        let mut b = InterpBuffer::default();
        let pi = std::f32::consts::PI;
        b.push(Sample { seq: 1, t: 0.0, pos: [0.0, 0.0], rotation: pi - 0.1 });
        b.push(Sample { seq: 2, t: 100.0, pos: [0.0, 0.0], rotation: -pi + 0.1 });
        let r = b.sample(50.0).unwrap().rotation;
        assert!((r.abs() - pi).abs() < 1e-3, "从 π−0.1 到 −π+0.1 走 0.2，不绕一圈：{r}");
    }

    #[test]
    fn the_buffer_is_capped_and_old_samples_are_pruned() {
        let mut b = InterpBuffer::default();
        for i in 0..100 {
            b.push(s(i, i as f64 * 50.0, i as f32));
        }
        assert!(b.len() <= MAX_SAMPLES);
        x_at(&mut b, 4900.0);
        assert!(b.len() <= 2, "播过的都丢掉：剩 {}", b.len());
    }

    #[test]
    fn non_finite_samples_are_rejected() {
        let mut b = InterpBuffer::default();
        assert!(!b.push(Sample { seq: 1, t: 0.0, pos: [f32::NAN, 0.0], rotation: 0.0 }));
        assert!(!b.push(Sample { seq: 1, t: f64::INFINITY, pos: [0.0, 0.0], rotation: 0.0 }));
        assert!(!b.push(Sample { seq: 1, t: 0.0, pos: [0.0, 0.0], rotation: f32::NAN }));
        assert!(b.is_empty());
    }

    #[test]
    fn clock_uses_the_fastest_packet_and_drifts_up_slowly() {
        let mut c = Clock::default();
        assert_eq!(c.remote_time(1000.0, 100.0), None);
        c.observe(1000.0, 0.0); // 偏移 1000
        c.observe(1060.0, 50.0); // 这个路上慢了 10
        c.observe(1100.0, 110.0); // 这个快：偏移降到 990
        assert_eq!(c.remote_time(1200.0, 100.0), Some(110.0));
        // 之后的包一直慢 50：偏移一点点往上跟，不会一下跳过去
        for i in 0..10 {
            c.observe(2040.0 + i as f64 * 50.0, 1000.0 + i as f64 * 50.0);
        }
        let offset = 3000.0 - c.remote_time(3000.0, 0.0).unwrap();
        assert!(offset > 990.0 && offset < 1040.0, "{offset}");
    }
}
