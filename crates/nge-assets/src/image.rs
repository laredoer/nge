//! 贴图：CPU 上的 RGBA8（sRGB）或 GPU 块压缩数据（KTX2），2D 和 3D 共用。

use glam::UVec2;

use crate::ktx2::{self, Compressed};
use crate::server::{Asset, Assets, Handle, LoadContext};

/// 纹理边长上限：wgpu 默认设备限制（WebGPU 默认值）。
pub const MAX_TEXTURE_SIDE: u32 = 8192;

pub type ImageId = Handle<Image>;

impl Handle<Image> {
    /// 内置的 1×1 白色贴图：纯色精灵、没贴图的材质、还没加载完的贴图都用它。
    pub const WHITE: Self = Handle::new(0);
}

#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    /// 像素尺寸（压缩贴图也是），精灵按它算大小。
    pub size: UVec2,
    /// RGBA8；有 `compressed` 时是空的。
    pub rgba: Vec<u8>,
    /// GPU 块压缩数据（来自 KTX2），带 mipmap。
    pub compressed: Option<Compressed>,
}

impl Image {
    /// 尺寸为 0、超过 [`MAX_TEXTURE_SIDE`] 或长度不符时返回错误。
    pub fn from_rgba(size: UVec2, rgba: Vec<u8>) -> Result<Self, String> {
        if size.min_element() == 0 || size.max_element() > MAX_TEXTURE_SIDE {
            return Err(format!("贴图尺寸 {size} 不合法（每边 1..={MAX_TEXTURE_SIDE}）"));
        }
        let expected = u64::from(size.x) * u64::from(size.y) * 4;
        if rgba.len() as u64 != expected {
            return Err(format!("像素数据 {} 字节，尺寸 {size} 需要 {expected} 字节", rgba.len()));
        }
        Ok(Self { size, rgba, compressed: None })
    }

    /// 解码 PNG / JPEG / KTX2（按文件头认）。
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.starts_with(&ktx2::MAGIC) {
            return ktx2::parse(bytes);
        }
        let image = image::load_from_memory(bytes).map_err(|e| format!("图片解码失败: {e}"))?.into_rgba8();
        Self::from_rgba(UVec2::new(image.width(), image.height()), image.into_raw())
    }
}

impl Asset for Image {
    fn decode(bytes: Vec<u8>, _ctx: &LoadContext) -> Result<Self, String> {
        Image::decode(&bytes)
    }
}

impl Assets<Image> {
    /// 第 0 号是白色贴图（[`ImageId::WHITE`]）。
    pub fn with_white() -> Self {
        let mut images = Self::default();
        images.add(Image { size: UVec2::ONE, rgba: vec![255; 4], compressed: None });
        images
    }

    pub fn add_rgba(&mut self, size: UVec2, rgba: Vec<u8>) -> Result<ImageId, String> {
        Ok(self.add(Image::from_rgba(size, rgba)?))
    }

    /// 解码 PNG / JPEG / KTX2（通常来自 `include_bytes!`）。
    pub fn load_png(&mut self, bytes: &[u8]) -> Result<ImageId, String> {
        Ok(self.add(Image::decode(bytes)?))
    }

    /// 已加载的贴图的像素尺寸。
    pub fn size(&self, id: ImageId) -> Option<UVec2> {
        self.get(id).map(|i| i.size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_rgba() {
        let mut images = Assets::<Image>::with_white();
        assert_eq!(images.size(ImageId::WHITE), Some(UVec2::ONE));
        let id = images.add_rgba(UVec2::new(2, 1), vec![0; 8]).unwrap();
        assert_eq!(images.size(id), Some(UVec2::new(2, 1)));
        assert!(images.add_rgba(UVec2::new(2, 1), vec![0; 4]).is_err());
        assert!(images.add_rgba(UVec2::new(0, 1), vec![]).is_err());
        assert!(images.add_rgba(UVec2::new(MAX_TEXTURE_SIDE + 1, 1), vec![0; (MAX_TEXTURE_SIDE as usize + 1) * 4]).is_err());
        assert_eq!(images.len(), 2, "失败的不占编号");
    }

    #[test]
    fn decodes_png() {
        let mut png = Vec::new();
        image::RgbaImage::from_raw(1, 2, vec![255, 0, 0, 255, 0, 0, 255, 128])
            .unwrap()
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let image = Image::decode(&png).unwrap();
        assert_eq!(image.size, UVec2::new(1, 2));
        assert_eq!(image.rgba, [255, 0, 0, 255, 0, 0, 255, 128]);
        assert!(Image::decode(b"nope").is_err());
    }

    #[test]
    fn decodes_ktx2_by_header() {
        let bytes = ktx2::write(Some(ktx2::BlockFormat::Bc3), true, UVec2::new(4, 4), &[vec![0; 16]]);
        let image = Image::decode(&bytes).unwrap();
        assert_eq!((image.size, image.compressed.map(|c| c.format)), (UVec2::new(4, 4), Some(ktx2::BlockFormat::Bc3)));
        assert!(Image::decode(&bytes[..20]).unwrap_err().contains("KTX2"), "头对但是坏了，报 KTX2 的错");
    }
}
