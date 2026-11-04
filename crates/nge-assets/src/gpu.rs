//! GPU 上的贴图：每张图只上传一次，2D 和 3D 的渲染器都从这里拿纹理视图。

use bevy_ecs::prelude::Resource;
use nge_core::{Gfx, wgpu};

use crate::image::{Image, ImageId};
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

fn upload(gfx: &Gfx, image: &Image) -> wgpu::TextureView {
    let size = wgpu::Extent3d { width: image.size.x, height: image.size.y, depth_or_array_layers: 1 };
    let texture = gfx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("image"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gfx.queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &image.rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * image.size.x), rows_per_image: Some(image.size.y) },
        size,
    );
    texture.create_view(&Default::default())
}
