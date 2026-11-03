//! 精灵渲染器：每帧从 World 提取精灵 → 合批 → 一个实例缓冲 + 每批一次 draw。

use bevy_ecs::prelude::*;
use glam::{Vec2, Vec4};
use nge_core::{Gfx, wgpu};
use nge_ecs::{App, Input, Plugin, PreUpdate, Renderer, Screen};

use crate::assets::{Assets, TextureId};
use crate::batch::{Batch, Instance, Item, batch};
use crate::camera::{Camera2d, Transform2d};
use crate::sprite::Sprite;

const SHADER: &str = r#"
struct Globals { view_proj: mat4x4f }
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct Inst {
    @location(0) pos: vec2f,
    @location(1) size: vec2f,
    @location(2) rotation_z: vec2f,
    @location(3) uv: vec4f,
    @location(4) color: vec4f,
}
struct Out {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, i: Inst) -> Out {
    var corners = array<vec2f, 6>(
        vec2f(-0.5, -0.5), vec2f(0.5, -0.5), vec2f(0.5, 0.5),
        vec2f(-0.5, -0.5), vec2f(0.5, 0.5), vec2f(-0.5, 0.5),
    );
    let c = corners[vi];
    let local = c * i.size;
    let s = sin(i.rotation_z.x);
    let k = cos(i.rotation_z.x);
    let p = vec2f(local.x * k - local.y * s, local.x * s + local.y * k) + i.pos;
    var out: Out;
    out.clip = globals.view_proj * vec4f(p, 0.0, 1.0);
    // 世界 y 朝上、图片 v 朝下
    out.uv = i.uv.xy + vec2f(c.x + 0.5, 0.5 - c.y) * i.uv.zw;
    out.color = i.color;
    return out;
}

@fragment
fn fs(in: Out) -> @location(0) vec4f {
    return textureSample(tex, samp, in.uv) * in.color;
}
"#;

const INITIAL_CAPACITY: u64 = 1024;

/// 每帧的清屏颜色（线性）。
#[derive(Resource, Debug, Clone, Copy)]
pub struct ClearColor(pub wgpu::Color);

impl Default for ClearColor {
    fn default() -> Self {
        Self(wgpu::Color { r: 0.02, g: 0.02, b: 0.04, a: 1.0 })
    }
}

/// 注册 [`Assets`]、[`ClearColor`]、指针世界坐标换算和精灵渲染器。
pub struct Render2dPlugin;

impl Plugin for Render2dPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Assets::default())
            .insert_resource(ClearColor::default())
            .add_systems(PreUpdate, pointer_world.in_set(PointerWorld))
            .add_renderer(SpriteRenderer::default());
    }
}

/// 把指针换算成世界坐标的系统（PreUpdate）。同帧要改相机的系统排在它前面。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PointerWorld;

fn pointer_world(screen: Res<Screen>, cameras: Query<&Camera2d>, mut input: ResMut<Input>) {
    let camera = cameras.iter().next().copied().unwrap_or_default();
    let size = screen.size;
    input.map_world(|s| camera.screen_to_world(size, s));
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    globals_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    textures: Vec<wgpu::BindGroup>,
    instances: wgpu::Buffer,
    capacity: u64,
}

#[derive(Default)]
struct SpriteRenderer {
    gpu: Option<Gpu>,
    items: Vec<Item>,
    instances: Vec<Instance>,
    batches: Vec<Batch>,
}

impl Renderer for SpriteRenderer {
    fn render(&mut self, world: &mut World, gfx: &Gfx, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let gpu = self.gpu.get_or_insert_with(|| Gpu::new(gfx));
        let mut sprites = world.query::<(&Sprite, &Transform2d)>();
        let camera = world.query::<&Camera2d>().iter(world).next().copied().unwrap_or_default();
        let assets = world.resource::<Assets>();
        gpu.upload_textures(gfx, assets);

        self.items.clear();
        for (sprite, transform) in sprites.iter(world) {
            let region = assets.region(sprite.image);
            let size = sprite.size.unwrap_or(region.size) * transform.scale;
            self.items.push(Item {
                texture: region.texture,
                instance: Instance {
                    pos: transform.translation.into(),
                    size: size.into(),
                    rotation_z: [transform.rotation, transform.z],
                    uv: region.uv.into(),
                    color: to_rgba8(sprite.color),
                    _pad: 0,
                },
            });
        }
        batch(&mut self.items, &mut self.instances, &mut self.batches);

        let view_proj = camera.view_proj(Vec2::from(gfx.viewport.size));
        gfx.queue.write_buffer(&gpu.globals, 0, bytemuck::bytes_of(&view_proj.to_cols_array()));
        gpu.write_instances(gfx, &self.instances);

        let clear = world.resource::<ClearColor>().0;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sprites"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        if self.batches.is_empty() {
            return;
        }
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, &gpu.globals_group, &[]);
        pass.set_vertex_buffer(0, gpu.instances.slice(..));
        for b in &self.batches {
            pass.set_bind_group(1, &gpu.textures[b.texture.0 as usize], &[]);
            pass.draw(0..6, b.range.clone());
        }
    }
}

fn to_rgba8(color: Vec4) -> [u8; 4] {
    (color.clamp(Vec4::ZERO, Vec4::ONE) * 255.0 + 0.5).to_array().map(|c| c as u8)
}

impl Gpu {
    fn new(gfx: &Gfx) -> Self {
        let device = &gfx.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprites"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4, 4 => Unorm8x4
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprites"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                })],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gfx.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sprite globals"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite globals"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprites"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let instances = instance_buffer(device, INITIAL_CAPACITY);
        Self { pipeline, globals, globals_group, sampler, textures: Vec::new(), instances, capacity: INITIAL_CAPACITY }
    }

    /// 上传还没上传过的纹理（纹理只增不减）。
    fn upload_textures(&mut self, gfx: &Gfx, assets: &Assets) {
        // Assets 被整个替换成更少的图片时不越界（已上传的不会重新上传）
        for image in assets.images().get(self.textures.len()..).unwrap_or_default() {
            let size = wgpu::Extent3d { width: image.size.x, height: image.size.y, depth_or_array_layers: 1 };
            let texture = gfx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sprite texture"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            gfx.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &image.rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * image.size.x), rows_per_image: Some(image.size.y) },
                size,
            );
            let view = texture.create_view(&Default::default());
            self.textures.push(gfx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sprite texture"),
                layout: &self.pipeline.get_bind_group_layout(1),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            }));
        }
    }

    /// 写实例数据；容量不够时按两倍扩容。
    fn write_instances(&mut self, gfx: &Gfx, instances: &[Instance]) {
        let needed = instances.len() as u64;
        if needed > self.capacity {
            self.capacity = needed.next_power_of_two();
            self.instances = instance_buffer(&gfx.device, self.capacity);
        }
        if !instances.is_empty() {
            gfx.queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(instances));
        }
    }
}

fn instance_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sprite instances"),
        size: capacity * size_of::<Instance>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

// 让 TextureId 在批次里按上传顺序索引
const _: () = assert!(TextureId::WHITE.0 == 0);
