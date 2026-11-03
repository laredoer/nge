//! 帧统计：每秒汇总一次帧率和最长帧间隔，用来衡量卡顿。

use std::time::{Duration, Instant};

/// 一个统计窗口的结果，每秒通过 [`crate::bridge::FRAME_STATS`] 发给 NSH。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct FrameReport {
    pub fps: u32,
    /// 窗口内最长的两帧间隔（毫秒）。
    pub worst_ms: u32,
}

pub struct FrameStats {
    window: Duration,
    since: Instant,
    last: Option<Instant>,
    frames: u32,
    worst: Duration,
}

impl FrameStats {
    pub fn new(now: Instant) -> Self {
        Self::with_window(now, Duration::from_secs(1))
    }

    pub fn with_window(now: Instant, window: Duration) -> Self {
        Self { window, since: now, last: None, frames: 0, worst: Duration::ZERO }
    }

    /// 记录一帧；窗口满了就返回汇总并开始下一个窗口。
    pub fn frame(&mut self, now: Instant) -> Option<FrameReport> {
        if let Some(last) = self.last {
            self.worst = self.worst.max(now - last);
        }
        self.last = Some(now);
        self.frames += 1;

        let elapsed = now - self.since;
        if elapsed < self.window {
            return None;
        }
        let report = FrameReport {
            fps: (self.frames as f64 / elapsed.as_secs_f64()).round() as u32,
            worst_ms: self.worst.as_millis() as u32,
        };
        self.since = now;
        self.frames = 0;
        self.worst = Duration::ZERO;
        Some(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn steady_60fps_reports_60_and_small_gap() {
        let t0 = Instant::now();
        let mut stats = FrameStats::new(t0);
        let mut report = None;
        for i in 1..=60 {
            report = stats.frame(t0 + Duration::from_micros(16_667 * i)).or(report);
        }
        let report = report.expect("一秒后应有汇总");
        assert_eq!(report.fps, 60);
        assert!(report.worst_ms <= 17, "{report:?}");
    }

    #[test]
    fn stall_shows_up_as_worst_gap() {
        let t0 = Instant::now();
        let mut stats = FrameStats::new(t0);
        assert_eq!(stats.frame(t0 + ms(10)), None);
        assert_eq!(stats.frame(t0 + ms(20)), None);
        let report = stats.frame(t0 + ms(1020)).expect("跨过窗口应汇总");
        assert_eq!(report.worst_ms, 1000);
        assert_eq!(report.fps, 3);
    }

    #[test]
    fn window_resets_after_report() {
        let t0 = Instant::now();
        let mut stats = FrameStats::new(t0);
        stats.frame(t0 + ms(500));
        assert!(stats.frame(t0 + ms(1000)).is_some());
        assert_eq!(stats.frame(t0 + ms(1010)), None);
        let report = stats.frame(t0 + ms(2000)).unwrap();
        assert_eq!(report.worst_ms, 990, "上个窗口的间隔不应带入");
    }
}
