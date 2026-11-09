//! 游戏接口：引擎在渲染线程上创建并驱动一个 [`Game`]。

use serde_json::Value as JsonValue;

/// 把事件发回 NSH（平台层的 `ComponentContext::emit`），可跨线程调用。
pub type Emit = Box<dyn Fn(&str, JsonValue) + Send>;

/// 视口：point 尺寸 + 像素尺寸。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Viewport {
    pub size: [f32; 2],
    pub pixels: [u32; 2],
}

impl Viewport {
    pub fn is_empty(&self) -> bool {
        self.size[0] < 1.0 || self.size[1] < 1.0 || self.pixels[0] == 0 || self.pixels[1] == 0
    }

    /// 每 point 多少像素。
    pub fn scale(&self) -> f32 {
        if self.size[0] > 0.0 { self.pixels[0] as f32 / self.size[0] } else { 1.0 }
    }
}

/// 一帧各段花的时间（秒），引擎每帧画完后交给游戏（见 docs/perf-overlay.md）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameTiming {
    /// 和上一帧画完时隔了多久。
    pub frame: f32,
    /// [`Game::update`]。
    pub update: f32,
    /// [`Game::render`] 加提交。
    pub render: f32,
    /// 拿画面和交给系统显示（通常是在等垂直同步）。
    pub wait: f32,
}

impl FrameTiming {
    /// 不是有限数、是负数的当 0。
    pub fn sanitized(self) -> Self {
        let ok = |t: f32| if t.is_finite() && t > 0.0 { t } else { 0.0 };
        Self { frame: ok(self.frame), update: ok(self.update), render: ok(self.render), wait: ok(self.wait) }
    }
}

/// 渲染线程上的 GPU 资源，游戏用它建管线、写缓冲。
pub struct Gfx {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
    pub viewport: Viewport,
}

/// 每次回调时游戏能拿到的上下文。
pub struct Ctx<'a> {
    pub viewport: Viewport,
    emit: &'a Emit,
}

impl<'a> Ctx<'a> {
    pub fn new(viewport: Viewport, emit: &'a Emit) -> Self {
        Self { viewport, emit }
    }

    pub fn emit(&self, name: &str, data: JsonValue) {
        (self.emit)(name, data);
    }
}

/// 一个游戏。全部回调都在渲染线程上执行，不需要 `Send`。
pub trait Game {
    /// NSH 调了 `ref.command(name, args)`。
    fn command(&mut self, ctx: &mut Ctx, name: &str, args: &JsonValue);

    /// NSH 改了属性（`None`：移除）。
    fn prop(&mut self, ctx: &mut Ctx, name: &str, value: Option<&JsonValue>) {
        let _ = (ctx, name, value);
    }

    /// 推进逻辑，`dt` 单位秒（已限幅）。
    fn update(&mut self, ctx: &mut Ctx, dt: f32);

    /// 往 `target` 录制本帧的绘制。
    fn render(&mut self, gfx: &Gfx, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView);

    /// 这一帧画完了，各段花了多久（拿不到画面的帧不报）。
    fn timing(&mut self, timing: FrameTiming) {
        let _ = timing;
    }
}

/// 在渲染线程上、GPU 就绪后创建游戏。
pub type GameFactory = std::sync::Arc<dyn Fn(&Gfx) -> Box<dyn Game> + Send + Sync>;
