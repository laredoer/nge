//! 重连退避：0.5 秒起每次翻倍，到上限为止，乘一个 ±20% 的抖动，免得一群客户端同时断开又同时连回来。

/// 抖动的幅度（±20%）。
pub const JITTER: f64 = 0.2;

#[derive(Debug, Clone)]
pub struct Backoff {
    base: f64,
    max: f64,
    jitter: f64,
    attempt: u32,
    rng: u64,
}

impl Backoff {
    /// `base` 秒起，最多 `max` 秒；不是正的有限数时用默认值（0.5、10）。
    pub fn new(base: f64, max: f64) -> Self {
        let positive = |v: f64, default: f64| if v.is_finite() && v > 0.0 { v } else { default };
        let base = positive(base, 0.5);
        Self { base, max: positive(max, 10.0).max(base), jitter: JITTER, attempt: 0, rng: 0x9e37_79b9_7f4a_7c15 }
    }

    /// 指定随机种子（测试用，结果可重现）。
    pub fn with_seed(self, seed: u64) -> Self {
        // xorshift 的状态不能是 0
        Self { rng: seed | 1, ..self }
    }

    /// 不加抖动（测试用）。
    pub fn without_jitter(self) -> Self {
        Self { jitter: 0.0, ..self }
    }

    /// 下一次重连前等几秒，并把次数加一。
    pub fn next_delay(&mut self) -> f64 {
        // 次数大了 2^n 会溢出成无穷，先夹住指数
        let delay = (self.base * 2f64.powi(self.attempt.min(30) as i32)).min(self.max);
        self.attempt = self.attempt.saturating_add(1);
        delay * (1.0 + self.jitter * (2.0 * self.random() - 1.0))
    }

    /// 连上了：从头算。
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// [0, 1) 的随机数（xorshift64）。
    fn random(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn attempts(&self) -> u32 {
        self.attempt
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(0.5, 10.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_up_to_the_cap() {
        let mut b = Backoff::new(0.5, 10.0).without_jitter();
        let delays: Vec<f64> = (0..8).map(|_| b.next_delay()).collect();
        assert_eq!(delays, vec![0.5, 1.0, 2.0, 4.0, 8.0, 10.0, 10.0, 10.0]);
        assert_eq!(b.attempts(), 8);
    }

    #[test]
    fn reset_starts_over() {
        let mut b = Backoff::new(0.5, 10.0).without_jitter();
        for _ in 0..5 {
            b.next_delay();
        }
        b.reset();
        assert_eq!(b.attempts(), 0);
        assert_eq!(b.next_delay(), 0.5);
    }

    #[test]
    fn jitter_stays_within_twenty_percent_and_varies() {
        let mut b = Backoff::new(1.0, 1.0).with_seed(42);
        let delays: Vec<f64> = (0..200).map(|_| b.next_delay()).collect();
        assert!(delays.iter().all(|d| (0.8..=1.2).contains(d)), "{delays:?}");
        let spread = delays.iter().cloned().fold(f64::MIN, f64::max) - delays.iter().cloned().fold(f64::MAX, f64::min);
        assert!(spread > 0.2, "抖动要真的分散开：{spread}");
        // 同一个种子结果一样
        let mut again = Backoff::new(1.0, 1.0).with_seed(42);
        assert_eq!(again.next_delay(), delays[0]);
    }

    #[test]
    fn many_attempts_do_not_overflow() {
        let mut b = Backoff::new(0.5, 10.0).without_jitter();
        for _ in 0..1000 {
            assert!(b.next_delay() <= 10.0);
        }
    }

    #[test]
    fn bad_settings_fall_back_to_defaults() {
        let mut b = Backoff::new(f64::NAN, -1.0).without_jitter();
        assert_eq!(b.next_delay(), 0.5);
        for _ in 0..10 {
            b.next_delay();
        }
        assert_eq!(b.next_delay(), 10.0);
    }
}
