//! 和驱动方式无关的部分：GPU 初始化、消息、一帧的更新和绘制。
//! 原生由渲染线程驱动（`runtime.rs`），wasm 由浏览器的 `requestAnimationFrame` 驱动（`web.rs`）。

use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::SyncSender;

use serde_json::Value as JsonValue;
use web_time::Instant;

use crate::bridge::{ENGINE_ERROR, EngineError, FRAME_STATS, READY, Ready};
use crate::game::{Ctx, Emit, FrameTiming, Game, Gfx, Viewport};
use crate::pipeline_cache;
use crate::stats::{FrameClock, FrameStats};

/// 单帧 dt 上限，避免卡顿后物理一步跨太远。
const MAX_DT: f32 = 0.05;
/// 开始跑以后过这么久存一次管线缓存（预热编完的管线不用等到切后台），之后每隔 [`CACHE_SAVE_EVERY`] 一次（没变不写）。
const CACHE_SAVE_AFTER: std::time::Duration = std::time::Duration::from_secs(10);
const CACHE_SAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// 平台层发给引擎的消息。`S` 是画面（测试里用别的类型代替）。
pub enum Msg<S = wgpu::Surface<'static>> {
    Viewport(Viewport),
    Command(String, JsonValue),
    Prop(String, Option<JsonValue>),
    /// 换一个画面（Android 切后台再回来时系统给的是新的 surface）。
    Surface(S),
    /// 停止取画面和提交 GPU 命令；进入暂停后回复确认。
    Pause(SyncSender<()>),
    Resume,
    Quit,
}

/// GPU 就绪、有了有效视口、且没有暂停之前收到的消息：视口和暂停只记最新的；
/// 同名的 prop 只留最新一份（放在最后一次出现的位置，web 上加载期间每帧都会发 scene），command 按顺序全留着。
pub(crate) struct Gate<S = wgpu::Surface<'static>> {
    viewport: Option<Viewport>,
    paused: bool,
    pending: Vec<Msg<S>>,
    /// 等的时候换过的画面：只留最新的。
    surface: Option<S>,
    /// 手上的画面不能用（配置失败）：等到新的画面才能开始。
    stale: bool,
}

impl<S> Default for Gate<S> {
    fn default() -> Self {
        Self { viewport: None, paused: false, pending: Vec::new(), surface: None, stale: false }
    }
}

impl<S> Gate<S> {
    /// 收一条消息；返回 false 表示收到了 `Quit`。
    pub fn push(&mut self, msg: Msg<S>) -> bool {
        match msg {
            Msg::Viewport(v) if !v.is_empty() => self.viewport = Some(v),
            Msg::Viewport(_) => {}
            Msg::Pause(ack) => {
                self.paused = true;
                let _ = ack.send(());
            }
            Msg::Resume => self.paused = false,
            Msg::Quit => return false,
            Msg::Surface(surface) => self.surface = Some(surface),
            Msg::Prop(name, value) => {
                self.pending.retain(|m| !matches!(m, Msg::Prop(n, _) if *n == name));
                self.pending.push(Msg::Prop(name, value));
            }
            other => self.pending.push(other),
        }
        true
    }

    /// 能开始了：返回视口。
    pub fn ready(&self) -> Option<Viewport> {
        if self.paused || (self.stale && self.surface.is_none()) { None } else { self.viewport }
    }

    /// 等的时候换来的画面（只给一次）。
    pub fn take_surface(&mut self) -> Option<S> {
        let surface = self.surface.take();
        if surface.is_some() {
            self.stale = false;
        }
        surface
    }

    /// 手上的画面配置不了：等下一个画面。
    pub fn surface_unusable(&mut self) {
        self.stale = true;
    }

    /// 开始时把攒着的消息交出去。
    pub fn take(self) -> Vec<Msg<S>> {
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

/// 支持就要的特性：贴图压缩格式和时间戳（GPU 时间，见 docs/big-scene-perf.md）。
pub fn optional_features(supported: wgpu::Features) -> wgpu::Features {
    compression_features(supported) | (supported & wgpu::Features::TIMESTAMP_QUERY)
}

/// 拿到了设备、还没配置画面的 GPU。
pub(crate) struct Gpu {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    cache: Option<DiskCache>,
}

/// 开着的管线缓存和它的文件。
pub(crate) struct DiskCache {
    cache: wgpu::PipelineCache,
    /// 写文件在后台线程上，不挡着渲染线程。
    store: Arc<Mutex<pipeline_cache::Store>>,
    /// 上次读到、存下的数据有多大。
    len: usize,
}

/// 什么时候存管线缓存。
pub(crate) struct SaveClock {
    at: Instant,
}

impl SaveClock {
    pub fn new(now: Instant) -> Self {
        Self { at: now + CACHE_SAVE_AFTER }
    }

    /// 到点了（到点以后排下一次）。
    pub fn due(&mut self, now: Instant) -> bool {
        if now < self.at {
            return false;
        }
        self.at = now + CACHE_SAVE_EVERY;
        true
    }
}

impl DiskCache {
    /// 平台层给了目录、适配器支持（现在只有 Vulkan）时，要在建设备时请求的特性和文件名。
    fn wanted(adapter: &wgpu::Adapter) -> Option<(std::path::PathBuf, String)> {
        let dir = pipeline_cache::dir()?;
        if !adapter.features().contains(wgpu::Features::PIPELINE_CACHE) {
            return None;
        }
        Some((dir, pipeline_cache::file_name(&adapter.get_info())?))
    }

    fn open(device: &wgpu::Device, dir: &std::path::Path, name: &str) -> Self {
        let mut store = pipeline_cache::Store::new(dir, name, &pipeline_cache::version());
        let data = store.load();
        log::info!("管线缓存 {}：{}", dir.join(name).display(), data.as_ref().map_or("没有存过".to_owned(), |d| format!("读到 {} 字节", d.len())));
        // SAFETY: 数据是 wgpu 自己 get_data 出来、经过我们的校验和的；fallback 让 wgpu 在数据和驱动对不上时当空的
        let cache = unsafe {
            device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor { label: Some("nge"), data: data.as_deref(), fallback: true })
        };
        Self { cache, store: Arc::new(Mutex::new(store)), len: data.map_or(0, |d| d.len()) }
    }

    /// 渲染线程上只取数据，写文件交给后台线程（没变的 `Store` 不写）。
    fn save(&mut self) {
        let Some(data) = self.cache.get_data() else { return };
        if pipeline_cache::shrank(self.len, data.len()) {
            log::info!("管线缓存从 {} 字节变成 {} 字节：wgpu 多半判定旧数据过期，当空的重来了", self.len, data.len());
        }
        self.len = data.len();
        let store = self.store.clone();
        let spawned = std::thread::Builder::new().name("nge-pipeline-cache".into()).spawn(move || {
            match store.lock().unwrap_or_else(|e| e.into_inner()).save(&data) {
                Ok(true) => log::info!("管线缓存存了 {} 字节", data.len()),
                Ok(false) => {}
                Err(e) => log::warn!("管线缓存存不下: {e}"),
            }
        });
        if let Err(e) = spawned {
            log::warn!("管线缓存存不下（起不了线程）: {e}");
        }
    }
}

/// 请求适配器和设备（原生上用 pollster 阻塞等，wasm 上在浏览器的事件循环里等）。
pub(crate) async fn request_gpu(instance: &wgpu::Instance, surface: wgpu::Surface<'static>) -> Result<Gpu, String> {
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: Some(&surface), ..Default::default() })
        .await
        .map_err(|e| e.to_string())?;
    let info = adapter.get_info();
    log::info!("GPU：{}（{:?}，驱动 {} {}）", info.name, info.backend, info.driver, info.driver_info);
    let disk = DiskCache::wanted(&adapter);
    let mut required_features = optional_features(adapter.features());
    if disk.is_some() {
        required_features |= wgpu::Features::PIPELINE_CACHE;
    }
    let descriptor = wgpu::DeviceDescriptor { required_limits: device_limits(&adapter.limits()), required_features, ..Default::default() };
    let (device, queue) = adapter.request_device(&descriptor).await.map_err(|e| e.to_string())?;
    device.on_uncaptured_error(Arc::new(|e| log::error!("wgpu 错误: {e}")));
    let cache = disk.map(|(dir, name)| DiskCache::open(&device, &dir, &name));
    Ok(Gpu { adapter, device, queue, surface, cache })
}

/// 画布的格式：有 sRGB 的就直接用（Android 的老设备没有 `VK_KHR_swapchain_mutable_format`，配不了 view format）；
/// 没有时（WebGPU 的画布）用第一个，配置时加上 sRGB 的 view format，引擎画到 sRGB 的 view 上，颜色和原生一致。
/// 返回（画布格式、引擎画的格式）；一个格式都没有时是 `None`。
pub(crate) fn surface_formats(formats: &[wgpu::TextureFormat]) -> Option<(wgpu::TextureFormat, wgpu::TextureFormat)> {
    if let Some(srgb) = formats.iter().find(|f| f.is_srgb()) {
        return Some((*srgb, *srgb));
    }
    formats.first().map(|f| (*f, f.add_srgb_suffix()))
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
    /// 换上新的画面，按原来的配置配好。
    fn replace(&mut self, device: &wgpu::Device, surface: wgpu::Surface<'static>) {
        surface.configure(device, &self.config);
        self.surface = surface;
        self.lost = false;
    }

    fn resize(&mut self, device: &wgpu::Device, pixels: [u32; 2]) {
        let [w, h] = pixels.map(|p| p.clamp(1, self.max_side));
        if [self.config.width, self.config.height] != [w, h] {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(device, &self.config);
        }
    }
}

impl Gpu {
    /// 换一个画面（配置之前收到的新画面）。
    pub fn with_surface(self, surface: Option<wgpu::Surface<'static>>) -> Self {
        match surface {
            Some(surface) => Self { surface, ..self },
            None => self,
        }
    }
}

/// 按视口配置画面，得到游戏用的 [`Gfx`]。画面配不了（没有能力、格式）时把 GPU 还回去，等下一个画面。
pub(crate) fn configure(gpu: Gpu, viewport: Viewport) -> Result<(Gfx, Surface, Option<DiskCache>), (Gpu, String)> {
    let max_side = gpu.device.limits().max_texture_dimension_2d;
    let [w, h] = viewport.pixels.map(|p| p.clamp(1, max_side));
    let formats = gpu.surface.get_capabilities(&gpu.adapter).formats;
    let (Some((format, view_format)), Some(mut config)) = (surface_formats(&formats), gpu.surface.get_default_config(&gpu.adapter, w, h)) else {
        return Err((gpu, "surface 不被该 adapter 支持".into()));
    };
    let Gpu { device, queue, surface, cache, .. } = gpu;
    config.format = format;
    if view_format != format {
        config.view_formats.push(view_format);
    }
    // Fifo：原生上 get_current_texture 阻塞到下一个 vsync，渲染线程自己就有了节奏；web 上节奏由 rAF 给
    config.present_mode = wgpu::PresentMode::Fifo;
    config.desired_maximum_frame_latency = 2;
    surface.configure(&device, &config);
    let gfx = Gfx { screen: None, device, queue, format: view_format, viewport, pipeline_cache: cache.as_ref().map(|c| c.cache.clone()) };
    Ok((gfx, Surface { surface, config, view_format, max_side, lost: false }, cache))
}

/// 设备丢了（显卡驱动重置、浏览器回收 GPU）：回调在哪个线程都可能来，先记下，下一帧报 `error`。
#[derive(Default, Clone)]
pub(crate) struct DeviceLost(Arc<Mutex<Option<String>>>);

impl DeviceLost {
    /// 给 `set_device_lost_callback` 的回调；引擎自己销毁设备（`Destroyed`）不算。
    pub fn callback(&self) -> impl Fn(wgpu::DeviceLostReason, String) + Send + 'static {
        let slot = self.0.clone();
        move |reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(message);
            }
        }
    }

    /// 丢了的话取出原因（只取一次）。
    pub fn take(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// 帧间隔：暂停、藏起来回来以后从回来的那一刻算，不把中间的时间算进 dt。
pub(crate) struct Tick {
    last: Instant,
}

impl Tick {
    pub fn new(now: Instant) -> Self {
        Self { last: now }
    }

    pub fn resume(&mut self, now: Instant) {
        self.last = now;
    }

    /// 离上一帧多久（秒，最多 [`MAX_DT`]）。
    pub fn dt(&mut self, now: Instant) -> f32 {
        let dt = now.saturating_duration_since(self.last).as_secs_f32().min(MAX_DT);
        self.last = now;
        dt
    }
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
    tick: Tick,
    stats: FrameStats,
    clock: FrameClock,
    lost: DeviceLost,
    /// 已经发过 `ready`。
    shown: bool,
    cache: Option<DiskCache>,
    save_cache: SaveClock,
}

impl Core {
    pub fn new(game: Box<dyn Game>, gfx: Gfx, surface: Surface, emit: Rc<Emit>, cache: Option<DiskCache>) -> Self {
        let now = Instant::now();
        let lost = DeviceLost::default();
        gfx.device.set_device_lost_callback(lost.callback());
        Self { game, gfx, surface, emit, tick: Tick::new(now), stats: FrameStats::new(now), clock: FrameClock::default(), lost, shown: false, cache, save_cache: SaveClock::new(now) }
    }

    /// 把管线缓存写到磁盘（暂停确认以后、退出、开始跑 10 秒以后每分钟）。
    pub fn save_pipeline_cache(&mut self) {
        if let Some(cache) = &mut self.cache {
            cache.save();
        }
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
            Msg::Surface(surface) => self.surface.replace(&self.gfx.device, surface),
            // 暂停确认以后再存（`runtime.rs`），写盘不挡着平台层
            Msg::Pause(ack) => return Flow::Pause(ack),
            Msg::Resume => {}
            Msg::Quit => {
                self.save_pipeline_cache();
                return Flow::Quit;
            }
        }
        Flow::Continue
    }

    /// 暂停回来：暂停期间的时间不计入 dt，也不计入帧统计。
    pub fn resumed(&mut self) {
        let now = Instant::now();
        self.tick.resume(now);
        self.stats = FrameStats::new(now);
        self.clock.reset();
    }

    /// 更新并画一帧；`start` 是这一帧开始处理消息的时刻（消息也算在更新里）。返回 false 表示没拿到画面。
    pub fn step(&mut self, start: Instant) -> bool {
        if let Some(message) = self.lost.take() {
            self.surface.lost = true;
            log::error!("GPU 设备丢了，停止渲染: {message}");
            (self.emit)(ENGINE_ERROR.name(), ENGINE_ERROR.encode(&EngineError { message: format!("GPU 设备丢了：{message}") }));
        }
        let now = Instant::now();
        if self.cache.is_some() && self.save_cache.due(now) {
            self.save_pipeline_cache();
        }
        let dt = self.tick.dt(now);
        self.game.update(&mut Ctx::new(self.gfx.viewport, &self.emit), dt);
        let update = start.elapsed().as_secs_f32();

        let Some((render, wait)) = self.draw() else {
            // 开头、暂停后、拿不到画面后的第一帧没有上一帧可比，不报
            self.clock.reset();
            return false;
        };
        if !self.shown {
            self.shown = true;
            (self.emit)(READY.name(), READY.encode(&Ready {}));
        }
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
                self.surface.lost = true;
                if cfg!(target_os = "android") {
                    // 画面没了、暂停又超时了（慢的设备上）：等平台层换上新的画面（Msg::Surface）
                    log::warn!("surface 丢失，等新的画面");
                } else {
                    // surface 需要重建，这一版先停止渲染并上报
                    log::error!("surface 丢失，停止渲染");
                    (self.emit)(ENGINE_ERROR.name(), ENGINE_ERROR.encode(&EngineError { message: "surface lost".into() }));
                }
                return None;
            }
            _ => return None,
        };
        let acquired = Instant::now();
        let target = frame.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(self.surface.view_format), ..Default::default() });
        let mut encoder = self.gfx.device.create_command_encoder(&Default::default());
        self.gfx.screen = Some(frame.texture.clone());
        self.game.render(&self.gfx, &mut encoder, &target);
        // 交出去以前放掉，不拿着已经显示了的纹理
        self.gfx.screen = None;
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
    fn timestamps_are_asked_for_only_when_supported() {
        use wgpu::Features as F;
        assert_eq!(optional_features(F::TIMESTAMP_QUERY | F::TEXTURE_COMPRESSION_BC | F::DEPTH_CLIP_CONTROL), F::TIMESTAMP_QUERY | F::TEXTURE_COMPRESSION_BC);
        assert_eq!(optional_features(F::TEXTURE_COMPRESSION_ASTC), F::TEXTURE_COMPRESSION_ASTC, "不支持时不要");
    }

    #[test]
    fn the_engine_draws_in_srgb_even_when_the_canvas_is_not() {
        use wgpu::TextureFormat as T;
        // WebGPU 的画布：只有非 sRGB 的，画到 sRGB 的 view 上
        assert_eq!(surface_formats(&[T::Bgra8Unorm]), Some((T::Bgra8Unorm, T::Bgra8UnormSrgb)));
        assert_eq!(surface_formats(&[T::Rgba8Unorm, T::Rgba16Float]), Some((T::Rgba8Unorm, T::Rgba8UnormSrgb)));
        // Mac、iPhone 默认就是 sRGB
        assert_eq!(surface_formats(&[T::Bgra8UnormSrgb]), Some((T::Bgra8UnormSrgb, T::Bgra8UnormSrgb)));
        // Android 的 Vulkan：非 sRGB 排在前面也直接用 sRGB 的，不靠 view_formats（老设备没有 VK_KHR_swapchain_mutable_format）
        assert_eq!(surface_formats(&[T::Rgba8Unorm, T::Rgba8UnormSrgb, T::Rgb10a2Unorm]), Some((T::Rgba8UnormSrgb, T::Rgba8UnormSrgb)));
        assert_eq!(surface_formats(&[]), None, "没有格式");
    }

    #[test]
    fn gate_waits_for_a_viewport_and_keeps_the_rest_in_order() {
        let mut gate = Gate::<u32>::default();
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
    fn dt_after_resuming_does_not_count_the_gap() {
        let t0 = Instant::now();
        let mut tick = Tick::new(t0);
        let ms = |n| std::time::Duration::from_millis(n);
        assert!((tick.dt(t0 + ms(16)) - 0.016).abs() < 1e-4);
        tick.resume(t0 + ms(10_000));
        assert!((tick.dt(t0 + ms(10_016)) - 0.016).abs() < 1e-4, "暂停的 10 秒不算");
        assert_eq!(tick.dt(t0 + ms(20_000)), MAX_DT, "没说暂停过的大间隔截到上限");
    }

    #[test]
    fn the_pipeline_cache_is_saved_ten_seconds_in_and_then_every_minute() {
        let t0 = Instant::now();
        let s = |n| t0 + std::time::Duration::from_secs(n);
        let mut clock = SaveClock::new(t0);
        assert!(!clock.due(s(9)));
        assert!(clock.due(s(10)), "预热编完的管线不等切后台");
        assert!(!clock.due(s(11)) && !clock.due(s(69)), "存过以后等一分钟");
        assert!(clock.due(s(70)));
        assert!(!clock.due(s(71)));
    }

    #[test]
    fn device_loss_is_reported_once_and_not_when_we_destroy_it() {
        let lost = DeviceLost::default();
        lost.callback()(wgpu::DeviceLostReason::Destroyed, "自己关的".into());
        assert_eq!(lost.take(), None, "引擎自己销毁设备不算出错");
        lost.callback()(wgpu::DeviceLostReason::Unknown, "显卡驱动重置".into());
        assert_eq!(lost.take().as_deref(), Some("显卡驱动重置"));
        assert_eq!(lost.take(), None, "只报一次");
    }

    #[test]
    fn gate_keeps_only_the_latest_value_of_each_prop() {
        let mut gate = Gate::<u32>::default();
        let prop = |n: &str, v: i32| Msg::Prop(n.into(), Some(JsonValue::from(v)));
        for i in 0..50 {
            gate.push(prop("scene", i));
        }
        gate.push(Msg::Command("burst".into(), JsonValue::from(1)));
        gate.push(prop("captureKeys", 0));
        gate.push(prop("scene", 1000));
        gate.push(Msg::Command("burst".into(), JsonValue::from(2)));
        let got: Vec<_> = gate
            .take()
            .into_iter()
            .map(|m| match m {
                Msg::Prop(n, v) => (n, v.unwrap_or_default()),
                Msg::Command(n, v) => (n, v),
                _ => unreachable!(),
            })
            .collect();
        let expect = [("burst", 1), ("captureKeys", 0), ("scene", 1000), ("burst", 2)].map(|(n, v)| (n.to_owned(), JsonValue::from(v)));
        assert_eq!(got, expect, "同名 prop 只留最新的、放在最后一次出现的位置，command 照原顺序都留着");
    }

    #[test]
    fn gate_keeps_the_latest_surface_and_waits_for_one_after_a_failed_configure() {
        let mut gate = Gate::<u32>::default();
        gate.push(Msg::Viewport(VIEW));
        gate.push(Msg::Surface(1));
        gate.push(Msg::Surface(2));
        assert_eq!(gate.take_surface(), Some(2), "只留最新的画面");
        assert_eq!(gate.take_surface(), None, "只给一次");
        gate.surface_unusable();
        assert_eq!(gate.ready(), None, "画面配置不了：等新的");
        gate.push(Msg::Surface(3));
        assert_eq!(gate.ready(), Some(VIEW));
        assert_eq!(gate.take_surface(), Some(3));
        assert_eq!(gate.ready(), Some(VIEW), "换上新的画面以后照常开始");
    }

    #[test]
    fn gate_stays_closed_while_paused_and_answers_the_pause() {
        let mut gate = Gate::<u32>::default();
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
