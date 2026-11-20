//! 客户端预测和回滚（纯逻辑，不碰渲染）：
//!
//! - 每个 tick 用自己的输入先走一步（`predict`），输入和走完的状态记进环形缓冲（`history`）。
//! - 收到服务端 tick `t` 的权威状态（`reconcile`）：和当时预测的比，一样就确认、丢掉更旧的；
//!   不一样就回到权威状态，把 `t` 之后还没确认的输入重放一遍（回滚）。
//! - 回滚前后显示的位置差记成一个偏移，之后按时间衰减到 0（平滑修正），差得太远（重生、被传送）直接跳过去。
//! - `Pacer`：客户端要比服务端超前一点（输入在服务端用到之前送到），按往返时间调快调慢。

use std::collections::VecDeque;

/// 要预测的东西：状态、输入、走一步。
pub trait Model {
    type State: Clone;
    type Input: Copy + Default;
    fn step(&self, state: &Self::State, input: Self::Input) -> Self::State;
    /// 显示的位置。
    fn pos(&self, state: &Self::State) -> [f32; 2];
    /// 预测的和权威的算不算一样（只比预测得了的部分）。
    fn same(&self, predicted: &Self::State, authoritative: &Self::State) -> bool;
}

/// 历史最多留几个 tick（30 Hz 下 2 秒多）。
pub const HISTORY: usize = 72;
/// 修正偏移衰减的时间常数（秒）：大约 0.1 秒追上一大半。
pub const SMOOTH_TAU: f32 = 0.1;
/// 修正超过这么远直接跳过去，不拖。
pub const SNAP: f32 = 200.0;

struct Frame<S, I> {
    tick: u32,
    input: I,
    state: S,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stats {
    /// 回滚了几次。
    pub rollbacks: u32,
    /// 回滚时重放了几个 tick（累计）。
    pub replayed: u64,
    /// 最近一次回滚的预测误差（显示位置差，point）。
    pub last_error: f32,
    /// 最大的一次。
    pub max_error: f32,
    /// 整个重来（服务端跑到前面去了、历史不够）几次。
    pub resets: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    /// 比确认过的还旧，或者重复：不管。
    Stale,
    /// 和预测的一样。
    Confirmed,
    /// 回滚重放了几个 tick，误差多大。
    Rolled { replayed: u32, error: f32 },
    /// 没有那个 tick 的预测：直接用权威状态从头来。
    Reset,
}

pub struct Predictor<M: Model> {
    model: M,
    tick: u32,
    state: M::State,
    /// 上一个 tick 的状态（显示时在两个 tick 之间插值）。
    prev: M::State,
    history: VecDeque<Frame<M::State, M::Input>>,
    confirmed: u32,
    offset: [f32; 2],
    pub stats: Stats,
}

impl<M: Model> Predictor<M> {
    /// 从 tick `tick` 的状态开始。
    pub fn new(model: M, tick: u32, state: M::State) -> Self {
        Self { model, tick, prev: state.clone(), state, history: VecDeque::new(), confirmed: tick, offset: [0.0; 2], stats: Stats::default() }
    }

    pub fn tick(&self) -> u32 {
        self.tick
    }

    pub fn state(&self) -> &M::State {
        &self.state
    }

    pub fn confirmed(&self) -> u32 {
        self.confirmed
    }

    pub fn model(&self) -> &M {
        &self.model
    }

    /// 用 `input` 走到下一个 tick，返回这个 tick。
    pub fn predict(&mut self, input: M::Input) -> u32 {
        self.tick = self.tick.wrapping_add(1);
        self.prev = self.state.clone();
        self.state = self.model.step(&self.state, input);
        self.history.push_back(Frame { tick: self.tick, input, state: self.state.clone() });
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        self.tick
    }

    /// 还没确认的输入：`(第一个 tick, 输入)`，最多 `max` 个（最新的）。
    pub fn pending(&self, max: usize) -> Option<(u32, Vec<M::Input>)> {
        let skip = self.history.len().saturating_sub(max);
        let first = self.history.get(skip)?.tick;
        Some((first, self.history.iter().skip(skip).map(|f| f.input).collect()))
    }

    /// 服务端说 tick `at` 时是 `auth`。
    pub fn reconcile(&mut self, at: u32, auth: &M::State) -> Outcome {
        if at <= self.confirmed {
            return Outcome::Stale;
        }
        while self.history.front().is_some_and(|f| f.tick < at) {
            self.history.pop_front();
        }
        let matches = match self.history.front() {
            Some(f) if f.tick == at => self.model.same(&f.state, auth),
            _ => return self.reset(at, auth),
        };
        self.history.pop_front();
        self.confirmed = at;
        if matches {
            return Outcome::Confirmed;
        }
        let before = self.display_pos();
        let mut s = auth.clone();
        let mut prev = auth.clone();
        for f in self.history.iter_mut() {
            prev = s.clone();
            s = self.model.step(&s, f.input);
            f.state = s.clone();
        }
        let replayed = self.history.len() as u32;
        self.prev = if replayed == 0 { auth.clone() } else { prev };
        self.state = s;
        let after = self.model.pos(&self.state);
        let error = dist(before, after);
        self.offset = [before[0] - after[0], before[1] - after[1]];
        if dist(self.offset, [0.0; 2]) > SNAP {
            self.offset = [0.0; 2];
        }
        self.stats.rollbacks += 1;
        self.stats.replayed += replayed as u64;
        self.stats.last_error = error;
        self.stats.max_error = self.stats.max_error.max(error);
        Outcome::Rolled { replayed, error }
    }

    fn reset(&mut self, at: u32, auth: &M::State) -> Outcome {
        self.tick = at;
        self.state = auth.clone();
        self.prev = auth.clone();
        self.history.clear();
        self.confirmed = at;
        self.offset = [0.0; 2];
        self.stats.resets += 1;
        Outcome::Reset
    }

    /// 现在显示的位置（含修正偏移，不插值）。
    fn display_pos(&self) -> [f32; 2] {
        let p = self.model.pos(&self.state);
        [p[0] + self.offset[0], p[1] + self.offset[1]]
    }

    /// 修正偏移随时间衰减 `dt` 秒。
    pub fn smooth(&mut self, dt: f32) {
        let k = (-dt.max(0.0) / SMOOTH_TAU).exp();
        self.offset = [self.offset[0] * k, self.offset[1] * k];
        if dist(self.offset, [0.0; 2]) < 0.01 {
            self.offset = [0.0; 2];
        }
    }

    pub fn offset(&self) -> [f32; 2] {
        self.offset
    }

    /// 显示的位置：上一个 tick 和这一个之间按 `alpha`（0..1）插值，加上修正偏移。
    pub fn display(&self, alpha: f32) -> [f32; 2] {
        let (a, b) = (self.model.pos(&self.prev), self.model.pos(&self.state));
        let t = alpha.clamp(0.0, 1.0);
        [a[0] + (b[0] - a[0]) * t + self.offset[0], a[1] + (b[1] - a[1]) * t + self.offset[1]]
    }
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// 本地 tick 走多快：超前得不够就快 10%，超前太多就慢 10%，差得离谱（比如刚连上）由调用方直接跳。
#[derive(Debug, Clone, Default)]
pub struct Pacer {
    acc: f64,
}

/// 一帧最多补几个 tick：卡了很久回来不一次追太多。
pub const MAX_STEPS: u32 = 8;

impl Pacer {
    /// 过了 `dt` 秒，现在超前 `lead` 个 tick、想要超前 `target` 个：这一帧走几个 tick。
    pub fn advance(&mut self, dt: f64, tick_seconds: f64, lead: i64, target: i64) -> u32 {
        let speed = if lead < target {
            1.1
        } else if lead > target + 2 {
            0.9
        } else {
            1.0
        };
        self.acc += dt.max(0.0) * speed;
        let steps = (self.acc / tick_seconds).floor().max(0.0) as u32;
        self.acc -= steps as f64 * tick_seconds;
        if steps > MAX_STEPS {
            self.acc = 0.0;
            return MAX_STEPS;
        }
        steps
    }

    /// 离下一个 tick 走了多少（0..1），显示插值用。
    pub fn alpha(&self, tick_seconds: f64) -> f32 {
        (self.acc / tick_seconds).clamp(0.0, 1.0) as f32
    }
}

/// 想超前几个 tick：一个往返（输入过去要半个，快照回来是半个旧的）加 2 个的余量。
pub fn target_lead(rtt: f64, tick_seconds: f64) -> i64 {
    (rtt.max(0.0) / tick_seconds).ceil() as i64 + 2
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一维的小模型：输入是速度，位置累加；`bump` 是服务端额外加的（客户端预测不到）。
    #[derive(Clone, Copy)]
    struct Line;

    impl Model for Line {
        type State = (f32, f32);
        type Input = i8;
        fn step(&self, s: &(f32, f32), i: i8) -> (f32, f32) {
            (s.0 + i as f32, s.1)
        }
        fn pos(&self, s: &(f32, f32)) -> [f32; 2] {
            [s.0, 0.0]
        }
        fn same(&self, a: &(f32, f32), b: &(f32, f32)) -> bool {
            a.0 == b.0
        }
    }

    fn predictor() -> Predictor<Line> {
        Predictor::new(Line, 10, (0.0, 0.0))
    }

    #[test]
    fn input_buffer_keeps_the_unconfirmed_inputs_in_order() {
        let mut p = predictor();
        for i in [1, 2, 3, 4] {
            p.predict(i);
        }
        assert_eq!(p.tick(), 14);
        assert_eq!(p.pending(10), Some((11, vec![1, 2, 3, 4])));
        assert_eq!(p.pending(2), Some((13, vec![3, 4])), "只要最新的几个");
        assert_eq!(p.reconcile(12, &(3.0, 0.0)), Outcome::Confirmed);
        assert_eq!(p.pending(10), Some((13, vec![3, 4])), "确认过的不再发");
        for _ in 0..HISTORY + 5 {
            p.predict(0);
        }
        assert_eq!(p.pending(usize::MAX).unwrap().1.len(), HISTORY, "环形缓冲有上限");
    }

    #[test]
    fn matching_snapshots_confirm_without_rollback() {
        let mut p = predictor();
        p.predict(1);
        p.predict(1);
        assert_eq!(p.reconcile(11, &(1.0, 0.0)), Outcome::Confirmed);
        assert_eq!(p.reconcile(11, &(1.0, 0.0)), Outcome::Stale, "重复的");
        assert_eq!(p.reconcile(10, &(9.0, 0.0)), Outcome::Stale, "更旧的");
        assert_eq!((p.stats.rollbacks, *p.state()), (0, (2.0, 0.0)));
    }

    #[test]
    fn a_mismatch_rolls_back_and_replays_the_unconfirmed_inputs() {
        let mut p = predictor();
        for i in [1, 1, 1, 1, 1] {
            p.predict(i);
        }
        // 服务端在 tick 12 时说是 5（它那边多了 3），之后 13..15 的输入要在 5 上重放
        assert_eq!(p.reconcile(12, &(5.0, 7.0)), Outcome::Rolled { replayed: 3, error: 3.0 });
        assert_eq!(*p.state(), (8.0, 7.0), "重放后的状态带着服务端改的部分");
        assert_eq!(p.stats.rollbacks, 1);
        assert_eq!(p.stats.replayed, 3);
        // 重放后的历史也改了：之后的确认按新的比
        assert_eq!(p.reconcile(14, &(7.0, 7.0)), Outcome::Confirmed);
        assert_eq!(p.pending(10), Some((15, vec![1])));
    }

    #[test]
    fn corrections_are_smoothed_not_teleported() {
        let mut p = predictor();
        for _ in 0..5 {
            p.predict(1);
        }
        let before = p.display(1.0);
        p.reconcile(12, &(5.0, 0.0));
        // 回滚的那一刻显示的位置不跳
        assert_eq!(p.display(1.0), before);
        assert_eq!(p.offset(), [-3.0, 0.0]);
        p.smooth(SMOOTH_TAU);
        assert!((p.offset()[0] + 3.0 * (-1.0f32).exp()).abs() < 1e-4, "按时间常数衰减 {:?}", p.offset());
        for _ in 0..30 {
            p.smooth(0.05);
        }
        assert_eq!(p.offset(), [0.0, 0.0], "最后归零");
        assert_eq!(p.display(1.0), [8.0, 0.0]);
        // 差得太远：直接跳
        p.reconcile(15, &(1000.0, 0.0));
        assert_eq!(p.offset(), [0.0, 0.0]);
    }

    #[test]
    fn a_snapshot_beyond_the_prediction_resets() {
        let mut p = predictor();
        p.predict(1);
        assert_eq!(p.reconcile(20, &(42.0, 0.0)), Outcome::Reset);
        assert_eq!((p.tick(), *p.state(), p.pending(10)), (20, (42.0, 0.0), None));
        // 历史被挤掉了的旧 tick 也是从头来
        let mut p = predictor();
        for _ in 0..HISTORY + 10 {
            p.predict(1);
        }
        assert_eq!(p.reconcile(12, &(2.0, 0.0)), Outcome::Reset);
        assert_eq!(p.stats.resets, 1);
    }

    #[test]
    fn display_interpolates_between_ticks() {
        let mut p = predictor();
        p.predict(4);
        assert_eq!(p.display(0.0), [0.0, 0.0]);
        assert_eq!(p.display(0.5), [2.0, 0.0]);
        assert_eq!(p.display(1.0), [4.0, 0.0]);
    }

    #[test]
    fn the_pacer_speeds_up_slows_down_and_caps_catch_up() {
        let dt = 1.0 / 30.0;
        let mut p = Pacer::default();
        let steps: u32 = (0..300).map(|_| p.advance(1.0 / 60.0, dt, 5, 5)).sum();
        assert!((149..=150).contains(&steps), "超前正好：正常速度 {steps}");
        let mut p = Pacer::default();
        let fast: u32 = (0..300).map(|_| p.advance(1.0 / 60.0, dt, 2, 5)).sum();
        let mut p = Pacer::default();
        let slow: u32 = (0..300).map(|_| p.advance(1.0 / 60.0, dt, 9, 5)).sum();
        assert!((164..=165).contains(&fast) && (134..=135).contains(&slow), "{fast} {slow}");
        let mut p = Pacer::default();
        assert_eq!(p.advance(3.0, dt, 5, 5), MAX_STEPS, "卡了很久：一帧最多补这么多");
        assert_eq!(p.advance(0.0, dt, 5, 5), 0);
        assert_eq!(target_lead(0.1, dt), 5);
        assert_eq!(target_lead(0.0, dt), 2);
    }
}
