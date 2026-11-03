//! 2D 渲染：世界坐标单位是 point，y 轴朝上，默认原点在视口中心。
//!
//! 精灵按 z 从小到大绘制，同一 z 内按纹理分组；相邻同纹理的精灵合成一次实例化 draw。

mod assets;
mod batch;
mod camera;
mod renderer;
mod sprite;

pub use assets::{AtlasId, Assets, Grid, MAX_TEXTURE_SIDE, TextureId};
pub use camera::{Camera2d, Transform2d};
pub use renderer::{ClearColor, PointerWorld, Render2dPlugin};
pub use sprite::{Sprite, SpriteImage};

pub mod prelude {
    pub use crate::{Assets, Camera2d, ClearColor, Grid, Render2dPlugin, Sprite, Transform2d};
}
