//! wasm 上引擎的阶段（`web.rs` 用）：等 GPU → 等视口 → 跑起来 → 停下。和 wgpu 无关，纯逻辑，原生上也测。
//!
//! 浏览器每帧把攒下的消息交给 [`Phase::frame`]；GPU 初始化完了调 [`Phase::gpu_ready`]。

use web_time::Instant;

use crate::Viewport;
use crate::frame::{Flow, Gate, Msg};

/// 跑起来的游戏要会的事（真的是 `frame::Core`，测试里是假的）。
pub(crate) trait Run {
    fn apply(&mut self, msg: Msg) -> Flow;
    /// 暂停、隐藏回来：不把这段时间算进 dt。
    fn resumed(&mut self);
    fn step(&mut self, start: Instant) -> bool;
}

pub(crate) enum Phase<G, C> {
    /// 等 GPU。
    Starting(Gate),
    /// GPU 好了，等视口（或者暂停着）。
    Ready(G, Gate),
    /// `hidden`：画布被藏起来了（视口是空的），不更新也不画。
    Running { core: C, paused: bool, hidden: bool },
    /// 出错了或者收到了 `Quit`，什么都不做。
    Stopped,
}

impl<G, C: Run> Phase<G, C> {
    pub fn running(&self) -> bool {
        matches!(self, Phase::Running { .. })
    }

    /// GPU 初始化完了（成功或失败）。返回新的阶段和要报的错。
    pub fn gpu_ready(self, gpu: Result<G, String>) -> (Self, Option<String>) {
        match (self, gpu) {
            (Phase::Starting(gate), Ok(gpu)) => (Phase::Ready(gpu, gate), None),
            (Phase::Starting(_), Err(e)) => (Phase::Stopped, Some(e)),
            // 已经退出了：GPU 不要了
            (phase, _) => (phase, None),
        }
    }

    /// 浏览器的一帧：处理这一帧的消息，跑起来了就更新、画。`begin` 用 GPU 和视口建游戏。返回新的阶段和要报的错。
    pub fn frame(self, msgs: Vec<Msg>, start: Instant, begin: impl FnOnce(G, Viewport) -> Result<C, String>) -> (Self, Option<String>) {
        match self {
            Phase::Starting(mut gate) => {
                let alive = msgs.into_iter().all(|m| gate.push(m));
                (if alive { Phase::Starting(gate) } else { Phase::Stopped }, None)
            }
            Phase::Ready(gpu, mut gate) => {
                if !msgs.into_iter().all(|m| gate.push(m)) {
                    return (Phase::Stopped, None);
                }
                let Some(viewport) = gate.ready() else { return (Phase::Ready(gpu, gate), None) };
                match begin(gpu, viewport) {
                    Ok(mut core) => {
                        for msg in gate.take() {
                            core.apply(msg);
                        }
                        (Phase::Running { core, paused: false, hidden: false }, None)
                    }
                    Err(e) => (Phase::Stopped, Some(e)),
                }
            }
            Phase::Running { core, paused, hidden } => (run(core, paused, hidden, msgs, start), None),
            Phase::Stopped => (Phase::Stopped, None),
        }
    }
}

fn run<G, C: Run>(mut core: C, mut paused: bool, mut hidden: bool, msgs: Vec<Msg>, start: Instant) -> Phase<G, C> {
    let was_idle = paused || hidden;
    for msg in msgs {
        match msg {
            Msg::Viewport(v) if v.is_empty() => hidden = true,
            Msg::Viewport(v) => {
                hidden = false;
                core.apply(Msg::Viewport(v));
            }
            Msg::Resume => paused = false,
            msg => match core.apply(msg) {
                Flow::Continue => {}
                Flow::Pause(ack) => {
                    paused = true;
                    let _ = ack.send(());
                }
                Flow::Quit => return Phase::Stopped,
            },
        }
    }
    if paused || hidden {
        return Phase::Running { core, paused, hidden };
    }
    if was_idle {
        core.resumed();
    }
    core.step(start);
    Phase::Running { core, paused, hidden }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use serde_json::Value as JsonValue;

    use super::*;

    const VIEW: Viewport = Viewport { size: [10.0, 10.0], pixels: [20, 20] };

    /// 假的游戏：记下发生了什么。
    #[derive(Default)]
    struct Fake {
        log: Vec<String>,
    }

    impl Run for Fake {
        fn apply(&mut self, msg: Msg) -> Flow {
            match msg {
                Msg::Viewport(v) => self.log.push(format!("viewport {}", v.pixels[0])),
                Msg::Prop(n, _) => self.log.push(format!("prop {n}")),
                Msg::Command(n, _) => self.log.push(format!("command {n}")),
                Msg::Pause(ack) => return Flow::Pause(ack),
                Msg::Resume => self.log.push("resume".into()),
                Msg::Quit => return Flow::Quit,
            }
            Flow::Continue
        }
        fn resumed(&mut self) {
            self.log.push("resumed".into());
        }
        fn step(&mut self, _: Instant) -> bool {
            self.log.push("step".into());
            true
        }
    }

    type P = Phase<(), Fake>;

    fn frame(p: P, msgs: Vec<Msg>) -> P {
        p.frame(msgs, Instant::now(), |(), _| Ok(Fake::default())).0
    }

    fn log(p: &P) -> &[String] {
        match p {
            Phase::Running { core, .. } => &core.log,
            _ => panic!("没跑起来"),
        }
    }

    fn running() -> P {
        let p = P::Starting(Gate::default());
        let p = frame(p, vec![Msg::Prop("scene".into(), None), Msg::Viewport(VIEW)]);
        let (p, err) = p.gpu_ready(Ok(()));
        assert!(err.is_none());
        frame(p, vec![Msg::Command("burst".into(), JsonValue::Null)])
    }

    #[test]
    fn starts_after_gpu_and_viewport_with_what_came_before() {
        let p = running();
        assert!(p.running());
        assert_eq!(log(&p), ["prop scene", "command burst"], "攒着的按顺序交给游戏，开始那一帧不画");
        let p = frame(p, vec![]);
        assert_eq!(log(&p).last().unwrap(), "step");
    }

    #[test]
    fn waits_for_a_viewport_after_the_gpu() {
        let (p, _) = P::Starting(Gate::default()).gpu_ready(Ok(()));
        let p = frame(p, vec![Msg::Viewport(Viewport::default())]);
        assert!(matches!(p, Phase::Ready(..)), "空视口不开始");
    }

    #[test]
    fn gpu_failure_stops_and_is_reported() {
        let (p, err) = P::Starting(Gate::default()).gpu_ready(Err("没有适配器".into()));
        assert!(matches!(p, Phase::Stopped));
        assert_eq!(err.as_deref(), Some("没有适配器"));
        let (p, err) = frame(p, vec![]).gpu_ready(Ok(()));
        assert!(matches!(p, Phase::Stopped) && err.is_none(), "停了以后 GPU 来了也不理");
    }

    #[test]
    fn failing_to_begin_is_reported() {
        let (p, _) = P::Starting(Gate::default()).gpu_ready(Ok(()));
        let (p, err) = p.frame(vec![Msg::Viewport(VIEW)], Instant::now(), |(), _| Err("配置不了画面".into()));
        assert!(matches!(p, Phase::Stopped));
        assert_eq!(err.as_deref(), Some("配置不了画面"));
    }

    #[test]
    fn quit_while_starting_or_running_stops() {
        assert!(matches!(frame(P::Starting(Gate::default()), vec![Msg::Quit]), Phase::Stopped));
        assert!(matches!(frame(running(), vec![Msg::Quit]), Phase::Stopped));
    }

    #[test]
    fn pause_stops_stepping_and_resume_zeroes_the_gap() {
        let (tx, rx) = mpsc::sync_channel(1);
        let p = frame(running(), vec![Msg::Pause(tx)]);
        assert_eq!(rx.try_recv(), Ok(()), "暂停当场确认");
        let p = frame(p, vec![Msg::Command("x".into(), JsonValue::Null)]);
        assert!(!log(&p).ends_with(&["step".to_owned()]), "暂停时不画");
        let p = frame(p, vec![Msg::Resume]);
        assert!(log(&p).ends_with(&["resumed".to_owned(), "step".to_owned()]), "回来先清掉暂停那段时间再走：{:?}", log(&p));
    }

    #[test]
    fn hidden_canvas_skips_steps_and_coming_back_zeroes_the_gap() {
        let p = frame(running(), vec![Msg::Viewport(Viewport::default())]);
        let p = frame(p, vec![]);
        let n = log(&p).len();
        assert!(!log(&p).ends_with(&["step".to_owned()]), "藏起来时不更新不画");
        let p = frame(p, vec![Msg::Viewport(VIEW)]);
        assert_eq!(log(&p)[n..], ["viewport 20", "resumed", "step"]);
    }
}
