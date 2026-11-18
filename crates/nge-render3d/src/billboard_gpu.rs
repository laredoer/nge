//! 方片和带子的 GPU 部分：两条管线（透明、叠加）、相机和光照的 uniform、场景深度（软粒子）、每帧重写的顶点缓冲。

use std::num::NonZeroU64;

use glam::Mat4;
use nge_assets::ImageId;
use nge_core::{Gfx, wgpu};
use nge_ecs::SCENE_DEPTH_FORMAT;
use wgpu::util::DeviceExt;

use crate::billboards::{Batch, BillboardVertex, Blend};

const SHADER: &str = include_str!("billboards.wgsl");

/// 和 billboards.wgsl 的 `View` 一一对应。
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ViewRaw {
    pub view_proj: [[f32; 4]; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    pub forward: [f32; 4],
    pub ambient: [f32; 4],
    pub env: [f32; 4],
    pub counts: [u32; 4],
    pub dirs: [crate::light::DirRaw; crate::MAX_DIRECTIONAL_LIGHTS],
    pub fog: crate::fog::FogRaw,
    pub sh: [[f32; 4]; 9],
}

impl ViewRaw {
    /// 从网格的全局 uniform 里拿粒子要的那些。
    pub fn new(g: &crate::light::Globals) -> Self {
        Self {
            view_proj: g.view_proj,
            inv_view_proj: g.inv_view_proj,
            forward: g.view_forward,
            ambient: g.ambient,
            env: g.env,
            counts: [g.counts[0], 0, 0, 0],
            dirs: g.dirs,
            fog: g.fog,
            sh: g.sh,
        }
    }
}

/// 着色器源码：前面拼上雾。GL 后端不读深度（同一张纹理既是附件又被采样是反馈回路），软粒子退成硬边。
pub(crate) fn shader_source(gl: bool) -> String {
    let source = format!("{}{SHADER}", crate::fog::FOG_WGSL);
    if !gl {
        return source;
    }
    let (start, end) = (source.find("// SOFT_BEGIN").expect("有 SOFT_BEGIN"), source.find("// SOFT_END").expect("有 SOFT_END"));
    format!("{}fn soft_fade(clip: vec4f, world: vec3f, soft: f32) -> f32 {{\n    return 1.0;\n}}\n{}", &source[..start], &source[end..])
}
const INITIAL_VERTICES: u64 = 1024;

pub(crate) struct BillboardGpu {
    /// [透明, 叠加]
    pipelines: [wgpu::RenderPipeline; 2],
    view: wgpu::Buffer,
    view_layout: wgpu::BindGroupLayout,
    /// 绑着 1 × 1 的假深度：没有软粒子、或在 GL 上时用。
    view_group: wgpu::BindGroup,
    /// 绑着场景深度：深度纹理换了（窗口大小变了）要重建。
    depth_group: Option<wgpu::BindGroup>,
    vertices: wgpu::Buffer,
    capacity: u64,
    /// GL 后端：不读深度。
    pub gl: bool,
}

impl BillboardGpu {
    pub fn new(gfx: &Gfx, texture_layout: &wgpu::BindGroupLayout) -> Self {
        let device = &gfx.device;
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("billboard view"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(size_of::<ViewRaw>() as u64) },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
            ],
        });
        let view = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("billboard view"),
            contents: bytemuck::bytes_of(&ViewRaw { view_proj: Mat4::IDENTITY.to_cols_array_2d(), inv_view_proj: Mat4::IDENTITY.to_cols_array_2d(), ..bytemuck::Zeroable::zeroed() }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("billboard dummy depth"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view_group = view_group(device, &view_layout, &view, &dummy.create_view(&Default::default()));
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("billboards"),
            bind_group_layouts: &[Some(&view_layout), Some(texture_layout)],
            immediate_size: 0,
        });
        let gl = gfx.device.adapter_info().backend == wgpu::Backend::Gl;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("billboards"), source: wgpu::ShaderSource::Wgsl(shader_source(gl).into()) });
        let pipelines = [Blend::Alpha, Blend::Add].map(|blend| pipeline(gfx, &layout, &module, blend));
        Self { pipelines, view, view_layout, view_group, depth_group: None, vertices: vertex_buffer(device, INITIAL_VERTICES), capacity: INITIAL_VERTICES, gl }
    }

    /// 场景深度换了：`depth` 是只取深度那一面的视图。
    pub fn set_depth(&mut self, device: &wgpu::Device, depth: &wgpu::TextureView) {
        self.depth_group = Some(view_group(device, &self.view_layout, &self.view, depth));
    }

    /// 写这一帧的相机、雾和顶点；容量不够时按两倍扩容。
    pub fn prepare(&mut self, gfx: &Gfx, view: &ViewRaw, vertices: &[BillboardVertex]) {
        if vertices.is_empty() {
            return;
        }
        let needed = vertices.len() as u64;
        if needed > self.capacity {
            self.capacity = needed.next_power_of_two();
            self.vertices = vertex_buffer(&gfx.device, self.capacity);
        }
        gfx.queue.write_buffer(&self.view, 0, bytemuck::bytes_of(view));
        gfx.queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(vertices));
    }

    /// `soft`：这个 pass 的深度附件是只读的，可以绑场景深度。
    pub fn draw<'a>(&self, pass: &mut wgpu::RenderPass, batches: &[Batch], soft: bool, texture: impl Fn(ImageId) -> &'a wgpu::BindGroup) {
        if batches.is_empty() {
            return;
        }
        let group = match (&self.depth_group, soft) {
            (Some(depth), true) => depth,
            _ => &self.view_group,
        };
        pass.set_bind_group(0, group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        for batch in batches {
            pass.set_pipeline(&self.pipelines[usize::from(batch.blend == Blend::Add)]);
            pass.set_bind_group(1, texture(batch.texture), &[]);
            pass.draw(batch.vertices.clone(), 0..1);
        }
    }
}

fn view_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, view: &wgpu::Buffer, depth: &wgpu::TextureView) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("billboard view"),
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: view.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) }],
    })
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
    const ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4, 4 => Float32x3, 5 => Float32x4];
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
            entry_point: Some(if blend == Blend::Add { "fs_add" } else { "fs" }),
            targets: &[Some(wgpu::ColorTargetState {
                format: gfx.format,
                blend: Some(if blend == Blend::Add { ADD } else { wgpu::BlendState::ALPHA_BLENDING }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: gfx.pipeline_cache.as_ref(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glsl_errors(source: &str) -> Vec<String> {
        use naga::back::glsl;
        let module = naga::front::wgsl::parse_str(source).expect("WGSL 解析");
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).expect("校验");
        let options = glsl::Options { version: glsl::Version::Embedded { version: 300, is_webgl: true }, ..Default::default() };
        let mut errors = Vec::new();
        for (stage, entry) in [(naga::ShaderStage::Vertex, "vs"), (naga::ShaderStage::Fragment, "fs"), (naga::ShaderStage::Fragment, "fs_add")] {
            let pipeline = glsl::PipelineOptions { shader_stage: stage, entry_point: entry.into(), multiview: None };
            let mut out = String::new();
            let written = glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default()).and_then(|mut w| w.write().map(drop));
            if let Err(e) = written {
                errors.push(format!("{entry}: {e}"));
            }
        }
        errors
    }

    #[test]
    fn the_gl_shader_does_not_read_the_scene_depth() {
        let (gl, other) = (shader_source(true), shader_source(false));
        assert!(!gl.contains("scene_depth") && other.contains("textureLoad(scene_depth"), "GL 上不读深度");
        assert!(gl.contains("fn soft_fade") && gl.contains("fn lighting"), "别的照旧");
        assert_eq!(glsl_errors(&gl), Vec::<String>::new(), "GL 版转得成 GLSL ES");
    }
}
