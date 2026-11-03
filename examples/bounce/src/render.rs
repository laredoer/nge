//! 弹球的渲染：一个全屏三角形，片元着色器按 uniform 里的小球列表画圆。

use nge_core::{Gfx, wgpu};

use crate::sim::{MAX_BALLS, Sim};

const SHADER: &str = r#"
struct Uniforms {
    size: vec2f,
    scale: f32,
    time: f32,
    count: u32,
    _p0: u32, _p1: u32, _p2: u32,
    balls: array<vec4f, 32>,
}

@group(0) @binding(0) var<uniform> u: Uniforms;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(p * 2.0 - 1.0, 0.0, 1.0);
}

fn hue(h: f32) -> vec3f {
    let k = vec3f(0.0, 2.0 / 3.0, 1.0 / 3.0);
    return clamp(abs(fract(h + k) * 6.0 - 3.0) - 1.0, vec3f(0.0), vec3f(1.0));
}

@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let p = pos.xy / u.scale;
    let uv = p / max(u.size, vec2f(1.0));
    var col = mix(vec3f(0.03, 0.04, 0.09), vec3f(0.09, 0.03, 0.14), uv.y);
    let g = abs(fract(p / 32.0) - 0.5);
    col += vec3f(0.035) * step(0.47, max(g.x, g.y));
    for (var i = 0u; i < u.count; i++) {
        let b = u.balls[i];
        let d = length(p - b.xy);
        let c = hue(b.w + u.time * 0.05);
        let glow = exp(-max(d - b.z, 0.0) / 12.0) * 0.45;
        let body = 1.0 - smoothstep(b.z - 1.0, b.z + 1.0, d);
        let shade = 1.15 - 0.5 * length(p - b.xy + vec2f(b.z * 0.35)) / b.z;
        col = mix(col + c * glow, c * shade, body);
    }
    return vec4f(col, 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    size: [f32; 2],
    scale: f32,
    time: f32,
    count: u32,
    _pad: [u32; 3],
    balls: [[f32; 4]; MAX_BALLS],
}

pub struct Renderer {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl Renderer {
    pub fn new(gfx: &Gfx) -> Self {
        let module = gfx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("game"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let uniforms = gfx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pipeline = gfx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("game"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(gfx.format.into())],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let bind_group = gfx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniforms"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        Self { pipeline, uniforms, bind_group }
    }

    pub fn render(&self, gfx: &Gfx, sim: &Sim, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let mut balls = [[0.0; 4]; MAX_BALLS];
        for (slot, b) in balls.iter_mut().zip(&sim.balls) {
            *slot = [b.pos[0], b.pos[1], b.radius, b.hue];
        }
        let uniforms = Uniforms {
            size: gfx.viewport.size,
            scale: gfx.viewport.scale(),
            time: sim.time,
            count: sim.balls.len() as u32,
            _pad: [0; 3],
            balls,
        };
        gfx.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("bounce"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
