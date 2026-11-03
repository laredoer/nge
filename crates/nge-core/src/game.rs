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
}

/// 在渲染线程上、GPU 就绪后创建游戏。
pub type GameFactory = std::sync::Arc<dyn Fn(&Gfx) -> Box<dyn Game> + Send + Sync>;
