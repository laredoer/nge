//! 方片和带子的 GPU 部分：两条管线（透明、叠加）、一个 `view_proj` uniform、每帧重写的顶点缓冲。

use std::num::NonZeroU64;

use glam::Mat4;
use nge_assets::ImageId;
use nge_core::{Gfx, wgpu};
use nge_ecs::SCENE_DEPTH_FORMAT;
use wgpu::util::DeviceExt;

use crate::billboards::{Batch, BillboardVertex, Blend};

const SHADER: &str = include_str!("billboards.wgsl");
const INITIAL_VERTICES: u64 = 1024;

pub(crate) struct BillboardGpu {
    /// [透明, 叠加]
    pipelines: [wgpu::RenderPipeline; 2],
    view: wgpu::Buffer,
    view_group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    capacity: u64,
}

impl BillboardGpu {
    pub fn new(gfx: &Gfx, texture_layout: &wgpu::BindGroupLayout) -> Self {
        let device = &gfx.device;
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("billboard view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(64) },
                count: None,
            }],
        });
        let view = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("billboard view"),
            contents: bytemuck::bytes_of(&Mat4::IDENTITY),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let view_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("billboard view"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: view.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("billboards"),
            bind_group_layouts: &[Some(&view_layout), Some(texture_layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("billboards"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let pipelines = [Blend::Alpha, Blend::Add].map(|blend| pipeline(gfx, &layout, &module, blend));
        Self { pipelines, view, view_group, vertices: vertex_buffer(device, INITIAL_VERTICES), capacity: INITIAL_VERTICES }
    }

    /// 写这一帧的相机和顶点；容量不够时按两倍扩容。
    pub fn prepare(&mut self, gfx: &Gfx, view_proj: Mat4, vertices: &[BillboardVertex]) {
        if vertices.is_empty() {
            return;
        }
        let needed = vertices.len() as u64;
        if needed > self.capacity {
            self.capacity = needed.next_power_of_two();
            self.vertices = vertex_buffer(&gfx.device, self.capacity);
        }
        gfx.queue.write_buffer(&self.view, 0, bytemuck::bytes_of(&view_proj));
        gfx.queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(vertices));
    }

    pub fn draw<'a>(&self, pass: &mut wgpu::RenderPass, batches: &[Batch], texture: impl Fn(ImageId) -> &'a wgpu::BindGroup) {
        if batches.is_empty() {
            return;
        }
        pass.set_bind_group(0, &self.view_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        for batch in batches {
            pass.set_pipeline(&self.pipelines[usize::from(batch.blend == Blend::Add)]);
            pass.set_bind_group(1, texture(batch.texture), &[]);
            pass.draw(batch.vertices.clone(), 0..1);
        }
    }
}

fn vertex_buffer(device: &wgpu::Device, count: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("billboard vertices"),
        size: count * size_of::<BillboardVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// 叠加：颜色按透明度加上去，目标的透明度不变。
const ADD: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
    alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
};

fn pipeline(gfx: &Gfx, layout: &wgpu::PipelineLayout, module: &wgpu::ShaderModule, blend: Blend) -> wgpu::RenderPipeline {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4];
    gfx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("billboards"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            buffers: &[Some(wgpu::VertexBufferLayout { array_stride: size_of::<BillboardVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ATTRIBUTES })],
            compilation_options: Default::default(),
        },
        // 方片和带子两面都画（带子会翻过来）
        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
        // 被模型挡住，自己不挡别的
        depth_stencil: Some(wgpu::DepthStencilState {
            format: SCENE_DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState {
                format: gfx.format,
                blend: Some(if blend == Blend::Add { ADD } else { wgpu::BlendState::ALPHA_BLENDING }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}
