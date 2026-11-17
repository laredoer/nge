//! 渲染线程（原生）：独立的帧循环，靠 surface 的 vsync 节奏出帧，不依赖主线程。一帧做什么见 `frame.rs`。
//!
//! 主线程（平台层）只负责创建 surface、转发视口变化 / command / prop，
//! 并在 View 离开窗口、App 进后台之前同步暂停渲染线程（[`Engine::pause`]）：
//! wgpu 在 macOS 上取画面时会访问 View 所在的窗口，View 离开窗口后不能再取画面。

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::thread::JoinHandle;
use std::time::Duration;

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use web_time::Instant;

use crate::bridge::{ENGINE_ERROR, EngineError};
use crate::frame::{Core, Flow, Gate, Msg, configure, request_gpu};
use crate::startup::{start, wait_unlocked};
use crate::game::{Emit, GameFactory};

/// 拿不到 drawable（被遮挡、最小化）时的退避。
const IDLE_BACKOFF: Duration = Duration::from_millis(16);
/// 等渲染线程确认暂停的上限（正常情况下最多一帧）。
const PAUSE_TIMEOUT: Duration = Duration::from_millis(250);
/// 还没进帧循环时，等渲染线程用完画面（请求 GPU、配置）的上限。
const START_PAUSE_TIMEOUT: Duration = Duration::from_secs(2);
/// 等渲染线程退出的上限。
const QUIT_TIMEOUT: Duration = Duration::from_millis(500);

/// 渲染线程的句柄；drop 时让线程退出并等它结束（保证 surface 先于 View 释放）。
pub struct Engine {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
    done: Receiver<()>,
    /// 渲染线程已进入帧循环：暂停时等它确认。
    in_frames: Arc<AtomicBool>,
    /// 进帧循环之前，渲染线程只在拿着它时碰画面（请求 GPU、配置），拿到后先看一遍消息。
    window: Arc<Mutex<()>>,
    /// 渲染线程退出超时时故意泄漏的对象（通常是 View），避免它在线程还在用时被释放。
    keep_alive: Option<Box<dyn Any>>,
}

/// 在平台 View 上建 surface。必须在主线程调用（wgpu 会往 View 上挂 CAMetalLayer）。
///
/// # Safety
/// `window` / `display` 指向的 View 必须比返回的 surface（也就是 [`Engine`]）活得久。
pub unsafe fn create_surface(
    window: RawWindowHandle,
    display: RawDisplayHandle,
) -> Result<(wgpu::Instance, wgpu::Surface<'static>), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let target = wgpu::SurfaceTargetUnsafe::RawHandle {
        raw_display_handle: Some(display),
        raw_window_handle: window,
    };
    let surface = unsafe { instance.create_surface_unsafe(target) }.map_err(|e| e.to_string())?;
    Ok((instance, surface))
}

impl Engine {
    /// 启动渲染线程。`keep_alive` 是 surface 所在的 View：退出超时时把它泄漏掉，
    /// 不让它先于渲染线程释放。
    pub fn spawn(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        emit: Emit,
        factory: GameFactory,
        keep_alive: Box<dyn Any>,
    ) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let (done_tx, done) = mpsc::sync_channel(1);
        let in_frames = Arc::new(AtomicBool::new(false));
        let flag = in_frames.clone();
        let window = Arc::new(Mutex::new(()));
        let touching = window.clone();
        let thread = std::thread::Builder::new()
            .name("nge-render".into())
            .spawn(move || {
                let emit = Rc::new(emit);
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| run(instance, surface, emit.clone(), factory, rx, &flag, &touching)));
                let error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(e)) => Some(e),
                    Err(panic) => Some(panic_message(&panic)),
                };
                if let Some(e) = error {
                    log::error!("渲染线程退出: {e}");
                    emit(ENGINE_ERROR.name(), ENGINE_ERROR.encode(&EngineError { message: e }));
                }
                let _ = done_tx.send(());
            })
            .map_err(|e| format!("创建渲染线程失败: {e}"))?;
        Ok(Self { tx, thread: Some(thread), done, in_frames, window, keep_alive: Some(keep_alive) })
    }

    pub fn send(&self, msg: Msg) {
        // 线程已退出（初始化失败）时丢弃即可
        let _ = self.tx.send(msg);
    }

    /// 同步暂停：返回时渲染线程已经不在取画面 / 提交了。超时返回 false。
    pub fn pause(&self) -> bool {
        let (ack_tx, ack) = mpsc::sync_channel(1);
        if self.tx.send(Msg::Pause(ack_tx)).is_err() {
            return true; // 线程已退出，本来就不在渲染
        }
        // 还没进帧循环：等它用完画面。Pause 已排在队列里，它再拿锁、进帧循环取画面前都会先看到
        if !self.in_frames.load(Ordering::SeqCst) {
            if wait_unlocked(&self.window, START_PAUSE_TIMEOUT) {
                return true;
            }
            log::error!("渲染线程 {}ms 内没有用完画面", START_PAUSE_TIMEOUT.as_millis());
            return false;
        }
        match ack.recv_timeout(PAUSE_TIMEOUT) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => true,
            Err(RecvTimeoutError::Timeout) => {
                log::error!("渲染线程 {}ms 内没有确认暂停", PAUSE_TIMEOUT.as_millis());
                false
            }
        }
    }

    pub fn resume(&self) {
        self.send(Msg::Resume);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Quit);
        let Some(thread) = self.thread.take() else { return };
        match self.done.recv_timeout(QUIT_TIMEOUT) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                let _ = thread.join();
                log::debug!("渲染线程已退出");
            }
            Err(RecvTimeoutError::Timeout) => {
                // 线程卡在取画面上：不 join，也不释放 View，宁可泄漏也不卡死主线程或崩溃
                log::error!("渲染线程 {}ms 内没有退出，泄漏它和 View", QUIT_TIMEOUT.as_millis());
                std::mem::forget(thread);
                std::mem::forget(self.keep_alive.take());
            }
        }
    }
}

fn panic_message(panic: &Box<dyn Any + Send>) -> String {
    let detail = panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    format!("渲染线程 panic: {detail}")
}

/// 渲染线程当前的状态。
struct Loop {
    core: Core,
    rx: Receiver<Msg>,
}

fn run(
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    emit: Rc<Emit>,
    factory: GameFactory,
    rx: Receiver<Msg>,
    in_frames: &AtomicBool,
    window: &Mutex<()>,
) -> Result<(), String> {
    // 等到第一个有效视口、且没有处于暂停时再初始化 GPU；期间收到的消息先攒着，换过画面（Android 切了一次后台）就用新的
    let mut gate = Gate::default();
    let started = start(
        &mut gate,
        &rx,
        window,
        surface,
        |surface| pollster::block_on(request_gpu(&instance, surface)),
        |gpu, surface, viewport| configure(gpu.with_surface(surface), viewport),
    )?;
    let Some((gfx, surface, cache)) = started else { return Ok(()) };
    let game = factory(&gfx);
    let mut state = Loop { core: Core::new(game, gfx, surface, emit, cache), rx };
    for msg in gate.take() {
        state.core.apply(msg);
    }
    // 先置标志再取消息：主线程看到 false 时发出的 Pause 一定会被下面第一次取到
    in_frames.store(true, Ordering::SeqCst);
    state.frames()
}

impl Loop {
    fn frames(&mut self) -> Result<(), String> {
        loop {
            // 处理 NSH 发来的消息（场景 JSON 解析等）也算在更新里
            let start = Instant::now();
            // 逐条取：暂停期间 paused() 直接从 rx 收后续消息，不能先攒到本地
            loop {
                match self.rx.try_recv() {
                    Ok(msg) => {
                        if !self.handle(msg) {
                            return Ok(());
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok(()),
                }
            }
            if self.core.step(start) {
                continue;
            }
            // 拿不到画面时不空转；同时还能及时响应消息
            match self.rx.recv_timeout(IDLE_BACKOFF) {
                Ok(msg) => {
                    if !self.handle(msg) {
                        return Ok(());
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// 处理一条消息；返回 false 表示该退出了。
    fn handle(&mut self, msg: Msg) -> bool {
        match self.core.apply(msg) {
            Flow::Continue => true,
            Flow::Quit => false,
            Flow::Pause(ack) => {
                if !self.paused(ack) {
                    return false;
                }
                self.core.resumed();
                true
            }
        }
    }

    /// 暂停：确认后阻塞到 Resume。返回 false 表示期间收到了 Quit。
    fn paused(&mut self, ack: SyncSender<()>) -> bool {
        // 先确认再存管线缓存：写盘不挡着平台层
        let _ = ack.send(());
        self.core.save_pipeline_cache();
        loop {
            match self.rx.recv() {
                Ok(Msg::Resume) => return true,
                Ok(Msg::Quit) | Err(_) => return false,
                Ok(Msg::Pause(ack)) => {
                    let _ = ack.send(());
                }
                Ok(msg) => {
                    self.core.apply(msg);
                }
            }
        }
    }
}
