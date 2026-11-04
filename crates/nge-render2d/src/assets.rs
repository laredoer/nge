//! 图集。贴图本身在 `nge_assets` 的 `Assets<Image>` 里（和 3D 共用），这里只记怎么切。

use bevy_ecs::prelude::Resource;
use glam::{Vec2, Vec4};
use nge_assets::{Assets, Image, ImageId};

use crate::sprite::{Sprite, SpriteImage};

/// 2D 里对贴图的叫法，和 [`ImageId`] 是同一个类型。
pub type TextureId = ImageId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtlasId(u32);

/// 等分网格图集：从左上角开始按行编号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub cols: u32,
    pub rows: u32,
}

struct Atlas {
    texture: TextureId,
    grid: Grid,
}

/// 精灵最终画哪张纹理的哪一块。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Region {
    pub texture: TextureId,
    /// (u, v, 宽, 高)，v 朝下。
    pub uv: Vec4,
    /// 这一块的像素大小。
    pub size: Vec2,
}

#[derive(Resource, Default)]
pub struct Atlases {
    atlases: Vec<Atlas>,
}

impl Atlases {
    /// 贴图还在加载时也可以建图集，加载完后按实际尺寸切。
    pub fn add(&mut self, texture: TextureId, grid: Grid) -> AtlasId {
        assert!(grid.cols > 0 && grid.rows > 0, "图集网格不能为空");
        self.atlases.push(Atlas { texture, grid });
        AtlasId(self.atlases.len() as u32 - 1)
    }

    /// 精灵不缩放时的大小（point）：`size` 优先，否则是纹理区域的像素大小（贴图没加载完时是 1×1）。
    pub fn sprite_size(&self, images: &Assets<Image>, sprite: &Sprite) -> Vec2 {
        sprite.size.unwrap_or_else(|| self.region(images, sprite.image).size)
    }

    /// 精灵对应的纹理区域。引用了不存在的贴图 / 图集时退回白色贴图；贴图还在加载时保留它的编号（渲染器先画白色）。
    pub(crate) fn region(&self, images: &Assets<Image>, image: SpriteImage) -> Region {
        const FALLBACK: Region = Region { texture: TextureId::WHITE, uv: Vec4::new(0.0, 0.0, 1.0, 1.0), size: Vec2::ONE };
        let (texture, uv) = match image {
            SpriteImage::Texture(texture) => (texture, Vec4::new(0.0, 0.0, 1.0, 1.0)),
            SpriteImage::Atlas(atlas, index) => {
                let Some(&Atlas { texture, grid }) = self.atlases.get(atlas.0 as usize) else { return FALLBACK };
                let index = index % (grid.cols * grid.rows);
                let cell = Vec2::new(1.0 / grid.cols as f32, 1.0 / grid.rows as f32);
                let at = Vec2::new((index % grid.cols) as f32, (index / grid.cols) as f32) * cell;
                (texture, Vec4::new(at.x, at.y, cell.x, cell.y))
            }
        };
        if texture.index() as usize >= images.len() {
            return FALLBACK;
        }
        let size = images.size(texture).map_or(Vec2::ONE, |s| s.as_vec2() * Vec2::new(uv.z, uv.w));
        Region { texture, uv, size }
    }
}

#[cfg(test)]
mod tests {
    use glam::UVec2;

    use super::*;

    fn images_with(size: UVec2) -> (Assets<Image>, TextureId) {
        let mut images = Assets::<Image>::with_white();
        let texture = images.add_rgba(size, vec![0; (size.x * size.y * 4) as usize]).unwrap();
        (images, texture)
    }

    #[test]
    fn grid_atlas_uv_and_size() {
        let (images, texture) = images_with(UVec2::new(128, 64));
        let mut atlases = Atlases::default();
        let atlas = atlases.add(texture, Grid { cols: 4, rows: 2 });
        let r = atlases.region(&images, SpriteImage::Atlas(atlas, 5));
        assert_eq!(r.texture, texture);
        assert_eq!(r.uv, Vec4::new(0.25, 0.5, 0.25, 0.5));
        assert_eq!(r.size, Vec2::new(32.0, 32.0));
    }

    #[test]
    fn atlas_index_wraps() {
        let (images, texture) = images_with(UVec2::new(4, 4));
        let mut atlases = Atlases::default();
        let atlas = atlases.add(texture, Grid { cols: 2, rows: 2 });
        assert_eq!(atlases.region(&images, SpriteImage::Atlas(atlas, 4)), atlases.region(&images, SpriteImage::Atlas(atlas, 0)));
    }

    #[test]
    fn sprite_size_prefers_explicit_size() {
        let (images, texture) = images_with(UVec2::new(128, 64));
        let mut atlases = Atlases::default();
        let atlas = atlases.add(texture, Grid { cols: 4, rows: 2 });
        assert_eq!(atlases.sprite_size(&images, &Sprite::from_atlas(atlas, 0)), Vec2::new(32.0, 32.0));
        assert_eq!(atlases.sprite_size(&images, &Sprite::from_texture(texture).with_size(Vec2::splat(5.0))), Vec2::splat(5.0));
    }

    #[test]
    fn unknown_ids_fall_back_to_white() {
        let images = Assets::<Image>::with_white();
        let mut other = Assets::<Image>::with_white();
        let foreign = other.add_rgba(UVec2::ONE, vec![0; 4]).unwrap();
        let atlases = Atlases::default();
        assert_eq!(atlases.region(&images, SpriteImage::Texture(foreign)).texture, TextureId::WHITE);
        assert_eq!(atlases.region(&images, SpriteImage::Atlas(AtlasId(3), 0)).texture, TextureId::WHITE);
    }
}
