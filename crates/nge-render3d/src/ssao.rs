//! 3D 屏幕空间环境光遮蔽（SSAO）与 HDR 色调映射配置。

use std::num::NonZeroU64;

use bevy_ecs::prelude::Resource;
use bytemuck::{Pod, Zeroable};
use nge_core::{Gfx, wgpu};

const SSAO_SHADER: &str = include_str!("ssao.wgsl");

/// 3D 渲染器内置色调映射（在写入 8-bit sRGB 帧缓冲前对真实线性 HDR 亮度做曲线压缩）。
/// 默认 [`ToneMapMode::None`]，保证与既有测试 100% 兼容。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ToneMapMode {
    #[default]
    None,
    /// ACES Filmic（Narkowicz 近似）。
    Aces,
    /// AgX（柔和保留亮部色彩，避免高光偏黄）。
    Agx,
    /// Reinhard 经典压缩。
    Reinhard,
}

impl ToneMapMode {
    pub(crate) fn as_f32(self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Aces => 1.0,
            Self::Agx => 2.0,
            Self::Reinhard => 3.0,
        }
    }
}

/// 3D HDR 色调映射资源。
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct ToneMap {
    pub mode: ToneMapMode,
    /// 曝光倍率（默认 1.0）。
    pub exposure: f32,
    /// 饱和度倍率（默认 1.0）。
    pub saturation: f32,
}

impl Default for ToneMap {
    fn default() -> Self {
        Self { mode: ToneMapMode::None, exposure: 1.0, saturation: 1.0 }
    }
}

/// 3D 屏幕空间接触暗部 / 环境光遮蔽（SSAO）资源。默认关闭。
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct Ssao {
    pub enabled: bool,
    /// 遮蔽强度（0..2，默认 0.65）。
    pub intensity: f32,
    /// 屏幕空间采样半径（UV 比例，默认 0.022）。
    pub radius: f32,
    /// 深度偏移（防平面自遮蔽，默认 0.0004）。
    pub bias: f32,
}

impl Default for Ssao {
    fn default() -> Self {
        Self { enabled: false, intensity: 0.65, radius: 0.022, bias: 0.0004 }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SsaoParamsRaw {
    params: [f32; 4],
    texel: [f32; 4],
}

pub(crate) struct SsaoGpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    group: Option<wgpu::BindGroup>,
}

impl SsaoGpu {
    pub(crate) fn new(gfx: &Gfx) -> Self {
        let device = &gfx.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ssao"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(size_of::<SsaoParamsRaw>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ssao"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ssao"),
            source: wgpu::ShaderSource::Wgsl(SSAO_SHADER.into()),
        });
        // 乘法混合：dst.rgb *= src.rgb，不需要额外的离屏颜色纹理
        let multiply = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ssao"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState { format: gfx.format, blend: Some(multiply), write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: gfx.pipeline_cache.as_ref(),
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ssao params"),
            size: size_of::<SsaoParamsRaw>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, layout, uniform, group: None }
    }

    pub(crate) fn set_depth(&mut self, device: &wgpu::Device, depth_only: &wgpu::TextureView) {
        self.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ssao"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth_only) },
            ],
        }));
    }

    pub(crate) fn draw(&self, gfx: &Gfx, pass: &mut wgpu::RenderPass, ssao: Ssao, size: wgpu::Extent3d) {
        let Some(group) = self.group.as_ref() else { return };
        let w = size.width.max(1) as f32;
        let h = size.height.max(1) as f32;
        let raw = SsaoParamsRaw {
            params: [ssao.intensity.clamp(0.0, 2.0), ssao.radius.clamp(0.002, 0.1), ssao.bias.clamp(0.0, 0.05), 0.0],
            texel: [1.0 / w, 1.0 / h, w, h],
        };
        gfx.queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&raw));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssao_shader_validates() {
        let module = naga::front::wgsl::parse_str(SSAO_SHADER).expect("SSAO WGSL 解析");
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .expect("SSAO WGSL 校验");
    }

    #[test]
    fn defaults_keep_backwards_compatibility() {
        assert_eq!(ToneMap::default().mode, ToneMapMode::None);
        assert!(!Ssao::default().enabled);
    }
}
