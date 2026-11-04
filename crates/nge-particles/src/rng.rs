//! 粒子用的随机数（xorshift32），不需要密码学强度。

#[derive(Debug, Clone)]
pub(crate) struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(seed.max(1))
    }

    /// [0, 1)
    pub fn next_f32(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// `[lo, hi]` 之间均匀取值。
    pub fn range(&mut self, (lo, hi): (f32, f32)) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_in_range() {
        let mut rng = Rng::new(7);
        let values: Vec<f32> = (0..10_000).map(|_| rng.range((2.0, 5.0))).collect();
        assert!(values.iter().all(|v| (2.0..=5.0).contains(v)));
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        assert!((mean - 3.5).abs() < 0.1, "{mean}");
    }

    #[test]
    fn zero_seed_still_moves() {
        let mut rng = Rng::new(0);
        assert_ne!(rng.next_f32(), rng.next_f32());
    }
}
