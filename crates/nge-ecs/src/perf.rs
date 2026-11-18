//! 性能面板的数据：每半秒汇总一次帧率、各段耗时、draw call、实体数（见 docs/perf-overlay.md）。

use bevy_ecs::prelude::Resource;
pub use nge_core::FrameTiming;

/// 多久汇总一次（秒）。
pub const PERF_WINDOW: f32 = 0.5;

/// 一个窗口的汇总。耗时是平均值（毫秒），draw call、实例、实体是窗口里最后一帧的。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PerfReport {
    pub fps: f32,
    /// 平均帧间隔。
    pub frame_ms: f32,
    /// 最长的帧间隔。
    pub worst_ms: f32,
    /// 跑系统（更新）。
    pub update_ms: f32,
    /// 录制、提交渲染命令。
    pub render_ms: f32,
    /// 等画面（拿下一张画面、交给系统显示），通常是在等垂直同步。
    pub wait_ms: f32,
    pub draws: u32,
    pub instances: u32,
    pub entities: u32,
    /// 3D 粒子数（包括子发射器的），窗口里最后一帧的。
    pub particles: u32,
}

/// 3D 粒子这一帧的统计（3D 粒子系统每帧写，见 docs/particles3d-more.md）。
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParticleStats {
    /// 存活的粒子（包括子发射器的）。
    pub alive: u32,
    /// 到了总数上限没发出去的。
    pub dropped: u32,
    /// 查了几次场景碰撞体。
    pub queries: u32,
}

/// 资源：最近一个窗口的汇总。引擎每帧画完后记一笔，暂停期间不记。
#[derive(Resource, Debug, Default)]
pub struct Perf {
    report: Option<PerfReport>,
    frames: u32,
    sum: FrameTiming,
    worst: f32,
    particles: u32,
}

impl Perf {
    /// 最近一个窗口的汇总；还没满一个窗口时是 `None`。
    pub fn report(&self) -> Option<&PerfReport> {
        self.report.as_ref()
    }

    /// 记下这一帧的粒子数，下一次 `record` 出汇总时用。
    pub fn particles(&mut self, alive: u32) {
        self.particles = alive;
    }

    /// 记一帧；满一个窗口就出汇总、开始下一个窗口。实体数要数一遍，只在出汇总时才调 `entities`。
    /// 帧间隔不是有限数、是负数的这一帧不算。
    pub fn record(&mut self, timing: FrameTiming, draws: u32, instances: u32, entities: impl FnOnce() -> u32) {
        if !(timing.frame.is_finite() && timing.frame >= 0.0) {
            return;
        }
        let t = timing.sanitized();
        self.frames += 1;
        self.sum = FrameTiming { frame: self.sum.frame + t.frame, update: self.sum.update + t.update, render: self.sum.render + t.render, wait: self.sum.wait + t.wait };
        self.worst = self.worst.max(t.frame);
        if self.sum.frame < PERF_WINDOW {
            return;
        }
        let n = self.frames as f32;
        let ms = |seconds: f32| seconds * 1000.0 / n;
        self.report = Some(PerfReport {
            fps: n / self.sum.frame,
            frame_ms: ms(self.sum.frame),
            worst_ms: self.worst * 1000.0,
            update_ms: ms(self.sum.update),
            render_ms: ms(self.sum.render),
            wait_ms: ms(self.sum.wait),
            draws,
            instances,
            entities: entities(),
            particles: self.particles,
        });
        *self = Self { report: self.report, ..Self::default() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(frame: f32) -> FrameTiming {
        FrameTiming { frame, update: 0.002, render: 0.003, wait: frame - 0.005 }
    }

    #[test]
    fn reports_averages_once_a_window_is_full() {
        let mut perf = Perf::default();
        for i in 0..29 {
            perf.record(frame(1.0 / 60.0), i, 0, || 0);
        }
        assert!(perf.report().is_none(), "还没满半秒");
        perf.particles(77);
        perf.record(frame(1.0 / 60.0), 42, 900, || 300);
        let r = *perf.report().expect("满半秒了");
        assert_eq!(r.particles, 77);
        assert!((r.fps - 60.0).abs() < 0.1, "{r:?}");
        assert!((r.frame_ms - 16.667).abs() < 0.01 && (r.worst_ms - 16.667).abs() < 0.01, "{r:?}");
        assert!((r.update_ms - 2.0).abs() < 1e-3 && (r.render_ms - 3.0).abs() < 1e-3 && (r.wait_ms - 11.667).abs() < 0.01, "{r:?}");
        assert_eq!((r.draws, r.instances, r.entities), (42, 900, 300), "最后一帧的");
    }

    #[test]
    fn a_stall_shows_as_the_worst_frame_and_the_next_window_starts_fresh() {
        let mut perf = Perf::default();
        perf.record(frame(0.1), 0, 0, || 0);
        perf.record(frame(0.3), 0, 0, || 0);
        perf.record(frame(0.1), 0, 0, || 0);
        let r = *perf.report().unwrap();
        assert!((r.worst_ms - 300.0).abs() < 0.01 && (r.fps - 6.0).abs() < 0.01, "{r:?}");
        // 下一个窗口不带上一个的最长帧
        for _ in 0..30 {
            perf.record(frame(1.0 / 60.0), 0, 0, || 0);
        }
        let r = *perf.report().unwrap();
        assert!((r.worst_ms - 16.667).abs() < 0.01 && (r.fps - 60.0).abs() < 0.1, "{r:?}");
    }

    #[test]
    fn bad_timings_count_as_zero_and_a_bad_frame_is_skipped() {
        let mut perf = Perf::default();
        perf.record(FrameTiming { frame: f32::NAN, update: 0.1, render: 0.1, wait: 0.0 }, 0, 0, || 0);
        perf.record(FrameTiming { frame: 0.3, update: -1.0, render: f32::INFINITY, wait: 0.2 }, 0, 0, || 0);
        perf.record(FrameTiming { frame: 0.3, update: 0.002, render: 0.001, wait: 0.2 }, 0, 0, || 0);
        let r = *perf.report().unwrap();
        assert!((r.fps - 1.0 / 0.3).abs() < 1e-3, "帧间隔坏了的那帧不算 {r:?}");
        assert!((r.update_ms - 1.0).abs() < 1e-3 && (r.render_ms - 0.5).abs() < 1e-3, "坏的分段当 0 {r:?}");
    }

    #[test]
    fn entities_are_counted_only_when_a_report_comes_out() {
        let mut perf = Perf::default();
        let mut counted = 0;
        perf.record(frame(0.2), 0, 0, || {
            counted += 1;
            1
        });
        perf.record(frame(0.4), 0, 0, || {
            counted += 1;
            2
        });
        assert_eq!((counted, perf.report().unwrap().entities), (1, 2));
        // 刚出完汇总的下一帧：汇总不变、重新累计
        perf.record(frame(0.1), 9, 9, || 9);
        assert_eq!(perf.report().unwrap().entities, 2);
    }
}
