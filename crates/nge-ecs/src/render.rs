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

/// 3D 模型和 2D 精灵按深度混排（见 docs/3d-more.md）：开着时 3D 渲染器把深度留给精灵，精灵按自己的 z 在 3D 空间里的深度和模型比，
/// z < 0 的精灵也不再固定画在模型后面。要配合跟 2D 对齐的 3D 相机（`camera3d.match2d`）用。
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Interleave(pub bool);

/// 3D 渲染器这一帧留下的深度（深度 + 模板格式）和它用的 3D 相机矩阵。
#[derive(Debug, Clone)]
pub struct SceneDepth {
    pub view: wgpu::TextureView,
    /// 世界坐标 → 裁剪空间（列主序）。
    pub view_proj: [f32; 16],
}

/// 和 3D 渲染器共用的深度缓冲格式（深度 + 模板：精灵的遮罩也写在里面）。
pub const SCENE_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// 这一帧裁剪的统计（见 docs/big-scene-perf.md）：`[画了, 一共]`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CullStats {
    /// 瓦片地图的块。
    pub chunks: [u32; 2],
    /// 画了几格瓦片。
    pub tiles: u32,
    /// 2D 精灵（实体）。
    pub sprites: [u32; 2],
    /// 3D 网格的部件。
    pub meshes: [u32; 2],
}

/// 本帧的渲染状态，每帧渲染前重置。渲染器按插件添加的顺序执行。
#[derive(Resource, Debug, Default)]
pub struct RenderFrame {
    cleared: bool,
    depth: Option<SceneDepth>,
    draws: u32,
    instances: u32,
    cull: CullStats,
}

impl RenderFrame {
    /// 颜色附件的 load 操作：本帧第一次调用时清屏，之后保留前面渲染器画的内容。
    pub fn color_load(&mut self, clear: ClearColor) -> wgpu::LoadOp<wgpu::Color> {
        if std::mem::replace(&mut self.cleared, true) { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(clear.0) }
    }

    /// 3D 渲染器留下深度（[`Interleave`] 开着时）。
    pub fn set_depth(&mut self, depth: SceneDepth) {
        self.depth = Some(depth);
    }

    /// 这一帧前面的渲染器留下的深度。
    pub fn depth(&self) -> Option<&SceneDepth> {
        self.depth.as_ref()
    }

    /// 渲染器记下这一帧发了几次 draw、一共画了多少个实例（性能面板用）。
    pub fn count(&mut self, draws: u32, instances: u32) {
        self.draws += draws;
        self.instances += instances;
    }

    /// 这一帧到目前为止的（draw 次数，实例数）。
    pub fn counts(&self) -> (u32, u32) {
        (self.draws, self.instances)
    }

    /// 渲染器往里加这一帧画了几个、一共几个。
    pub fn cull_mut(&mut self) -> &mut CullStats {
        &mut self.cull
    }

    /// 这一帧到目前为止的裁剪统计。
    pub fn culled(&self) -> CullStats {
        self.cull
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
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

    #[test]
    fn renderers_add_up_their_draws_until_the_next_frame() {
        let mut frame = RenderFrame::default();
        frame.count(3, 100);
        frame.count(2, 5);
        assert_eq!(frame.counts(), (5, 105));
        frame.reset();
        assert_eq!(frame.counts(), (0, 0));
    }

    #[test]
    fn cull_stats_add_up_until_the_next_frame() {
        let mut frame = RenderFrame::default();
        frame.cull_mut().sprites[0] += 3;
        frame.cull_mut().sprites[1] += 10;
        frame.cull_mut().chunks = [2, 40];
        frame.cull_mut().sprites[1] += 5;
        assert_eq!(frame.culled(), CullStats { chunks: [2, 40], tiles: 0, sprites: [3, 15], meshes: [0, 0] });
        frame.reset();
        assert_eq!(frame.culled(), CullStats::default());
    }
}
