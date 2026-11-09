//! 精灵渲染器：每帧从 World 提取精灵 → 合批 → 一个实例缓冲 + 每批一次 draw。

use std::cell::{OnceCell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use bevy_ecs::prelude::*;
use glam::{Vec2, Vec4};
use nge_core::{Gfx, wgpu};
use nge_ecs::{App, ClearColor, Input, Interleave, Plugin, PreUpdate, RenderFrame, Renderer, Screen, Time};

use nge_assets::{Assets, GpuImages, Image};

use crate::assets::{Atlases, TextureId};
use crate::batch::{Batch, Instance, Item, LIT, batch};
use crate::camera::{Camera2d, DrawAt, Parallax, Transform2d, drawn};
use crate::effects::{Globals, Lighting2d, Masking, PostEffects, Shaders, SpriteShader, Unlit};
use crate::pipes::{Mode, Pipes, STENCIL_FORMAT, pipe};
use crate::post::PostRenderer;
use crate::repeat::{Repeat, copies};
use crate::sprite::{Sprite, SpriteInstances};

const INITIAL_CAPACITY: u64 = 1024;

/// 注册 [`Atlases`]、指针世界坐标换算和精灵渲染器（`ClearColor` 由 `App` 自带）。没有 `AssetPlugin` 时加一个默认的。
/// 精灵分两层画：z < 0 的背景排在所有渲染器前面（在 3D 后面），其余的排在已注册的渲染器后面（在 3D 上面），所以要在 `Render3dPlugin` 之后添加。
/// 开着 `Interleave` 时背景层不画，全部精灵在 3D 之后按深度和模型比（见 docs/3d-more.md）。
pub struct Render2dPlugin;

impl Plugin for Render2dPlugin {
    fn build(&self, app: &mut App) {
        nge_assets::ensure_plugin(app);
        app.init_resource::<Atlases>()
            .init_resource::<SpriteInstances>()
            .init_resource::<Shaders>()
            .init_resource::<Lighting2d>()
            .init_resource::<PostEffects>()
            .add_systems(PreUpdate, (pointer_world.in_set(PointerWorld), clear_sprite_instances))
            .set_post_process(PostRenderer::default());
        let (back, front) = sprite_layers();
        app.prepend_renderer(back).add_renderer(front);
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

/// 背景层和前景层的渲染器：共用一份管线，只预热一次。
fn sprite_layers() -> (SpriteRenderer, SpriteRenderer) {
    let pipes = SharedPipes::default();
    (SpriteRenderer::new(Layer::Back, pipes.clone()), SpriteRenderer::new(Layer::Front, pipes))
}

/// 两层精灵渲染器共用的管线，第一次画时建。
type SharedPipes = Rc<OnceCell<RefCell<Pipes>>>;

struct Gpu {
    pipes: SharedPipes,
    texture_layout: wgpu::BindGroupLayout,
    /// 遮罩用的模板缓冲，跟着渲染目标的大小走。
    stencil: Option<(wgpu::TextureView, wgpu::Extent3d)>,
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
    /// 和 3D 混排时背景层不画，前面那层画全部（深度交给 3D 的深度缓冲比）。
    fn contains(self, z: f32, interleave: bool) -> bool {
        if interleave {
            return self == Layer::Front;
        }
        (z < 0.0) == (self == Layer::Back)
    }
}

struct SpriteRenderer {
    layer: Layer,
    pipes: SharedPipes,
    gpu: Option<Gpu>,
    items: Vec<Item>,
    instances: Vec<Instance>,
    batches: Vec<Batch>,
    /// 重复的精灵这一帧画在哪些位置（复用）。
    copies: Vec<Vec2>,
    /// 份数太多时警告过的精灵（每个只警告一次）。
    warned: HashSet<Entity>,
    /// 提示过「这一层有只画在遮罩里 / 外的精灵、却没有遮罩」。
    warned_unmasked: bool,
}

impl SpriteRenderer {
    fn new(layer: Layer, pipes: SharedPipes) -> Self {
        Self { layer, pipes, gpu: None, items: Vec::new(), instances: Vec::new(), batches: Vec::new(), copies: Vec::new(), warned: HashSet::new(), warned_unmasked: false }
    }
}

/// 转了 `rotation`（弧度）的 `size` 矩形的外接框的一半；`size` 可以是负的（镜像）。
fn rotated_half(size: Vec2, rotation: f32) -> Vec2 {
    let (s, c) = rotation.sin_cos();
    Vec2::new(size.x.abs() * c.abs() + size.y.abs() * s.abs(), size.x.abs() * s.abs() + size.y.abs() * c.abs()) / 2.0
}

impl Renderer for SpriteRenderer {
    fn render(&mut self, world: &mut World, gfx: &Gfx, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let gpu = self.gpu.get_or_insert_with(|| Gpu::new(gfx, &self.pipes));
        let shared = gpu.pipes.clone();
        let mut pipes = shared.get().expect("Gpu::new 建好了").borrow_mut();
        let mut sprites = world.query::<(Entity, &Sprite, &Transform2d, Option<&DrawAt>, Option<&Parallax>, Option<&Repeat>, Option<&SpriteShader>, Option<&Masking>, Has<Unlit>)>();
        let camera = world.query::<&Camera2d>().iter(world).next().copied().unwrap_or_default();
        world.resource_scope(|world, mut images: Mut<GpuImages>| images.sync(gfx, world.resource::<Assets<Image>>()));
        let (images, atlases, gpu_images) = (world.resource::<Assets<Image>>(), world.resource::<Atlases>(), world.resource::<GpuImages>());
        let extra = world.resource::<SpriteInstances>();
        let interleave = world.resource::<Interleave>().0;
        // 3D 留下了深度（混排、前面有 3D 渲染器）时精灵按深度和模型比
        let scene_depth = world.resource::<RenderFrame>().depth().cloned();
        let depth = scene_depth.is_some();
        let shaders = world.resource::<Shaders>();
        pipes.sync(gfx, shaders, depth);

        self.items.clear();
        let view = camera.visible(Vec2::from(gfx.viewport.size));
        for (entity, sprite, transform, draw, parallax, repeat, shader, masking, unlit) in sprites.iter(world).filter(|(_, _, t, ..)| self.layer.contains(t.z, interleave)) {
            let Transform2d { translation: pos, rotation, .. } = drawn(transform, draw, parallax, &camera);
            let region = atlases.region(images, sprite.image);
            let size = sprite.size.unwrap_or(region.size) * transform.scale;
            self.copies.clear();
            // 没写大小、贴图还在加载时大小是 1×1，先不铺
            let sized = sprite.size.is_some() || images.size(region.texture).is_some();
            match repeat {
                Some(&repeat) if sized => {
                    if let Err(n) = copies(pos, size.abs(), rotated_half(size, rotation), repeat, view, &mut self.copies)
                        && self.warned.insert(entity)
                    {
                        log::warn!("重复的精灵一帧要画 {n} 份（步长 {}，视野 {} × {}），超过上限：只画原来那一份", size.abs(), view.1.x - view.0.x, view.1.y - view.0.y);
                    }
                }
                _ => self.copies.push(pos),
            }
            let mode = Mode::of(masking);
            let module = shader.map_or(0, |s| pipes.module(gfx, shaders, &s.name));
            let params = shader.map_or([0.0; 4], |s| s.params.into());
            for &pos in &self.copies {
                self.items.push(Item {
                    texture: region.texture,
                    pipe: pipe(module, mode, depth),
                    mask: mode == Mode::Write,
                    instance: Instance {
                        pos: pos.into(),
                        size: size.into(),
                        rotation_z: [rotation, transform.z],
                        uv: region.uv.into(),
                        color: to_rgba8(sprite.color),
                        flags: if unlit { 0 } else { LIT },
                        params,
                    },
                });
            }
        }
        // 同一张图的额外实例通常是连着放的（一个发射器一批），只在图变了时重新查区域
        let mut last = None;
        for &(image, sprite) in extra.items.iter().filter(|(_, s)| self.layer.contains(s.z, interleave)) {
            let region = match last {
                Some((i, region)) if i == image => region,
                _ => atlases.region(images, image),
            };
            last = Some((image, region));
            // 粒子、文字、调试线不受光
            self.items.push(Item {
                texture: region.texture,
                pipe: pipe(0, Mode::Normal, depth),
                mask: false,
                instance: Instance {
                    pos: sprite.position.into(),
                    size: sprite.size.into(),
                    rotation_z: [sprite.rotation, sprite.z],
                    uv: region.uv.into(),
                    color: to_rgba8(sprite.color),
                    flags: 0,
                    params: [0.0; 4],
                },
            });
        }
        let masked = self.items.iter().any(|i| i.pipe % 4 >= Mode::Inside as u32);
        if masked && !self.items.iter().any(|i| i.mask) && !std::mem::replace(&mut self.warned_unmasked, true) {
            log::warn!("z {} 0 的精灵里有只画在遮罩里 / 外的，这一层却没有遮罩（遮罩只管同一层：z < 0、z ≥ 0 各一层；和 3D 混排时只有一层）", if self.layer == Layer::Back { "<" } else { "≥" });
        }
        // 没有背景精灵时不开 pass，清屏留给后面的渲染器
        if self.layer == Layer::Back && self.items.is_empty() {
            return;
        }
        batch(&mut self.items, &mut self.instances, &mut self.batches);
        for id in std::iter::once(TextureId::WHITE).chain(self.batches.iter().map(|b| b.texture)) {
            gpu.cache_texture(gfx, gpu_images, id);
        }
        for b in &self.batches {
            pipes.prepare(gfx, b.pipe);
        }

        let view_proj = camera.view_proj(Vec2::from(gfx.viewport.size));
        let time = world.resource::<Time>().elapsed as f32;
        let globals = Globals::new(view_proj.to_cols_array(), scene_depth.as_ref().map(|d| d.view_proj), time, world.resource::<Lighting2d>());
        gfx.queue.write_buffer(&gpu.globals, 0, bytemuck::bytes_of(&globals));
        gpu.write_instances(gfx, &self.instances);
        // 混排时用 3D 的深度（读深度、清模板），否则用自己的模板缓冲
        let (stencil, depth_ops) = match &scene_depth {
            Some(d) => (d.view.clone(), Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Discard })),
            None => (gpu.stencil_view(gfx, target.texture().size()), None),
        };

        let clear = *world.resource::<ClearColor>();
        let mut frame = world.resource_mut::<RenderFrame>();
        frame.count(self.batches.len() as u32, self.instances.len() as u32);
        let load = frame.color_load(clear);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sprites"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &stencil,
                depth_ops,
                stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
            }),
            ..Default::default()
        });
        if self.batches.is_empty() {
            return;
        }
        pass.set_stencil_reference(1);
        pass.set_bind_group(0, &gpu.globals_group, &[]);
        pass.set_vertex_buffer(0, gpu.instances.slice(..));
        let mut current = None;
        for b in &self.batches {
            if current != Some(b.pipe) {
                pass.set_pipeline(pipes.get(b.pipe));
                current = Some(b.pipe);
            }
            pass.set_bind_group(1, gpu.texture(b.texture), &[]);
            pass.draw(0..6, b.range.clone());
        }
    }
}

fn to_rgba8(color: Vec4) -> [u8; 4] {
    (color.clamp(Vec4::ZERO, Vec4::ONE) * 255.0 + 0.5).to_array().map(|c| c as u8)
}

impl Gpu {
    fn new(gfx: &Gfx, shared: &SharedPipes) -> Self {
        let device = &gfx.device;
        let pipes = shared.get_or_init(|| RefCell::new(Pipes::new(gfx, size_of::<Globals>() as u64))).borrow();
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sprite globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite globals"),
            layout: &pipes.globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprites"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let instances = instance_buffer(device, INITIAL_CAPACITY);
        let texture_layout = pipes.texture_layout.clone();
        drop(pipes);
        Self { pipes: shared.clone(), texture_layout, stencil: None, globals, globals_group, sampler, textures: Vec::new(), instances, capacity: INITIAL_CAPACITY }
    }

    /// 模板缓冲跟着渲染目标的大小走（返回的 view 是克隆，便宜）。
    fn stencil_view(&mut self, gfx: &Gfx, size: wgpu::Extent3d) -> wgpu::TextureView {
        let size = wgpu::Extent3d { depth_or_array_layers: 1, ..size };
        if self.stencil.as_ref().is_none_or(|(_, s)| *s != size) {
            let texture = gfx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sprite stencil"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: STENCIL_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.stencil = Some((texture.create_view(&Default::default()), size));
        }
        self.stencil.as_ref().expect("刚创建").0.clone()
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
            layout: &self.texture_layout,
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


#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::{Layer, rotated_half, sprite_layers};

    #[test]
    fn both_sprite_layers_share_one_set_of_pipelines() {
        let (back, front) = sprite_layers();
        assert!(back.layer == Layer::Back && front.layer == Layer::Front);
        assert!(std::rc::Rc::ptr_eq(&back.pipes, &front.pipes));
    }

    #[test]
    fn rotated_half_is_half_the_box_around_the_turned_sprite() {
        let close = |a: Vec2, b: Vec2| assert!((a - b).length() < 1e-4, "{a} {b}");
        close(rotated_half(Vec2::new(40.0, 20.0), 0.0), Vec2::new(20.0, 10.0));
        close(rotated_half(Vec2::new(-40.0, 20.0), std::f32::consts::FRAC_PI_2), Vec2::new(10.0, 20.0));
        let d = 10.0 * std::f32::consts::SQRT_2;
        close(rotated_half(Vec2::splat(20.0), std::f32::consts::FRAC_PI_4), Vec2::splat(d));
    }
}
