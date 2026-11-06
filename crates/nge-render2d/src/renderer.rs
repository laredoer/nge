//! 精灵渲染器：每帧从 World 提取精灵 → 合批 → 一个实例缓冲 + 每批一次 draw。

use bevy_ecs::prelude::*;
use glam::{Vec2, Vec4};
use nge_core::{Gfx, wgpu};
use nge_ecs::{App, ClearColor, Input, Plugin, PreUpdate, RenderFrame, Renderer, Screen};

use nge_assets::{Assets, GpuImages, Image};

use crate::assets::{Atlases, TextureId};
use crate::batch::{Batch, Instance, Item, batch};
use crate::camera::{Camera2d, DrawAt, Parallax, Transform2d, drawn};
use crate::sprite::{Sprite, SpriteInstances};

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

/// 注册 [`Atlases`]、指针世界坐标换算和精灵渲染器（`ClearColor` 由 `App` 自带）。没有 `AssetPlugin` 时加一个默认的。
/// 精灵分两层画：z < 0 的背景排在所有渲染器前面（在 3D 后面），其余的排在已注册的渲染器后面（在 3D 上面），所以要在 `Render3dPlugin` 之后添加。
pub struct Render2dPlugin;

impl Plugin for Render2dPlugin {
    fn build(&self, app: &mut App) {
        nge_assets::ensure_plugin(app);
        app.init_resource::<Atlases>()
            .init_resource::<SpriteInstances>()
            .add_systems(PreUpdate, (pointer_world.in_set(PointerWorld), clear_sprite_instances))
            .prepend_renderer(SpriteRenderer::new(Layer::Back))
            .add_renderer(SpriteRenderer::new(Layer::Front));
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

fn clear_sprite_instances(mut instances: ResMut<SpriteInstances>) {
    instances.clear();
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    globals_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    /// 按贴图编号缓存；贴图还没加载完的不缓存，先用白色贴图的。
    textures: Vec<Option<wgpu::BindGroup>>,
    instances: wgpu::Buffer,
    capacity: u64,
}

/// 精灵层：z < 0 是背景。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layer {
    Back,
    Front,
}

impl Layer {
    fn contains(self, z: f32) -> bool {
        (z < 0.0) == (self == Layer::Back)
    }
}

struct SpriteRenderer {
    layer: Layer,
    gpu: Option<Gpu>,
    items: Vec<Item>,
    instances: Vec<Instance>,
    batches: Vec<Batch>,
}

impl SpriteRenderer {
    fn new(layer: Layer) -> Self {
        Self { layer, gpu: None, items: Vec::new(), instances: Vec::new(), batches: Vec::new() }
    }
}

impl Renderer for SpriteRenderer {
    fn render(&mut self, world: &mut World, gfx: &Gfx, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let gpu = self.gpu.get_or_insert_with(|| Gpu::new(gfx));
        let mut sprites = world.query::<(&Sprite, &Transform2d, Option<&DrawAt>, Option<&Parallax>)>();
        let camera = world.query::<&Camera2d>().iter(world).next().copied().unwrap_or_default();
        world.resource_scope(|world, mut images: Mut<GpuImages>| images.sync(gfx, world.resource::<Assets<Image>>()));
        let (images, atlases, gpu_images) = (world.resource::<Assets<Image>>(), world.resource::<Atlases>(), world.resource::<GpuImages>());
        let extra = world.resource::<SpriteInstances>();

        self.items.clear();
        for (sprite, transform, draw, parallax) in sprites.iter(world).filter(|(_, t, ..)| self.layer.contains(t.z)) {
            let Transform2d { translation: pos, rotation, .. } = drawn(transform, draw, parallax, &camera);
            let region = atlases.region(images, sprite.image);
            let size = sprite.size.unwrap_or(region.size) * transform.scale;
            self.items.push(Item {
                texture: region.texture,
                instance: Instance {
                    pos: pos.into(),
                    size: size.into(),
                    rotation_z: [rotation, transform.z],
                    uv: region.uv.into(),
                    color: to_rgba8(sprite.color),
                    _pad: 0,
                },
            });
        }
        // 同一张图的额外实例通常是连着放的（一个发射器一批），只在图变了时重新查区域
        let mut last = None;
        for &(image, sprite) in extra.items.iter().filter(|(_, s)| self.layer.contains(s.z)) {
            let region = match last {
                Some((i, region)) if i == image => region,
                _ => atlases.region(images, image),
            };
            last = Some((image, region));
            self.items.push(Item {
                texture: region.texture,
                instance: Instance {
                    pos: sprite.position.into(),
                    size: sprite.size.into(),
                    rotation_z: [sprite.rotation, sprite.z],
                    uv: region.uv.into(),
                    color: to_rgba8(sprite.color),
                    _pad: 0,
                },
            });
        }
        // 没有背景精灵时不开 pass，清屏留给后面的渲染器
        if self.layer == Layer::Back && self.items.is_empty() {
            return;
        }
        batch(&mut self.items, &mut self.instances, &mut self.batches);
        for id in std::iter::once(TextureId::WHITE).chain(self.batches.iter().map(|b| b.texture)) {
            gpu.cache_texture(gfx, gpu_images, id);
        }

        let view_proj = camera.view_proj(Vec2::from(gfx.viewport.size));
        gfx.queue.write_buffer(&gpu.globals, 0, bytemuck::bytes_of(&view_proj.to_cols_array()));
        gpu.write_instances(gfx, &self.instances);

        let clear = *world.resource::<ClearColor>();
        let load = world.resource_mut::<RenderFrame>().color_load(clear);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sprites"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
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
            pass.set_bind_group(1, gpu.texture(b.texture), &[]);
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

    /// 贴图加载好了就建 bind group 缓存起来；没加载好的什么也不做（画的时候用白色）。
    fn cache_texture(&mut self, gfx: &Gfx, images: &GpuImages, id: TextureId) {
        let index = id.index() as usize;
        if self.textures.get(index).is_some_and(Option::is_some) {
            return;
        }
        let Some(view) = images.view(id) else { return };
        if self.textures.len() <= index {
            self.textures.resize_with(index + 1, || None);
        }
        self.textures[index] = Some(gfx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite texture"),
            layout: &self.pipeline.get_bind_group_layout(1),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        }));
    }

    fn texture(&self, id: TextureId) -> &wgpu::BindGroup {
        self.textures.get(id.index() as usize).and_then(Option::as_ref).or(self.textures[0].as_ref()).expect("白色贴图总是先上传")
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

