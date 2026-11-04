//! 2D 渲染：世界坐标单位是 point，y 轴朝上，默认原点在视口中心。
//!
//! 精灵按 z 从小到大绘制，同一 z 内按纹理分组；相邻同纹理的精灵合成一次实例化 draw。

mod assets;
mod batch;
mod camera;
mod renderer;
mod sprite;

pub use assets::{AtlasId, Atlases, Grid, TextureId};
pub use nge_assets::{AssetPlugin, AssetServer, Assets, Image, ImageId, MAX_TEXTURE_SIDE};
pub use camera::{Camera2d, DrawAt, Transform2d};
pub use nge_ecs::ClearColor;
pub use renderer::{PointerWorld, Render2dPlugin};
pub use sprite::{Sprite, SpriteImage, SpriteInstance, SpriteInstances};

pub mod prelude {
    pub use crate::{AssetServer, Assets, Atlases, Camera2d, ClearColor, DrawAt, Grid, Image, Render2dPlugin, Sprite, Transform2d};
}
