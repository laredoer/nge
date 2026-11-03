//! 渲染线程：独立的帧循环，靠 surface 的 vsync 节奏出帧，不依赖主线程。
//!
//! 主线程（平台层）只负责创建 surface、转发视口变化 / command / prop，
//! 并在 View 离开窗口、App 进后台之前同步暂停渲染线程（[`Engine::pause`]）：
//! wgpu 在 macOS 上取画面时会访问 View 所在的窗口，View 离开窗口后不能再取画面。

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use serde_json::Value as JsonValue;

use crate::bridge::{ENGINE_ERROR, EngineError, FRAME_STATS};
use crate::game::{Ctx, Emit, Game, GameFactory, Gfx, Viewport};
use crate::stats::FrameStats;

/// 单帧 dt 上限，避免卡顿后物理一步跨太远。
const MAX_DT: f32 = 0.05;
/// 拿不到 drawable（被遮挡、最小化）时的退避。
const IDLE_BACKOFF: Duration = Duration::from_millis(16);
/// 等渲染线程确认暂停的上限（正常情况下最多一帧）。
const PAUSE_TIMEOUT: Duration = Duration::from_millis(250);
/// 等渲染线程退出的上限。
const QUIT_TIMEOUT: Duration = Duration::from_millis(500);

/// 主线程发给渲染线程的消息。
pub enum Msg {
    Viewport(Viewport),
    Command(String, JsonValue),
    Prop(String, Option<JsonValue>),
    /// 停止取画面和提交 GPU 命令；进入暂停后回复确认。
    Pause(SyncSender<()>),
    Resume,
    Quit,
}

/// 渲染线程的句柄；drop 时让线程退出并等它结束（保证 surface 先于 View 释放）。
pub struct Engine {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
    done: Receiver<()>,
    /// 渲染线程已进入帧循环。之前它不会碰 View，暂停时不必等确认（GPU 初始化可能要几百毫秒）。
    in_frames: Arc<AtomicBool>,
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
        let thread = std::thread::Builder::new()
            .name("nge-render".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| run(instance, surface, &emit, factory, rx, &flag)));
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
        Ok(Self { tx, thread: Some(thread), done, in_frames, keep_alive: Some(keep_alive) })
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
        // 还没进帧循环：Pause 已排在队列里，帧循环取画面前一定会先处理它
        if !self.in_frames.load(Ordering::SeqCst) {
            return true;
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

struct Surface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// GPU 支持的最大纹理边长，视口超出时截断。
    max_side: u32,
    /// surface 已丢失（需要重建），不再尝试取画面。
    lost: bool,
}

impl Surface {
    fn resize(&mut self, device: &wgpu::Device, pixels: [u32; 2]) {
        let [w, h] = pixels.map(|p| p.clamp(1, self.max_side));
        if [self.config.width, self.config.height] != [w, h] {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(device, &self.config);
        }
    }
}

/// 渲染线程当前的状态。
struct Loop<'a> {
    game: Box<dyn Game>,
    gfx: Gfx,
    surface: Surface,
    emit: &'a Emit,
    rx: Receiver<Msg>,
}

enum Flow {
    Continue,
    Pause(SyncSender<()>),
    Quit,
}

fn run(
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    emit: &Emit,
    factory: GameFactory,
    rx: Receiver<Msg>,
    in_frames: &AtomicBool,
) -> Result<(), String> {
    // 等到第一个有效视口、且没有处于暂停时再初始化 GPU；期间收到的消息先攒着
    let mut pending = Vec::new();
    let mut paused = false;
    let mut viewport = None;
    while paused || viewport.is_none() {
        match rx.recv().map_err(|_| "主线程已断开".to_string())? {
            Msg::Viewport(v) if !v.is_empty() => viewport = Some(v),
            Msg::Viewport(_) => {}
            Msg::Pause(ack) => {
                paused = true;
                let _ = ack.send(());
            }
            Msg::Resume => paused = false,
            Msg::Quit => return Ok(()),
            other => pending.push(other),
        }
    }
    let viewport = viewport.expect("循环保证已有视口");

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: Some(&surface),
        ..Default::default()
    }))
    .map_err(|e| e.to_string())?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&Default::default())).map_err(|e| e.to_string())?;
    device.on_uncaptured_error(Arc::new(|e| log::error!("wgpu 错误: {e}")));

    let max_side = device.limits().max_texture_dimension_2d;
    let [w, h] = viewport.pixels.map(|p| p.clamp(1, max_side));
    let mut config = surface.get_default_config(&adapter, w, h).ok_or("surface 不被该 adapter 支持")?;
    // Fifo：get_current_texture 阻塞到下一个 vsync，渲染线程自己就有了节奏
    config.present_mode = wgpu::PresentMode::Fifo;
    config.desired_maximum_frame_latency = 2;
    surface.configure(&device, &config);

    let gfx = Gfx { device, queue, format: config.format, viewport };
    let game = factory(&gfx);
    let mut state = Loop {
        game,
        gfx,
        surface: Surface { surface, config, max_side, lost: false },
        emit,
        rx,
    };
    for msg in pending {
        state.apply(msg);
    }
    // 先置标志再取消息：主线程看到 false 时发出的 Pause 一定会被下面第一次取到
    in_frames.store(true, Ordering::SeqCst);
    state.frames()
}

impl Loop<'_> {
    fn frames(&mut self) -> Result<(), String> {
        let mut last = Instant::now();
        let mut stats = FrameStats::new(last);
        loop {
            // 逐条取：暂停期间 paused() 直接从 rx 收后续消息，不能先攒到本地
            loop {
                match self.rx.try_recv() {
                    Ok(msg) => {
                        if !self.handle(msg, &mut last, &mut stats) {
                            return Ok(());
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok(()),
                }
            }

            let now = Instant::now();
            let dt = (now - last).as_secs_f32().min(MAX_DT);
            last = now;
            self.game.update(&mut Ctx::new(self.gfx.viewport, self.emit), dt);

            if self.draw() {
                if let Some(report) = stats.frame(Instant::now()) {
                    (self.emit)(FRAME_STATS.name(), FRAME_STATS.encode(&report));
                }
                continue;
            }
            // 拿不到画面时不空转；同时还能及时响应消息
            match self.rx.recv_timeout(IDLE_BACKOFF) {
                Ok(msg) => {
                    if !self.handle(msg, &mut last, &mut stats) {
                        return Ok(());
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// 处理一条消息；返回 false 表示该退出了。
    fn handle(&mut self, msg: Msg, last: &mut Instant, stats: &mut FrameStats) -> bool {
        match self.apply(msg) {
            Flow::Continue => true,
            Flow::Quit => false,
            Flow::Pause(ack) => {
                if !self.paused(ack) {
                    return false;
                }
                // 暂停期间的时间不计入 dt，也不计入帧统计
                *last = Instant::now();
                *stats = FrameStats::new(*last);
                true
            }
        }
    }

    /// 暂停：确认后阻塞到 Resume。返回 false 表示期间收到了 Quit。
    fn paused(&mut self, ack: SyncSender<()>) -> bool {
        let _ = ack.send(());
        loop {
            match self.rx.recv() {
                Ok(Msg::Resume) => return true,
                Ok(Msg::Quit) | Err(_) => return false,
                Ok(Msg::Pause(ack)) => {
                    let _ = ack.send(());
                }
                Ok(msg) => {
                    self.apply(msg);
                }
            }
        }
    }

    fn apply(&mut self, msg: Msg) -> Flow {
        let ctx = &mut Ctx::new(self.gfx.viewport, self.emit);
        match msg {
            Msg::Viewport(v) => {
                if !v.is_empty() && v != self.gfx.viewport {
                    self.gfx.viewport = v;
                    self.surface.resize(&self.gfx.device, v.pixels);
                }
            }
            Msg::Command(name, args) => self.game.command(ctx, &name, &args),
            Msg::Prop(name, value) => self.game.prop(ctx, &name, value.as_ref()),
            Msg::Pause(ack) => return Flow::Pause(ack),
            Msg::Resume => {}
            Msg::Quit => return Flow::Quit,
        }
        Flow::Continue
    }

    /// 绘制一帧；返回 false 表示这一帧没拿到画面。
    fn draw(&mut self) -> bool {
        if self.surface.lost {
            return false;
        }
        let frame = match self.surface.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.surface.configure(&self.gfx.device, &self.surface.config);
                return false;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                // surface 需要重建，这一版先停止渲染并上报
                self.surface.lost = true;
                log::error!("surface 丢失，停止渲染");
                (self.emit)(ENGINE_ERROR.name(), ENGINE_ERROR.encode(&EngineError { message: "surface lost".into() }));
                return false;
            }
            _ => return false,
        };
        let target = frame.texture.create_view(&Default::default());
        let mut encoder = self.gfx.device.create_command_encoder(&Default::default());
        self.game.render(&self.gfx, &mut encoder, &target);
        self.gfx.queue.submit([encoder.finish()]);
        self.gfx.queue.present(frame);
        true
    }
}
