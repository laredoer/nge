//! 方向光阴影：从开了 `shadows` 的方向光那边正交地画一张深度图，主 pass 里 3×3 PCF 采样（见 docs/3d-more.md）。

use std::num::NonZeroU64;

use bevy_ecs::prelude::Resource;
use glam::{Mat4, Vec3};
use glam::camera::rh::proj::directx;
use nge_core::{Gfx, wgpu};

/// 阴影图的设置。阴影图按光的方向框住所有 3D 物体（模型少的场景够用；物体铺得很开时一个像素管的范围大，阴影边缘会粗）。
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct Shadows {
    /// 阴影图边长（像素），会被限制在 [64, 4096]。
    pub size: u32,
}

impl Default for Shadows {
    fn default() -> Self {
        Self { size: 2048 }
    }
}

impl Shadows {
    pub(crate) fn clamped_size(&self) -> u32 {
        self.size.clamp(64, 4096)
    }
}

pub(crate) const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// 光的方向 + 所有物体的世界包围盒 → 光的正交矩阵（世界 → 裁剪空间，深度 0–1）和一个阴影图像素在世界里多大。
/// 没有物体、方向是 0 时是 `None`。
pub(crate) fn fit(direction: Vec3, boxes: impl IntoIterator<Item = (Vec3, Vec3)>, size: u32) -> Option<(Mat4, f32)> {
    let forward = direction.try_normalize()?;
    let up = if forward.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    // 光的视图：看向 -z，原点在世界原点（正交投影，只要方向）
    let right = forward.cross(up).normalize();
    let view = Mat4::from_mat3(glam::Mat3::from_cols(right, right.cross(forward), -forward).transpose());
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for (min, max) in boxes {
        for i in 0..8 {
            let corner = Vec3::new(if i & 1 == 0 { min.x } else { max.x }, if i & 2 == 0 { min.y } else { max.y }, if i & 4 == 0 { min.z } else { max.z });
            let p = view.transform_point3(corner);
            (lo, hi) = (lo.min(p), hi.max(p));
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        return None;
    }
    // 四周留一点，边上的物体不被切掉；扁平的方向至少一个单位
    let pad = ((hi - lo).max_element() * 0.02).max(1e-3);
    let (lo, hi) = (lo - Vec3::splat(pad), hi + Vec3::splat(pad));
    let side = (hi.x - lo.x).max(hi.y - lo.y);
    // 看向 -z：近的 z 大
    let proj = directx::orthographic(lo.x, hi.x, lo.y, hi.y, -hi.z, -lo.z);
    Some((proj * view, side / size as f32))
}

/// 阴影 pass 的 GPU 资源。主 pass 的全局 bind group 要引用阴影图，所以图换了尺寸时由渲染器重建它。
pub(crate) struct ShadowGpu {
    /// [普通, 蒙皮]
    pub pipelines: [wgpu::RenderPipeline; 2],
    pub light: wgpu::Buffer,
    pub light_group: wgpu::BindGroup,
    pub sampler: wgpu::Sampler,
    pub map: wgpu::TextureView,
    pub size: u32,
}

impl ShadowGpu {
    pub fn new(gfx: &Gfx, draw_layout: &wgpu::BindGroupLayout) -> Self {
        let device = &gfx.device;
        let light_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow light"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(64) },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow"),
            bind_group_layouts: &[Some(&light_layout), Some(draw_layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("shadow"), source: wgpu::ShaderSource::Wgsl(include_str!("shadow.wgsl").into()) });
        let pipeline = |skinned: bool| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some(if skinned { "vs_skinned" } else { "vs" }),
                buffers: &crate::renderer::vertex_layouts(skinned),
                compilation_options: Default::default(),
            },
            // 两面都画：薄片、单面的平面也挡光
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: SHADOW_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                // 斜着照的面按斜率多推一点，免得自己挡自己（阴影痘）
                bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
            }),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        let light = device.create_buffer(&wgpu::BufferDescriptor { label: Some("shadow light"), size: 64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let light_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow light"),
            layout: &light_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: light.as_entire_binding() }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        Self { pipelines: [pipeline(false), pipeline(true)], light, light_group, sampler, map: map(gfx, 1), size: 1 }
    }

    /// 阴影图的尺寸变了就重建，返回有没有重建（主 pass 的 bind group 要跟着换）。
    pub fn resize(&mut self, gfx: &Gfx, size: u32) -> bool {
        if size == self.size {
            return false;
        }
        (self.map, self.size) = (map(gfx, size), size);
        true
    }
}

fn map(gfx: &Gfx, size: u32) -> wgpu::TextureView {
    gfx.device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow map"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ndc(m: Mat4, p: Vec3) -> Vec3 {
        let c = m * p.extend(1.0);
        c.truncate() / c.w
    }

    #[test]
    fn fit_frames_every_box_in_the_light_view() {
        let boxes = [(Vec3::new(-3.0, 0.0, -3.0), Vec3::new(3.0, 0.1, 3.0)), (Vec3::new(0.5, 1.0, 0.5), Vec3::new(1.0, 2.0, 1.0))];
        for direction in [Vec3::NEG_Y, Vec3::new(1.0, -1.0, 0.3), Vec3::X] {
            let (m, texel) = fit(direction, boxes, 1024).unwrap();
            for (min, max) in boxes {
                for p in [min, max, Vec3::new(min.x, max.y, min.z)] {
                    let q = ndc(m, p);
                    assert!(q.x.abs() <= 1.0 && q.y.abs() <= 1.0 && (0.0..=1.0).contains(&q.z), "{direction}: {p} → {q}");
                }
            }
            assert!(texel > 0.0 && texel < 0.02, "{texel}");
        }
        // 正上方照下来：高的点离光近（深度小）
        let (m, _) = fit(Vec3::NEG_Y, boxes, 1024).unwrap();
        assert!(ndc(m, Vec3::new(0.7, 2.0, 0.7)).z < ndc(m, Vec3::new(0.7, 0.0, 0.7)).z);
    }

    #[test]
    fn nothing_to_fit() {
        assert!(fit(Vec3::NEG_Y, [], 1024).is_none());
        assert!(fit(Vec3::ZERO, [(Vec3::ZERO, Vec3::ONE)], 1024).is_none());
    }
}
