//! 令牌桶：每秒补 `rate` 个，最多攒 `burst` 个，发一条用一个。客户端限用户消息，中继限每条连接。

#[derive(Debug, Clone)]
pub struct TokenBucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    last: Option<f64>,
}

impl TokenBucket {
    /// 一开始是满的。
    pub fn new(rate: f64, burst: f64) -> Self {
        let burst = burst.max(1.0);
        Self { rate: rate.max(0.0), burst, tokens: burst, last: None }
    }

    /// 时间 `now`（秒）要发一条：有令牌就用掉一个返回 true。
    pub fn take(&mut self, now: f64) -> bool {
        if let Some(last) = self.last
            && now > last
        {
            self.tokens = (self.tokens + (now - last) * self.rate).min(self.burst);
        }
        // 时间倒退（时钟不准）时不补也不扣时间
        self.last = Some(self.last.map_or(now, |l| l.max(now)));
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_a_burst_then_the_rate() {
        let mut b = TokenBucket::new(4.0, 5.0);
        assert_eq!((0..8).filter(|_| b.take(0.0)).count(), 5, "先放过攒着的");
        assert!(!b.take(0.125), "半个令牌不够");
        assert!(b.take(0.25));
        assert_eq!((0..20).filter(|_| b.take(10.0)).count(), 5, "攒满为止");
    }

    #[test]
    fn time_going_backwards_does_not_refill() {
        let mut b = TokenBucket::new(2.0, 1.0);
        assert!(b.take(5.0));
        assert!(!b.take(1.0));
        assert!(!b.take(5.25), "倒退那一下没把时间往回拨");
        assert!(b.take(5.5));
    }
}
