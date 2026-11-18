//! 网格 pass 的深度纹理（见 docs/particles3d-more.md 的软粒子）：平时只当深度附件；第一次有软粒子时重建成也能绑成纹理的，
//! 之后一直是。移动端的 tile GPU 上，能绑成纹理的深度要多存一次，没有软粒子的场景不付这个代价。

use nge_core::wgpu;
use nge_ecs::SCENE_DEPTH_FORMAT;

#[derive(Default)]
pub(crate) struct DepthTarget {
    texture: Option<(wgpu::Texture, wgpu::TextureView)>,
    readable: bool,
}

impl DepthTarget {
    /// 按这一帧的大小准备好；`readable` 时（之后也一直）带上 `TEXTURE_BINDING`。新建了能读的纹理时返回只有深度的视图，给软粒子绑。
    pub(crate) fn prepare(&mut self, device: &wgpu::Device, size: wgpu::Extent3d, readable: bool) -> Option<wgpu::TextureView> {
        let size = wgpu::Extent3d { depth_or_array_layers: 1, ..size };
        self.readable |= readable;
        let fresh = self.texture.as_ref().is_some_and(|(t, _)| t.size() == size && (!self.readable || t.usage().contains(wgpu::TextureUsages::TEXTURE_BINDING)));
        if fresh {
            return None;
        }
        let usage = if self.readable { wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING } else { wgpu::TextureUsages::RENDER_ATTACHMENT };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mesh depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_DEPTH_FORMAT,
            usage,
            view_formats: &[],
        });
        let depth_only = self.readable.then(|| texture.create_view(&wgpu::TextureViewDescriptor { aspect: wgpu::TextureAspect::DepthOnly, ..Default::default() }));
        self.texture = Some((texture.clone(), texture.create_view(&Default::default())));
        depth_only
    }

    pub(crate) fn view(&self) -> &wgpu::TextureView {
        &self.texture.as_ref().expect("先 prepare").1
    }

    #[cfg(test)]
    fn usage(&self) -> Option<wgpu::TextureUsages> {
        self.texture.as_ref().map(|(t, _)| t.usage())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Option<wgpu::Device> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        Some(pollster::block_on(adapter.request_device(&Default::default())).ok()?.0)
    }

    #[test]
    fn the_depth_becomes_readable_only_once_soft_particles_show_up() {
        let Some(device) = device() else { return };
        let size = |w| wgpu::Extent3d { width: w, height: 32, depth_or_array_layers: 1 };
        let mut depth = DepthTarget::default();
        assert!(depth.prepare(&device, size(32), false).is_none());
        assert!(!depth.usage().unwrap().contains(wgpu::TextureUsages::TEXTURE_BINDING), "没有软粒子时不能读");
        assert!(depth.prepare(&device, size(32), false).is_none(), "大小没变不重建");
        assert!(depth.prepare(&device, size(32), true).is_some(), "第一次有软粒子：重建，给出视图");
        assert!(depth.usage().unwrap().contains(wgpu::TextureUsages::TEXTURE_BINDING));
        assert!(depth.prepare(&device, size(32), false).is_none(), "软粒子没了也不来回重建");
        assert!(depth.prepare(&device, size(64), false).is_some(), "换大小时还是能读的，重新给视图");
        assert!(depth.usage().unwrap().contains(wgpu::TextureUsages::TEXTURE_BINDING));
    }
}
