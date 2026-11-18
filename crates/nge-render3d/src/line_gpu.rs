//! 调试画线的 GPU 部分：两条 `LineList` 管线（按深度被挡住、画在最前面），一个 `view_proj` uniform，每帧重写的顶点缓冲。

use std::num::NonZeroU64;

use glam::Mat4;
use nge_core::{Gfx, wgpu};
use nge_ecs::SCENE_DEPTH_FORMAT;
use wgpu::util::DeviceExt;

use crate::gizmos::{Gizmos3d, LineVertex};

const SHADER: &str = include_str!("lines.wgsl");
const INITIAL_VERTICES: u64 = 1024;

pub(crate) struct LineGpu {
    /// [按深度被挡住, 画在最前面]
    pipelines: [wgpu::RenderPipeline; 2],
    view: wgpu::Buffer,
    view_group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    capacity: u64,
    /// 这一帧两组各多少个顶点。
    counts: [u32; 2],
}

impl LineGpu {
    pub fn new(gfx: &Gfx) -> Self {
        let device = &gfx.device;
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("line view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(64) },
                count: None,
            }],
        });
        let view = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("line view"),
            contents: bytemuck::bytes_of(&Mat4::IDENTITY),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let view_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("line view"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: view.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("lines"), bind_group_layouts: &[Some(&view_layout)], immediate_size: 0 });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("lines"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let pipelines = [wgpu::CompareFunction::Less, wgpu::CompareFunction::Always].map(|compare| pipeline(gfx, &layout, &module, compare));
        Self { pipelines, view, view_group, vertices: vertex_buffer(device, INITIAL_VERTICES), capacity: INITIAL_VERTICES, counts: [0; 2] }
    }

    /// 写这一帧的相机和顶点（先按深度的，再画在最前面的）；容量不够时按两倍扩容。
    pub fn prepare(&mut self, gfx: &Gfx, view_proj: Mat4, gizmos: &Gizmos3d) {
        let (depth, top) = (&gizmos.depth_tested().0, &gizmos.top().0);
        self.counts = [depth.len() as u32, top.len() as u32];
        let needed = (depth.len() + top.len()) as u64;
        if needed == 0 {
            return;
        }
        if needed > self.capacity {
            self.capacity = needed.next_power_of_two();
            self.vertices = vertex_buffer(&gfx.device, self.capacity);
        }
        gfx.queue.write_buffer(&self.view, 0, bytemuck::bytes_of(&view_proj));
        gfx.queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(depth));
        gfx.queue.write_buffer(&self.vertices, (depth.len() * size_of::<LineVertex>()) as u64, bytemuck::cast_slice(top));
    }

    /// 这一帧有几次 draw。
    pub fn draws(&self) -> u32 {
        self.counts.iter().filter(|&&n| n > 0).count() as u32
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass) {
        if self.counts == [0, 0] {
            return;
        }
        pass.set_bind_group(0, &self.view_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        let mut start = 0;
        for (pipeline, &n) in self.pipelines.iter().zip(&self.counts) {
            if n > 0 {
                pass.set_pipeline(pipeline);
                pass.draw(start..start + n, 0..1);
            }
            start += n;
        }
    }
}

fn vertex_buffer(device: &wgpu::Device, count: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("line vertices"),
        size: count * size_of::<LineVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// 不写深度：按 `compare` 和模型比，自己不挡别的。
fn pipeline(gfx: &Gfx, layout: &wgpu::PipelineLayout, module: &wgpu::ShaderModule, compare: wgpu::CompareFunction) -> wgpu::RenderPipeline {
    const ATTRIBUTES: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];
    gfx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("lines"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            buffers: &[Some(wgpu::VertexBufferLayout { array_stride: size_of::<LineVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ATTRIBUTES })],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: SCENE_DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(compare),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState { format: gfx.format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: gfx.pipeline_cache.as_ref(),
    })
}
