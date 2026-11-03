//! 纹理和图集。图片在 CPU 侧同步解码，渲染器第一次用到时上传 GPU。

use bevy_ecs::prelude::Resource;
use glam::{UVec2, Vec2, Vec4};

use crate::sprite::SpriteImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextureId(pub(crate) u32);

/// 纹理边长上限：wgpu 默认设备限制（WebGPU 默认值）。
pub const MAX_TEXTURE_SIDE: u32 = 8192;

impl TextureId {
    /// 内置的 1×1 白色纹理，纯色精灵用。
    pub const WHITE: Self = Self(0);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtlasId(u32);

/// 等分网格图集：从左上角开始按行编号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub cols: u32,
    pub rows: u32,
}

pub(crate) struct Image {
    pub size: UVec2,
    pub rgba: Vec<u8>,
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

#[derive(Resource)]
pub struct Assets {
    images: Vec<Image>,
    atlases: Vec<Atlas>,
}

impl Default for Assets {
    fn default() -> Self {
        Self { images: vec![Image { size: UVec2::ONE, rgba: vec![255; 4] }], atlases: Vec::new() }
    }
}

impl Assets {
    /// 解码 PNG（通常来自 `include_bytes!`）。
    pub fn load_png(&mut self, bytes: &[u8]) -> Result<TextureId, String> {
        let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .map_err(|e| format!("PNG 解码失败: {e}"))?
            .into_rgba8();
        let size = UVec2::new(image.width(), image.height());
        self.add_rgba(size, image.into_raw())
    }

    /// 直接添加 RGBA8（sRGB）像素。尺寸为 0、超过 [`MAX_TEXTURE_SIDE`] 或长度不符时返回错误。
    pub fn add_rgba(&mut self, size: UVec2, rgba: Vec<u8>) -> Result<TextureId, String> {
        if size.min_element() == 0 || size.max_element() > MAX_TEXTURE_SIDE {
            return Err(format!("纹理尺寸 {size} 不合法（每边 1..={MAX_TEXTURE_SIDE}）"));
        }
        let expected = u64::from(size.x) * u64::from(size.y) * 4;
        if rgba.len() as u64 != expected {
            return Err(format!("像素数据 {} 字节，尺寸 {size} 需要 {expected} 字节", rgba.len()));
        }
        self.images.push(Image { size, rgba });
        Ok(TextureId(self.images.len() as u32 - 1))
    }

    pub fn atlas(&mut self, texture: TextureId, grid: Grid) -> AtlasId {
        assert!(grid.cols > 0 && grid.rows > 0, "图集网格不能为空");
        self.atlases.push(Atlas { texture, grid });
        AtlasId(self.atlases.len() as u32 - 1)
    }

    /// 不存在的纹理（比如来自另一个 Assets）返回 None。
    pub fn texture_size(&self, texture: TextureId) -> Option<UVec2> {
        self.images.get(texture.0 as usize).map(|i| i.size)
    }

    pub(crate) fn images(&self) -> &[Image] {
        &self.images
    }

    /// 精灵对应的纹理区域；引用了不存在的纹理 / 图集时退回白色纹理。
    pub(crate) fn region(&self, image: SpriteImage) -> Region {
        const FALLBACK: Region = Region { texture: TextureId::WHITE, uv: Vec4::new(0.0, 0.0, 1.0, 1.0), size: Vec2::ONE };
        match image {
            SpriteImage::Texture(texture) => match self.texture_size(texture) {
                Some(size) => Region { texture, uv: Vec4::new(0.0, 0.0, 1.0, 1.0), size: size.as_vec2() },
                None => FALLBACK,
            },
            SpriteImage::Atlas(atlas, index) => {
                let Some(&Atlas { texture, grid }) = self.atlases.get(atlas.0 as usize) else { return FALLBACK };
                let Some(size) = self.texture_size(texture) else { return FALLBACK };
                let index = index % (grid.cols * grid.rows);
                let cell = Vec2::new(1.0 / grid.cols as f32, 1.0 / grid.rows as f32);
                let at = Vec2::new((index % grid.cols) as f32, (index / grid.cols) as f32) * cell;
                Region {
                    texture,
                    uv: Vec4::new(at.x, at.y, cell.x, cell.y),
                    size: size.as_vec2() * cell,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets_with(size: UVec2) -> (Assets, TextureId) {
        let mut assets = Assets::default();
        let texture = assets.add_rgba(size, vec![0; (size.x * size.y * 4) as usize]).unwrap();
        (assets, texture)
    }

    #[test]
    fn white_texture_is_builtin() {
        let assets = Assets::default();
        assert_eq!(assets.texture_size(TextureId::WHITE), Some(UVec2::ONE));
    }

    #[test]
    fn grid_atlas_uv_and_size() {
        let (mut assets, texture) = assets_with(UVec2::new(128, 64));
        let atlas = assets.atlas(texture, Grid { cols: 4, rows: 2 });
        let r = assets.region(SpriteImage::Atlas(atlas, 5));
        assert_eq!(r.texture, texture);
        assert_eq!(r.uv, Vec4::new(0.25, 0.5, 0.25, 0.5));
        assert_eq!(r.size, Vec2::new(32.0, 32.0));
    }

    #[test]
    fn atlas_index_wraps() {
        let (mut assets, texture) = assets_with(UVec2::new(4, 4));
        let atlas = assets.atlas(texture, Grid { cols: 2, rows: 2 });
        assert_eq!(assets.region(SpriteImage::Atlas(atlas, 4)), assets.region(SpriteImage::Atlas(atlas, 0)));
    }

    #[test]
    fn invalid_textures_rejected() {
        let mut assets = Assets::default();
        assert!(assets.add_rgba(UVec2::ZERO, vec![]).is_err());
        assert!(assets.add_rgba(UVec2::new(MAX_TEXTURE_SIDE + 1, 1), vec![0; (MAX_TEXTURE_SIDE as usize + 1) * 4]).is_err());
        assert!(assets.add_rgba(UVec2::new(2, 2), vec![0; 4]).is_err());
    }

    #[test]
    fn unknown_ids_fall_back_to_white() {
        let assets = Assets::default();
        let r = assets.region(SpriteImage::Texture(TextureId(42)));
        assert_eq!(r.texture, TextureId::WHITE);
        assert_eq!(assets.region(SpriteImage::Atlas(AtlasId(3), 0)).texture, TextureId::WHITE);
    }

    #[test]
    fn png_roundtrip() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 4]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut assets = Assets::default();
        let texture = assets.load_png(&png).unwrap();
        assert_eq!(assets.texture_size(texture), Some(UVec2::new(3, 2)));
        assert_eq!(&assets.images()[texture.0 as usize].rgba[..4], &[1, 2, 3, 4]);
        assert!(assets.load_png(b"not a png").is_err());
    }
}
