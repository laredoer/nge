//! 和驱动方式无关的部分：GPU 初始化、消息、一帧的更新和绘制。
//! 原生由渲染线程驱动（`runtime.rs`），wasm 由浏览器的 `requestAnimationFrame` 驱动（`web.rs`）。

use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;

use serde_json::Value as JsonValue;
use web_time::Instant;

use crate::bridge::{ENGINE_ERROR, EngineError, FRAME_STATS};
use crate::game::{Ctx, Emit, FrameTiming, Game, Gfx, Viewport};
use crate::stats::{FrameClock, FrameStats};

/// 单帧 dt 上限，避免卡顿后物理一步跨太远。
const MAX_DT: f32 = 0.05;

/// 平台层发给引擎的消息。
pub enum Msg {
    Viewport(Viewport),
    Command(String, JsonValue),
    Prop(String, Option<JsonValue>),
    /// 停止取画面和提交 GPU 命令；进入暂停后回复确认。
    Pause(SyncSender<()>),
    Resume,
    Quit,
}

/// GPU 就绪、有了有效视口、且没有暂停之前收到的消息：视口和暂停只记最新的，其余的按顺序攒着。
#[derive(Default)]
pub(crate) struct Gate {
    viewport: Option<Viewport>,
    paused: bool,
    pending: Vec<Msg>,
}

impl Gate {
    /// 收一条消息；返回 false 表示收到了 `Quit`。
    pub fn push(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Viewport(v) if !v.is_empty() => self.viewport = Some(v),
            Msg::Viewport(_) => {}
            Msg::Pause(ack) => {
                self.paused = true;
                let _ = ack.send(());
            }
            Msg::Resume => self.paused = false,
            Msg::Quit => return false,
            other => self.pending.push(other),
        }
        true
    }

    /// 能开始了：返回视口。
    pub fn ready(&self) -> Option<Viewport> {
        if self.paused { None } else { self.viewport }
    }

    /// 开始时把攒着的消息交出去。
    pub fn take(self) -> Vec<Msg> {
        self.pending
    }
}

/// WebGPU 默认上限，硬件做不到的按硬件的来（iOS 模拟器的阶段间变量比默认少一个）。
fn device_limits(supported: &wgpu::Limits) -> wgpu::Limits {
    wgpu::Limits::default().or_worse_values_from(supported)
}

/// 贴图压缩格式（ASTC / BC / ETC2）：适配器支持哪个就要哪个，压缩贴图能直接上传。
pub fn compression_features(supported: wgpu::Features) -> wgpu::Features {
    supported & (wgpu::Features::TEXTURE_COMPRESSION_ASTC | wgpu::Features::TEXTURE_COMPRESSION_BC | wgpu::Features::TEXTURE_COMPRESSION_ETC2)
}

/// 拿到了设备、还没配置画面的 GPU。
pub(crate) struct Gpu {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
}

/// 请求适配器和设备（原生上用 pollster 阻塞等，wasm 上在浏览器的事件循环里等）。
pub(crate) async fn request_gpu(instance: &wgpu::Instance, surface: wgpu::Surface<'static>) -> Result<Gpu, String> {
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: Some(&surface), ..Default::default() })
        .await
        .map_err(|e| e.to_string())?;
    let descriptor = wgpu::DeviceDescriptor {
        required_limits: device_limits(&adapter.limits()),
        required_features: compression_features(adapter.features()),
        ..Default::default()
    };
    let (device, queue) = adapter.request_device(&descriptor).await.map_err(|e| e.to_string())?;
    device.on_uncaptured_error(Arc::new(|e| log::error!("wgpu 错误: {e}")));
    Ok(Gpu { adapter, device, queue, surface })
}

/// 画布的格式：sRGB 的照用；WebGPU 的画布只有非 sRGB 的格式，配置时加上 sRGB 的 view format，
/// 引擎画到 sRGB 的 view 上，颜色和原生一致。返回（画布格式、引擎画的格式）。
pub(crate) fn surface_formats(format: wgpu::TextureFormat) -> (wgpu::TextureFormat, wgpu::TextureFormat) {
    (format, format.add_srgb_suffix())
}

pub(crate) struct Surface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// 引擎画的格式（sRGB）。
    view_format: wgpu::TextureFormat,
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

/// 按视口配置画面，得到游戏用的 [`Gfx`]。
pub(crate) fn configure(gpu: Gpu, viewport: Viewport) -> Result<(Gfx, Surface), String> {
    let Gpu { adapter, device, queue, surface } = gpu;
    let max_side = device.limits().max_texture_dimension_2d;
    let [w, h] = viewport.pixels.map(|p| p.clamp(1, max_side));
    let mut config = surface.get_default_config(&adapter, w, h).ok_or("surface 不被该 adapter 支持")?;
    let (format, view_format) = surface_formats(config.format);
    config.format = format;
    if view_format != format {
        config.view_formats.push(view_format);
    }
    // Fifo：原生上 get_current_texture 阻塞到下一个 vsync，渲染线程自己就有了节奏；web 上节奏由 rAF 给
    config.present_mode = wgpu::PresentMode::Fifo;
    config.desired_maximum_frame_latency = 2;
    surface.configure(&device, &config);
    let gfx = Gfx { device, queue, format: view_format, viewport };
    Ok((gfx, Surface { surface, config, view_format, max_side, lost: false }))
}

pub(crate) enum Flow {
    Continue,
    Pause(SyncSender<()>),
    Quit,
}

/// 跑起来的游戏：GPU、画面和帧计时。
pub(crate) struct Core {
    game: Box<dyn Game>,
    gfx: Gfx,
    surface: Surface,
    emit: Rc<Emit>,
    last: Instant,
    stats: FrameStats,
    clock: FrameClock,
}

impl Core {
    pub fn new(game: Box<dyn Game>, gfx: Gfx, surface: Surface, emit: Rc<Emit>) -> Self {
        let now = Instant::now();
        Self { game, gfx, surface, emit, last: now, stats: FrameStats::new(now), clock: FrameClock::default() }
    }

    pub fn apply(&mut self, msg: Msg) -> Flow {
        let ctx = &mut Ctx::new(self.gfx.viewport, &self.emit);
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

    /// 暂停回来：暂停期间的时间不计入 dt，也不计入帧统计。
    pub fn resumed(&mut self) {
        self.last = Instant::now();
        self.stats = FrameStats::new(self.last);
        self.clock.reset();
    }

    /// 更新并画一帧；`start` 是这一帧开始处理消息的时刻（消息也算在更新里）。返回 false 表示没拿到画面。
    pub fn step(&mut self, start: Instant) -> bool {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(MAX_DT);
        self.last = now;
        self.game.update(&mut Ctx::new(self.gfx.viewport, &self.emit), dt);
        let update = start.elapsed().as_secs_f32();

        let Some((render, wait)) = self.draw() else {
            // 开头、暂停后、拿不到画面后的第一帧没有上一帧可比，不报
            self.clock.reset();
            return false;
        };
        let done = Instant::now();
        if let Some(frame) = self.clock.drawn(done) {
            self.game.timing(FrameTiming { frame: frame.as_secs_f32(), update, render, wait });
        }
        if let Some(report) = self.stats.frame(done) {
            (self.emit)(FRAME_STATS.name(), FRAME_STATS.encode(&report));
        }
        true
    }

    /// 绘制一帧，返回（录制和提交，拿画面和呈现）各花了多少秒；`None` 表示这一帧没拿到画面。
    fn draw(&mut self) -> Option<(f32, f32)> {
        if self.surface.lost {
            return None;
        }
        let start = Instant::now();
        let frame = match self.surface.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.surface.configure(&self.gfx.device, &self.surface.config);
                return None;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                // surface 需要重建，这一版先停止渲染并上报
                self.surface.lost = true;
                log::error!("surface 丢失，停止渲染");
                (self.emit)(ENGINE_ERROR.name(), ENGINE_ERROR.encode(&EngineError { message: "surface lost".into() }));
                return None;
            }
            _ => return None,
        };
        let acquired = Instant::now();
        let target = frame.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(self.surface.view_format), ..Default::default() });
        let mut encoder = self.gfx.device.create_command_encoder(&Default::default());
        self.game.render(&self.gfx, &mut encoder, &target);
        self.gfx.queue.submit([encoder.finish()]);
        let submitted = Instant::now();
        self.gfx.queue.present(frame);
        let render = (submitted - acquired).as_secs_f32();
        Some((render, (acquired - start).as_secs_f32() + submitted.elapsed().as_secs_f32()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    const VIEW: Viewport = Viewport { size: [100.0, 50.0], pixels: [200, 100] };

    #[test]
    fn device_limits_never_ask_more_than_the_adapter_has() {
        // iOS 模拟器：阶段间变量只有 15 个，比 WebGPU 默认的 16 少
        let simulator = wgpu::Limits { max_inter_stage_shader_variables: 15, ..wgpu::Limits::default() };
        assert_eq!(device_limits(&simulator).max_inter_stage_shader_variables, 15);
        // 硬件更好时不多要，按默认值
        let big = wgpu::Limits { max_texture_dimension_2d: 16384, ..wgpu::Limits::default() };
        assert_eq!(device_limits(&big), wgpu::Limits::default());
    }

    #[test]
    fn asks_only_for_the_compression_the_adapter_has() {
        use wgpu::Features as F;
        let apple = F::TEXTURE_COMPRESSION_ASTC | F::TEXTURE_COMPRESSION_ETC2 | F::DEPTH_CLIP_CONTROL;
        assert_eq!(compression_features(apple), F::TEXTURE_COMPRESSION_ASTC | F::TEXTURE_COMPRESSION_ETC2, "别的特性不要");
        assert_eq!(compression_features(F::TEXTURE_COMPRESSION_BC), F::TEXTURE_COMPRESSION_BC);
        assert_eq!(compression_features(F::empty()), F::empty());
    }

    #[test]
    fn the_engine_draws_in_srgb_even_when_the_canvas_is_not() {
        use wgpu::TextureFormat as T;
        // WebGPU 的画布
        assert_eq!(surface_formats(T::Bgra8Unorm), (T::Bgra8Unorm, T::Bgra8UnormSrgb));
        assert_eq!(surface_formats(T::Rgba8Unorm), (T::Rgba8Unorm, T::Rgba8UnormSrgb));
        // Mac、iPhone 默认就是 sRGB
        assert_eq!(surface_formats(T::Bgra8UnormSrgb), (T::Bgra8UnormSrgb, T::Bgra8UnormSrgb));
    }

    #[test]
    fn gate_waits_for_a_viewport_and_keeps_the_rest_in_order() {
        let mut gate = Gate::default();
        assert!(gate.push(Msg::Prop("scene".into(), None)));
        assert!(gate.push(Msg::Viewport(Viewport::default())));
        assert_eq!(gate.ready(), None, "空视口不算");
        assert!(gate.push(Msg::Command("shake".into(), JsonValue::Null)));
        assert!(gate.push(Msg::Viewport(VIEW)));
        assert_eq!(gate.ready(), Some(VIEW));
        let names: Vec<_> = gate
            .take()
            .into_iter()
            .map(|m| match m {
                Msg::Prop(n, _) | Msg::Command(n, _) => n,
                _ => panic!("视口、暂停不该攒着"),
            })
            .collect();
        assert_eq!(names, ["scene", "shake"]);
    }

    #[test]
    fn gate_stays_closed_while_paused_and_answers_the_pause() {
        let mut gate = Gate::default();
        gate.push(Msg::Viewport(VIEW));
        let (tx, rx) = mpsc::sync_channel(1);
        gate.push(Msg::Pause(tx));
        assert_eq!(rx.try_recv(), Ok(()), "暂停当场确认");
        assert_eq!(gate.ready(), None);
        gate.push(Msg::Resume);
        assert_eq!(gate.ready(), Some(VIEW));
        assert!(!gate.push(Msg::Quit));
    }
}
