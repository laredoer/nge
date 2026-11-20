//! 测试用的模拟网络：单向的一条线，可设延迟、抖动（抖动大于发送间隔就会乱序）、丢包，随机数用固定的种子。

/// 一条单向的模拟线路。
#[derive(Debug, Clone)]
pub struct SimNet<T> {
    /// 基本延迟（秒）。
    pub latency: f64,
    /// 额外的随机延迟 0..`jitter`（秒）。
    pub jitter: f64,
    /// 丢包率 0..1。
    pub loss: f64,
    rng: u64,
    seq: u64,
    queue: Vec<(f64, u64, T)>,
    pub sent: u64,
    pub lost: u64,
    /// 比先发的先到的有几个。
    pub reordered: u64,
    last_arrival: f64,
}

impl<T> SimNet<T> {
    pub fn new(latency: f64, jitter: f64, loss: f64, seed: u64) -> Self {
        Self { latency, jitter, loss, rng: seed | 1, seq: 0, queue: Vec::new(), sent: 0, lost: 0, reordered: 0, last_arrival: f64::NEG_INFINITY }
    }

    /// 0..1 的伪随机数（xorshift）。
    fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }

    /// 时间 `now` 发出去。
    pub fn send(&mut self, now: f64, msg: T) {
        self.sent += 1;
        if self.random() < self.loss {
            self.lost += 1;
            return;
        }
        let arrival = now + self.latency + self.jitter * self.random();
        if arrival < self.last_arrival {
            self.reordered += 1;
        }
        self.last_arrival = self.last_arrival.max(arrival);
        self.seq += 1;
        self.queue.push((arrival, self.seq, msg));
    }

    /// 到 `now` 为止到了的，按到的先后。
    pub fn recv(&mut self, now: f64) -> Vec<T> {
        self.queue.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let n = self.queue.partition_point(|(at, ..)| *at <= now);
        self.queue.drain(..n).map(|(_, _, m)| m).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_drops_and_reorders() {
        let mut net = SimNet::new(0.1, 0.0, 0.0, 1);
        net.send(0.0, 1);
        assert!(net.recv(0.05).is_empty());
        assert_eq!(net.recv(0.1), vec![1]);
        let mut lossy = SimNet::new(0.0, 0.0, 0.5, 7);
        for i in 0..1000 {
            lossy.send(0.0, i);
        }
        let got = lossy.recv(1.0).len();
        assert!((400..600).contains(&got), "{got}");
        let mut shaky = SimNet::new(0.05, 0.1, 0.0, 3);
        for i in 0..100 {
            shaky.send(i as f64 * 0.01, i);
        }
        let order = shaky.recv(10.0);
        assert_eq!(order.len(), 100);
        assert!(shaky.reordered > 0 && order.windows(2).any(|w| w[0] > w[1]), "抖动比间隔大就乱序");
    }
}
