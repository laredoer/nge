//! 多个渲染器共用一帧：第一个清屏，后面的叠加。

use bevy_ecs::prelude::Resource;
use nge_core::wgpu;

/// 每帧的清屏颜色（线性）。
#[derive(Resource, Debug, Clone, Copy)]
pub struct ClearColor(pub wgpu::Color);

impl Default for ClearColor {
    fn default() -> Self {
        Self(wgpu::Color { r: 0.02, g: 0.02, b: 0.04, a: 1.0 })
    }
}

/// 本帧的渲染状态，每帧渲染前重置。渲染器按插件添加的顺序执行。
#[derive(Resource, Debug, Default)]
pub struct RenderFrame {
    cleared: bool,
}

impl RenderFrame {
    /// 颜色附件的 load 操作：本帧第一次调用时清屏，之后保留前面渲染器画的内容。
    pub fn color_load(&mut self, clear: ClearColor) -> wgpu::LoadOp<wgpu::Color> {
        if std::mem::replace(&mut self.cleared, true) { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(clear.0) }
    }

    pub(crate) fn reset(&mut self) {
        self.cleared = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_renderer_clears_the_rest_load() {
        let mut frame = RenderFrame::default();
        let clear = ClearColor(wgpu::Color::RED);
        assert_eq!(frame.color_load(clear), wgpu::LoadOp::Clear(wgpu::Color::RED));
        assert_eq!(frame.color_load(clear), wgpu::LoadOp::Load);
        frame.reset();
        assert_eq!(frame.color_load(clear), wgpu::LoadOp::Clear(wgpu::Color::RED));
    }
}
