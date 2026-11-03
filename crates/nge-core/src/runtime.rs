//! 渲染线程：独立的帧循环，靠 surface 的 vsync 节奏出帧，不依赖主线程。
//!
//! 主线程（平台层）只负责创建 surface、转发视口变化 / command / prop。
//! 主线程卡住时，渲染线程照常推进和出帧。

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use serde_json::{Value as JsonValue, json};

use crate::game::{Ctx, Emit, Game, GameFactory, Gfx, Viewport};
use crate::stats::FrameStats;

/// 单帧 dt 上限，避免卡顿后物理一步跨太远。
const MAX_DT: f32 = 0.05;
/// 拿不到 drawable（被遮挡、最小化）时的退避。
const IDLE_BACKOFF: Duration = Duration::from_millis(16);

/// 主线程发给渲染线程的消息。
pub enum Msg {
    Viewport(Viewport),
    Command(String, JsonValue),
    Prop(String, Option<JsonValue>),
    Quit,
}

/// 渲染线程的句柄；drop 时让线程退出并等它结束（保证 surface 先于 View 释放）。
pub struct Engine {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
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
    pub fn spawn(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        emit: Emit,
        factory: GameFactory,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("nge-render".into())
            .spawn(move || {
                if let Err(e) = run(instance, surface, &emit, factory, rx) {
                    log::error!("渲染线程退出: {e}");
                    emit("error", json!({ "message": e }));
                }
            })
            .expect("创建渲染线程失败");
        Self { tx, thread: Some(thread) }
    }

    pub fn send(&self, msg: Msg) {
        // 线程已退出（初始化失败）时丢弃即可
        let _ = self.tx.send(msg);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Surface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

fn run(
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    emit: &Emit,
    factory: GameFactory,
    rx: Receiver<Msg>,
) -> Result<(), String> {
    // 等到第一个有效视口再初始化 GPU；期间收到的消息先攒着
    let mut pending = Vec::new();
    let viewport = loop {
        match rx.recv().map_err(|_| "主线程已断开".to_string())? {
            Msg::Viewport(v) if !v.is_empty() => break v,
            Msg::Quit => return Ok(()),
            Msg::Viewport(_) => {}
            other => pending.push(other),
        }
    };

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: Some(&surface),
        ..Default::default()
    }))
    .map_err(|e| e.to_string())?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&Default::default())).map_err(|e| e.to_string())?;
    let mut config = surface
        .get_default_config(&adapter, viewport.pixels[0], viewport.pixels[1])
        .ok_or("surface 不被该 adapter 支持")?;
    // Fifo：get_current_texture 阻塞到下一个 vsync，渲染线程自己就有了节奏
    config.present_mode = wgpu::PresentMode::Fifo;
    config.desired_maximum_frame_latency = 2;
    surface.configure(&device, &config);

    let mut gfx = Gfx { device, queue, format: config.format, viewport };
    let mut surface = Surface { surface, config };
    let mut game = factory(&gfx);

    for msg in pending {
        apply(&mut *game, &mut gfx, &mut surface, emit, msg);
    }

    let mut last = Instant::now();
    let mut stats = FrameStats::new(last);
    loop {
        for msg in rx.try_iter() {
            if let Msg::Quit = msg {
                return Ok(());
            }
            apply(&mut *game, &mut gfx, &mut surface, emit, msg);
        }

        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(MAX_DT);
        last = now;
        game.update(&mut Ctx::new(gfx.viewport, emit), dt);

        if !draw(&mut *game, &gfx, &mut surface) {
            // 拿不到画面时不空转；同时还能及时响应 Quit
            match rx.recv_timeout(IDLE_BACKOFF) {
                Ok(Msg::Quit) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Ok(msg) => apply(&mut *game, &mut gfx, &mut surface, emit, msg),
                Err(RecvTimeoutError::Timeout) => {}
            }
            continue;
        }

        if let Some(report) = stats.frame(Instant::now()) {
            emit("fps", json!({ "fps": report.fps, "worst_ms": report.worst_ms }));
        }
    }
}

fn apply(game: &mut dyn Game, gfx: &mut Gfx, surface: &mut Surface, emit: &Emit, msg: Msg) {
    match msg {
        Msg::Viewport(v) => {
            if v.is_empty() || v == gfx.viewport {
                return;
            }
            gfx.viewport = v;
            if [surface.config.width, surface.config.height] != v.pixels {
                surface.config.width = v.pixels[0];
                surface.config.height = v.pixels[1];
                surface.surface.configure(&gfx.device, &surface.config);
            }
        }
        Msg::Command(name, args) => game.command(&mut Ctx::new(gfx.viewport, emit), &name, &args),
        Msg::Prop(name, value) => game.prop(&mut Ctx::new(gfx.viewport, emit), &name, value.as_ref()),
        Msg::Quit => {}
    }
}

/// 绘制一帧；返回 false 表示这一帧没拿到画面。
fn draw(game: &mut dyn Game, gfx: &Gfx, surface: &mut Surface) -> bool {
    let frame = match surface.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
        wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
            surface.surface.configure(&gfx.device, &surface.config);
            return false;
        }
        _ => return false,
    };
    let target = frame.texture.create_view(&Default::default());
    let mut encoder = gfx.device.create_command_encoder(&Default::default());
    game.render(gfx, &mut encoder, &target);
    gfx.queue.submit([encoder.finish()]);
    gfx.queue.present(frame);
    true
}
