//! 2D 渲染：世界坐标单位是 point，y 轴朝上，默认原点在视口中心。
//!
//! 精灵按 z 从小到大绘制，同一 z 内按纹理分组；相邻同纹理的精灵合成一次实例化 draw。

mod assets;
mod batch;
mod bloom;
mod camera;
mod effects;
mod gizmos;
mod occluders;
mod pipes;
mod post;
mod renderer;
mod repeat;
mod sprite;
mod warm;

/// 单元测试用的 GPU；没有时是 `None`（测试跳过）。
#[cfg(test)]
pub(crate) fn test_gfx(format: nge_core::wgpu::TextureFormat) -> Option<nge_core::Gfx> {
    use nge_core::wgpu;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    Some(nge_core::Gfx { screen: None, device, queue, format, viewport: nge_core::Viewport { size: [64.0; 2], pixels: [64; 2] } })
}

pub use assets::{AtlasId, Atlases, Grid, TextureId};
pub use nge_assets::{AssetPlugin, AssetServer, Assets, Image, ImageId, MAX_TEXTURE_SIDE};
pub use camera::{Camera2d, DrawAt, Parallax, Transform2d, drawn};
pub use nge_ecs::ClearColor;
pub use effects::{Light2d, Lighting2d, MAX_LIGHTS, Masking, PostEffect, PostEffects, PostLayer, Shaders, SpriteShader, Unlit};
pub use occluders::{MAX_OCCLUDER_EDGES, NormalMap, Occluder2d};
pub use gizmos::{GIZMO_Z, Gizmos};
pub use renderer::{PointerWorld, Render2dPlugin};
pub use repeat::{MAX_COPIES, Repeat};
pub use sprite::{Sprite, SpriteImage, SpriteInstance, SpriteInstances};

pub mod prelude {
    pub use crate::{AssetServer, Assets, Atlases, Camera2d, ClearColor, DrawAt, Gizmos, Grid, Image, Render2dPlugin, Sprite, Transform2d};
}
