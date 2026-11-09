//! GPU 上的贴图：每张图只上传一次，2D 和 3D 的渲染器都从这里拿纹理视图。

use bevy_ecs::prelude::Resource;
use glam::UVec2;
use nge_core::{Gfx, wgpu};

use crate::image::{Image, ImageId};
use crate::ktx2::{BlockFormat, Compressed, level_size};
use crate::server::Assets;

#[derive(Resource, Default)]
pub struct GpuImages {
    views: Vec<Option<wgpu::TextureView>>,
}

impl GpuImages {
    /// 上传新加载好的贴图。渲染器每帧画之前调用，没有新贴图时只是一次遍历。
    pub fn sync(&mut self, gfx: &Gfx, images: &Assets<Image>) {
        if self.views.len() < images.len() {
            self.views.resize_with(images.len(), || None);
        }
        for (id, image) in images.iter() {
            let slot = &mut self.views[id.index() as usize];
            if slot.is_none() {
                *slot = Some(upload(gfx, image));
            }
        }
    }

    /// 还没加载完（或者没上传）的贴图返回 None，调用方用 [`ImageId::WHITE`] 代替，并且不要缓存。
    pub fn view(&self, id: ImageId) -> Option<&wgpu::TextureView> {
        self.views.get(id.index() as usize).and_then(Option::as_ref)
    }
}

/// 块压缩格式对应的 wgpu 格式和需要的设备特性。
fn wgpu_format(format: BlockFormat, srgb: bool) -> (wgpu::TextureFormat, wgpu::Features) {
    use wgpu::{AstcBlock, AstcChannel, Features, TextureFormat as F};
    let astc = |block| (F::Astc { block, channel: if srgb { AstcChannel::UnormSrgb } else { AstcChannel::Unorm } }, Features::TEXTURE_COMPRESSION_ASTC);
    match format {
        BlockFormat::Bc1 => (if srgb { F::Bc1RgbaUnormSrgb } else { F::Bc1RgbaUnorm }, Features::TEXTURE_COMPRESSION_BC),
        BlockFormat::Bc3 => (if srgb { F::Bc3RgbaUnormSrgb } else { F::Bc3RgbaUnorm }, Features::TEXTURE_COMPRESSION_BC),
        BlockFormat::Bc7 => (if srgb { F::Bc7RgbaUnormSrgb } else { F::Bc7RgbaUnorm }, Features::TEXTURE_COMPRESSION_BC),
        BlockFormat::Etc2Rgba8 => (if srgb { F::Etc2Rgba8UnormSrgb } else { F::Etc2Rgba8Unorm }, Features::TEXTURE_COMPRESSION_ETC2),
        BlockFormat::Astc4x4 => astc(AstcBlock::B4x4),
        BlockFormat::Astc6x6 => astc(AstcBlock::B6x6),
        BlockFormat::Astc8x8 => astc(AstcBlock::B8x8),
    }
}

/// 这台设备能不能直接用这张压缩贴图：有对应特性、宽高是块的整数倍（wgpu 的要求）。
pub(crate) fn direct_format(c: &Compressed, size: UVec2, features: wgpu::Features) -> Option<wgpu::TextureFormat> {
    let (format, need) = wgpu_format(c.format, c.srgb);
    (features.contains(need) && size % c.format.block() == UVec2::ZERO).then_some(format)
}

fn upload(gfx: &Gfx, image: &Image) -> wgpu::TextureView {
    let Some(c) = &image.compressed else {
        return upload_rgba(gfx, image.size, &image.rgba, true);
    };
    if let Some(format) = direct_format(c, image.size, gfx.device.features()) {
        return upload_compressed(gfx, image.size, c, format);
    }
    // 设备不支持这种格式：在 CPU 上把每一级解成 RGBA8，画出来一样（mipmap 也在），只是没省显存
    log::info!("设备不支持 {:?}（或尺寸 {} 不是块的整数倍），贴图解成 RGBA8 上传", c.format, image.size);
    match (0..c.levels.len()).map(|level| c.decode_level(image.size, level)).collect::<Result<Vec<_>, _>>() {
        Ok(levels) => upload_rgba_levels(gfx, image.size, &levels, c.srgb),
        Err(e) => {
            log::error!("{e}，用白色代替");
            upload_rgba(gfx, UVec2::ONE, &[255; 4], true)
        }
    }
}

fn texture(gfx: &Gfx, size: UVec2, levels: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
    gfx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("image"),
        size: wgpu::Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn upload_rgba(gfx: &Gfx, size: UVec2, rgba: &[u8], srgb: bool) -> wgpu::TextureView {
    upload_rgba_levels(gfx, size, &[rgba], srgb)
}

fn upload_rgba_levels(gfx: &Gfx, size: UVec2, levels: &[impl AsRef<[u8]>], srgb: bool) -> wgpu::TextureView {
    let format = if srgb { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
    let texture = texture(gfx, size, levels.len() as u32, format);
    for (level, rgba) in levels.iter().enumerate() {
        let s = level_size(size, level);
        gfx.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            rgba.as_ref(),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * s.x), rows_per_image: Some(s.y) },
            wgpu::Extent3d { width: s.x, height: s.y, depth_or_array_layers: 1 },
        );
    }
    texture.create_view(&Default::default())
}

fn upload_compressed(gfx: &Gfx, size: UVec2, c: &Compressed, format: wgpu::TextureFormat) -> wgpu::TextureView {
    let texture = texture(gfx, size, c.levels.len() as u32, format);
    let block = c.format.block();
    for (level, data) in c.levels.iter().enumerate() {
        // 小于一块的级按一整块拷（物理尺寸）
        let blocks = c.format.blocks(level_size(size, level));
        let physical = blocks * block;
        gfx.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(blocks.x * c.format.block_bytes() as u32), rows_per_image: Some(blocks.y) },
            wgpu::Extent3d { width: physical.x, height: physical.y, depth_or_array_layers: 1 },
        );
    }
    texture.create_view(&Default::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compressed(format: BlockFormat, srgb: bool) -> Compressed {
        Compressed { format, srgb, levels: vec![] }
    }

    #[test]
    fn picks_the_format_only_when_the_device_has_it_and_the_size_is_aligned() {
        use wgpu::{Features, TextureFormat as F};
        let all = Features::TEXTURE_COMPRESSION_BC | Features::TEXTURE_COMPRESSION_ETC2 | Features::TEXTURE_COMPRESSION_ASTC;
        let bc3 = compressed(BlockFormat::Bc3, true);
        assert_eq!(direct_format(&bc3, UVec2::new(8, 4), all), Some(F::Bc3RgbaUnormSrgb));
        assert_eq!(direct_format(&bc3, UVec2::new(8, 4), Features::TEXTURE_COMPRESSION_ASTC), None, "没有 BC 特性");
        assert_eq!(direct_format(&bc3, UVec2::new(8, 6), all), None, "高不是 4 的倍数");
        assert_eq!(direct_format(&bc3, UVec2::new(6, 4), all), None, "宽不是 4 的倍数");
        let astc = compressed(BlockFormat::Astc6x6, false);
        assert_eq!(direct_format(&astc, UVec2::new(12, 6), all), Some(F::Astc { block: wgpu::AstcBlock::B6x6, channel: wgpu::AstcChannel::Unorm }));
        assert_eq!(direct_format(&astc, UVec2::new(8, 8), all), None, "6×6 的块，8 不对齐");
        assert_eq!(direct_format(&compressed(BlockFormat::Etc2Rgba8, true), UVec2::splat(4), Features::TEXTURE_COMPRESSION_ETC2), Some(F::Etc2Rgba8UnormSrgb));
        // 每种格式都映射到块大小一样的 wgpu 格式
        for f in [BlockFormat::Bc1, BlockFormat::Bc3, BlockFormat::Bc7, BlockFormat::Etc2Rgba8, BlockFormat::Astc4x4, BlockFormat::Astc6x6, BlockFormat::Astc8x8] {
            for srgb in [false, true] {
                let (w, _) = wgpu_format(f, srgb);
                assert_eq!(w.block_dimensions(), (f.block().x, f.block().y), "{f:?}");
                assert_eq!(w.block_copy_size(None), Some(f.block_bytes() as u32), "{f:?}");
                assert_eq!(w.is_srgb(), srgb, "{f:?}");
            }
        }
    }
}
