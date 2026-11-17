//! 渲染线程（原生）起 GPU 的那一段：请求适配器、配置画面都要碰画面，这期间平台层可能暂停、换画面
//! （Android 刚启动就按了 Home）。纯逻辑：画面、GPU、配置结果都是类型参数，测试里用数字代替（见 docs/android.md）。

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use web_time::Instant;

use crate::frame::{Gate, Msg};
use crate::game::Viewport;

/// 一直收消息，直到能开始（有视口、没暂停、画面能用）；收到 `Quit` 返回 `None`。
fn wait<S>(gate: &mut Gate<S>, rx: &Receiver<Msg<S>>) -> Result<Option<Viewport>, String> {
    loop {
        if let Some(viewport) = gate.ready() {
            return Ok(Some(viewport));
        }
        if !gate.push(rx.recv().map_err(|_| "主线程已断开".to_string())?) {
            return Ok(None);
        }
    }
}

/// 已经到了的消息都过一遍；收到 `Quit` 返回 false。
fn drain<S>(gate: &mut Gate<S>, rx: &Receiver<Msg<S>>) -> bool {
    loop {
        match rx.try_recv() {
            Ok(msg) => {
                if !gate.push(msg) {
                    return false;
                }
            }
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

/// 起 GPU：等到能开始，拿着 `window` 再看一遍消息，用最新的画面请求 GPU（只请求一次）；请求期间到的消息再过一遍，
/// 暂停了就等回来、换了画面就换上，再配置。配置失败时留着 GPU，等下一个画面再配。收到 `Quit` 返回 `None`。
///
/// 只在拿着 `window` 时碰画面：平台层暂停时等它放开（[`wait_unlocked`]），之后的消息拿锁以后一定会先看到。
pub(crate) fn start<S, G, R>(
    gate: &mut Gate<S>,
    rx: &Receiver<Msg<S>>,
    window: &Mutex<()>,
    surface: S,
    request: impl FnOnce(S) -> Result<G, String>,
    mut configure: impl FnMut(G, Option<S>, Viewport) -> Result<R, (G, String)>,
) -> Result<Option<R>, String> {
    let mut surface = Some(surface);
    let mut request = Some(request);
    let mut gpu = None;
    loop {
        if wait(gate, rx)?.is_none() {
            return Ok(None);
        }
        let _touching = window.lock().unwrap_or_else(|e| e.into_inner());
        if !drain(gate, rx) {
            return Ok(None);
        }
        if gate.ready().is_none() {
            continue;
        }
        if gpu.is_none() {
            let (Some(first), Some(request)) = (gate.take_surface().or_else(|| surface.take()), request.take()) else {
                return Err("没有画面".into());
            };
            gpu = Some(request(first)?);
            // 请求 GPU 期间到的消息
            if !drain(gate, rx) {
                return Ok(None);
            }
        }
        let Some(viewport) = gate.ready() else { continue };
        let Some(g) = gpu.take() else { continue };
        match configure(g, gate.take_surface(), viewport) {
            Ok(r) => return Ok(Some(r)),
            Err((g, e)) => {
                log::warn!("配置画面失败，等下一个画面: {e}");
                gpu = Some(g);
                gate.surface_unusable();
            }
        }
    }
}

/// 等渲染线程放开画面（[`start`] 里拿着的锁），最多 `timeout`；超时返回 false。
pub(crate) fn wait_unlocked(window: &Mutex<()>, timeout: Duration) -> bool {
    let until = Instant::now() + timeout;
    loop {
        match window.try_lock() {
            Ok(_) | Err(std::sync::TryLockError::Poisoned(_)) => return true,
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    const VIEW: Viewport = Viewport { size: [100.0, 50.0], pixels: [200, 100] };

    /// 测试里的 GPU：记着用的是哪个画面。
    #[derive(Debug, PartialEq)]
    struct Gpu(u32);

    /// 起一次：`during` 是请求 GPU 时到的消息，`configures` 依次是每次配置的结果（true 成功），
    /// 配置失败后过一会儿到 `on_fail` 的消息。返回（结果、请求了几次、每次配置用的画面）。在别的线程跑，卡住了算失败。
    fn run(during: Vec<Msg<u32>>, on_fail: Vec<Msg<u32>>, configures: Vec<bool>) -> (Option<u32>, usize, Vec<u32>) {
        let (out_tx, out_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (tx, rx) = mpsc::channel();
            tx.send(Msg::Viewport(VIEW)).unwrap();
            let (mut during, mut on_fail) = (Some(during), Some(on_fail));
            let (mut requests, mut used, mut results) = (0, Vec::new(), configures.into_iter());
            let tx2 = tx.clone();
            let out = start(
                &mut Gate::default(),
                &rx,
                &Mutex::new(()),
                1,
                |surface| {
                    requests += 1;
                    for m in during.take().unwrap_or_default() {
                        tx.send(m).unwrap();
                    }
                    Ok(Gpu(surface))
                },
                |mut gpu, new, viewport| {
                    assert_eq!(viewport, VIEW);
                    if let Some(s) = new {
                        gpu.0 = s;
                    }
                    used.push(gpu.0);
                    if results.next().unwrap_or(true) {
                        return Ok(gpu.0);
                    }
                    // 过一会儿才到：失败以后要等，不能拿着旧画面一直重试
                    let later = on_fail.take().unwrap_or_default();
                    let tx = tx2.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(30));
                        for m in later {
                            let _ = tx.send(m);
                        }
                    });
                    Err((gpu, "没有格式".into()))
                },
            )
            .unwrap();
            let _ = out_tx.send((out, requests, used));
        });
        out_rx.recv_timeout(Duration::from_secs(5)).expect("起 GPU 卡住了")
    }

    #[test]
    fn starts_on_the_first_surface() {
        assert_eq!(run(vec![], vec![], vec![true]), (Some(1), 1, vec![1]));
    }

    #[test]
    fn paused_while_requesting_the_gpu_does_not_configure_until_resumed() {
        let (ack, acked) = mpsc::sync_channel(1);
        let out = run(vec![Msg::Pause(ack), Msg::Quit], vec![], vec![true]);
        assert_eq!(out, (None, 1, vec![]), "暂停着收到 Quit：没配置过就退出");
        assert_eq!(acked.try_recv(), Ok(()), "暂停当场确认");
        let (ack, _) = mpsc::sync_channel(1);
        assert_eq!(run(vec![Msg::Pause(ack), Msg::Resume], vec![], vec![true]), (Some(1), 1, vec![1]), "回来了照常配置");
    }

    #[test]
    fn a_surface_that_came_while_requesting_the_gpu_is_the_one_configured() {
        let (ack, _) = mpsc::sync_channel(1);
        let during = vec![Msg::Pause(ack), Msg::Surface(2), Msg::Resume];
        assert_eq!(run(during, vec![], vec![true]), (Some(2), 1, vec![2]));
    }

    #[test]
    fn a_failed_configure_keeps_the_gpu_and_waits_for_the_next_surface() {
        assert_eq!(run(vec![], vec![Msg::Surface(3)], vec![false, true]), (Some(3), 1, vec![1, 3]), "GPU 只请求一次");
        assert_eq!(run(vec![], vec![Msg::Quit], vec![false]), (None, 1, vec![1]), "等的时候退出不算出错");
    }

    #[test]
    fn quit_before_anything_starts_nothing() {
        let (tx, rx) = mpsc::channel::<Msg<u32>>();
        tx.send(Msg::Quit).unwrap();
        let out = start(&mut Gate::default(), &rx, &Mutex::new(()), 1, |_| -> Result<Gpu, String> { panic!("不该请求") }, |g, _, _| Err((g, String::new())));
        assert_eq!(out, Ok(None::<u32>));
    }

    #[test]
    fn pausing_before_the_frames_waits_while_the_window_is_in_use() {
        let window = Mutex::new(());
        assert!(wait_unlocked(&window, Duration::from_millis(10)), "没人用：马上返回");
        std::thread::scope(|s| {
            let (tx, rx) = mpsc::channel();
            let window = &window;
            s.spawn(move || {
                let _held = window.lock().unwrap();
                tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(30));
            });
            rx.recv().unwrap();
            let t = Instant::now();
            assert!(wait_unlocked(&window, Duration::from_secs(2)));
            assert!(t.elapsed() >= Duration::from_millis(20), "等到用完");
        });
        let _held = window.lock().unwrap();
        assert!(!wait_unlocked(&window, Duration::from_millis(30)), "一直在用：超时");
    }
}
