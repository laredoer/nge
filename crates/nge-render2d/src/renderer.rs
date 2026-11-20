//! 精灵渲染器：每帧从 World 提取精灵 → 合批 → 一个实例缓冲 + 每批一次 draw。

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use bevy_ecs::prelude::*;
use glam::{Vec2, Vec4};
use nge_core::{Gfx, wgpu};
use nge_ecs::{App, ClearColor, Input, Interleave, Plugin, PreUpdate, RenderFrame, Renderer, Screen, Time};

use nge_assets::{Assets, GpuImages, Image};

use crate::assets::{Atlases, TextureId};
use crate::batch::{BUMPY, Batch, Instance, Item, LIT, NO_SHADOW, batch};
use crate::camera::{Camera2d, DrawAt, Parallax, Transform2d, drawn};
use crate::effects::{Globals, Lighting2d, Masking, PostEffect, PostEffects, PostLayer, Shaders, SpriteShader, Unlit};
use crate::occluders::{NormalMap, Occluder2d, outline, pack_edges, rect};
use crate::pipes::{Mode, Pipes, STENCIL_FORMAT, pipe};
use crate::post::{PostChain, PostRenderer};
use crate::repeat::{Repeat, copies};
use crate::sprite::{Sprite, SpriteInstances};
use crate::tiles::{TileLayer, View2d, frame_image};

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
            .add_systems(nge_ecs::PostUpdate, crate::occluders::mark_normal_maps)
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
    /// 按（贴图，法线图）缓存；贴图还没加载完的不缓存，先用白色贴图的。贴图重传过（代数变了）就清掉。
    textures: HashMap<(TextureId, TextureId), wgpu::BindGroup>,
    generation: u64,
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
    /// 和 3D 混排时背景层不画，前面那层画全部（深度交给 3D 的深度缓冲比）；
    /// 但有背景层的后处理时（`back_post`）照样分两层：z < 0 的在 3D 之前画、总在 3D 后面。
    fn contains(self, z: f32, interleave: bool, back_post: bool) -> bool {
        if interleave && !back_post {
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
    /// 这一帧挡光的边（世界坐标）和打进 uniform 的样子（复用）。
    edges: Vec<(Vec2, Vec2)>,
    packed_edges: Vec<[f32; 4]>,
    /// 提示过挡光的边太多。
    warned_edges: bool,
    /// 这一帧瓦片层拷出来的实例（复用）。
    tiles: Vec<Instance>,
    /// 背景层自己的后处理和画到的离屏纹理。
    post: PostChain,
    post_target: Option<wgpu::TextureView>,
    /// 提示过背景层的后处理做不了。
    warned_post: bool,
}

impl SpriteRenderer {
    fn new(layer: Layer, pipes: SharedPipes) -> Self {
        Self { layer, pipes, gpu: None, items: Vec::new(), instances: Vec::new(), batches: Vec::new(), copies: Vec::new(), tiles: Vec::new(), warned: HashSet::new(), warned_unmasked: false, edges: Vec::new(), packed_edges: Vec::new(), warned_edges: false, post: PostChain::default(), post_target: None, warned_post: false }
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
        let mut sprites = world.query::<(
            Entity,
            &Sprite,
            &Transform2d,
            Option<&DrawAt>,
            Option<&Parallax>,
            Option<&Repeat>,
            Option<&SpriteShader>,
            Option<&Masking>,
            Has<Unlit>,
            Option<&NormalMap>,
            Has<Occluder2d>,
        )>();
        let mut occluders = world.query::<(&Occluder2d, &Transform2d, Option<&Sprite>, Option<&DrawAt>, Option<&Parallax>)>();
        let camera = world.query::<&Camera2d>().iter(world).next().copied().unwrap_or_default();
        world.resource_scope(|world, mut images: Mut<GpuImages>| images.sync(gfx, world.resource::<Assets<Image>>()));
        let view2d = View2d::new(camera, Vec2::from(gfx.viewport.size));
        let (interleave, has_back_post) = (world.resource::<Interleave>().0, world.resource::<PostEffects>().layer(PostLayer::Back).next().is_some());
        let tile_draws = draw_tiles(self.layer, &mut self.tiles, world, &view2d, interleave, has_back_post);
        let (images, atlases, gpu_images) = (world.resource::<Assets<Image>>(), world.resource::<Atlases>(), world.resource::<GpuImages>());
        let extra = world.resource::<SpriteInstances>();
        // 3D 留下了深度（混排、前面有 3D 渲染器）时精灵按深度和模型比
        let scene_depth = world.resource::<RenderFrame>().depth().cloned();
        let depth = scene_depth.is_some();
        let shaders = world.resource::<Shaders>();
        pipes.sync(gfx, shaders, depth);
        // 背景层自己那串后处理也每帧预热（用不用都热），切到有背景层后处理的关卡时不卡
        if self.layer == Layer::Back {
            self.post.warm(gfx, shaders);
        }

        self.items.clear();
        let view = camera.visible(Vec2::from(gfx.viewport.size));
        let lighting = world.resource::<Lighting2d>();
        let lit_scene = lighting.enabled;
        let mut sprite_count = [0u32; 2];
        for (entity, sprite, transform, draw, parallax, repeat, shader, masking, unlit, normal, occluder) in
            sprites.iter(world).filter(|(_, _, t, ..)| self.layer.contains(t.z, interleave, has_back_post))
        {
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
            // 视野外的不画；自定义着色器可能挪顶点，不裁
            if shader.is_none() {
                let half = rotated_half(size, rotation);
                self.copies.retain(|&p| view2d.overlaps(p - half, p + half, 0.0));
            }
            sprite_count[1] += 1;
            sprite_count[0] += u32::from(!self.copies.is_empty());
            let mode = Mode::of(masking);
            let module = shader.map_or(0, |s| pipes.module(gfx, shaders, &s.name));
            let params = shader.map_or([0.0; 4], |s| s.params.into());
            // 法线图加载好了才用，不然当没有（不会暗一帧）
            let normal = normal.map(|n| n.0).filter(|&n| !unlit && lit_scene && gpu_images.view(n).is_some());
            let flags = match (unlit, normal.is_some(), occluder) {
                (true, ..) => 0,
                (false, bumpy, occluder) => LIT | if bumpy { BUMPY } else { 0 } | if occluder { NO_SHADOW } else { 0 },
            };
            for &pos in &self.copies {
                self.items.push(Item {
                    texture: region.texture,
                    normal: normal.unwrap_or(TextureId::WHITE),
                    pipe: pipe(module, mode, depth),
                    mask: mode == Mode::Write,
                    tile: false,
                    instance: Instance {
                        pos: pos.into(),
                        size: size.into(),
                        rotation_z: [rotation, transform.z],
                        uv: region.uv.into(),
                        color: to_rgba8(sprite.color),
                        flags,
                        params,
                    },
                });
            }
        }
        for &(texture, ref range) in &tile_draws {
            for &instance in &self.tiles[range.clone()] {
                self.items.push(Item { texture, normal: TextureId::WHITE, pipe: pipe(0, Mode::Normal, depth), mask: false, tile: true, instance });
            }
        }
        // 同一张图的额外实例通常是连着放的（一个发射器一批），只在图变了时重新查区域
        let mut last = None;
        for &(image, sprite) in extra.items.iter().filter(|(_, s)| self.layer.contains(s.z, interleave, has_back_post)) {
            let region = match last {
                Some((i, region)) if i == image => region,
                _ => atlases.region(images, image),
            };
            last = Some((image, region));
            // 粒子、文字、调试线不受光
            self.items.push(Item {
                texture: region.texture,
                normal: TextureId::WHITE,
                pipe: pipe(0, Mode::Normal, depth),
                mask: false,
                tile: false,
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
        if gpu.generation != gpu_images.generation() {
            gpu.generation = gpu_images.generation();
            gpu.textures.clear();
        }
        for (color, normal) in std::iter::once((TextureId::WHITE, TextureId::WHITE)).chain(self.batches.iter().map(|b| (b.texture, b.normal))) {
            gpu.cache_texture(gfx, gpu_images, color, normal);
        }
        // 挡光的边：只在有灯开了 shadows 时收
        self.edges.clear();
        if lighting.enabled && lighting.lights.iter().any(|l| l.shadows) {
            for (occluder, transform, sprite, draw, parallax) in occluders.iter(world) {
                let t = drawn(transform, draw, parallax, &camera);
                if occluder.points.is_empty() {
                    let Some(sprite) = sprite else { continue };
                    let region = atlases.region(images, sprite.image);
                    // 没写大小、贴图还在加载时不知道多大，先不挡
                    if sprite.size.is_none() && images.size(region.texture).is_none() {
                        continue;
                    }
                    let size = sprite.size.unwrap_or(region.size);
                    outline(&rect(size), t.translation, t.rotation, t.scale, &mut self.edges);
                } else {
                    outline(&occluder.points, t.translation, t.rotation, t.scale, &mut self.edges);
                }
            }
        }
        let dropped = pack_edges(&self.edges, &lighting.lights, &mut self.packed_edges);
        if dropped > 0 && !std::mem::replace(&mut self.warned_edges, true) {
            log::warn!("挡光的边超过 {} 条，多出的 {dropped} 条不挡光", crate::MAX_OCCLUDER_EDGES);
        }
        for b in &self.batches {
            pipes.prepare(gfx, b.pipe);
        }

        let view_proj = camera.view_proj(Vec2::from(gfx.viewport.size));
        let time = world.resource::<Time>().elapsed as f32;
        let globals = Globals::new(view_proj.to_cols_array(), scene_depth.as_ref().map(|d| d.view_proj), time, lighting, &self.packed_edges);
        gfx.queue.write_buffer(&gpu.globals, 0, bytemuck::bytes_of(&globals));
        gpu.write_instances(gfx, &self.instances);
        // 混排时用 3D 的深度（读深度、清模板），否则用自己的模板缓冲
        let (stencil, depth_ops) = match &scene_depth {
            Some(d) => (d.view.clone(), Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Discard })),
            None => (gpu.stencil_view(gfx, target.texture().size()), None),
        };

        let clear = *world.resource::<ClearColor>();
        let back_post: Vec<PostEffect> = match self.layer {
            Layer::Back if has_back_post => world.resource::<PostEffects>().layer(PostLayer::Back).cloned().collect(),
            Layer::Back => Vec::new(),
            Layer::Front => Vec::new(),
        };
        let mut frame = world.resource_mut::<RenderFrame>();
        frame.count(self.batches.len() as u32, self.instances.len() as u32);
        let cull = frame.cull_mut();
        cull.sprites[0] += sprite_count[0];
        cull.sprites[1] += sprite_count[1];
        let load = frame.color_load(clear);
        // 背景层的后处理：先画到离屏纹理上，过完一串再画到目标上。前面已经有别的渲染器画过时不做（会盖掉它们）
        let offscreen = match (back_post.is_empty(), load) {
            (true, _) => None,
            (false, wgpu::LoadOp::Clear(_)) => Some(post_target(&mut self.post_target, gfx, target)),
            (false, _) => {
                warn_once(&mut self.warned_post, "背景层的后处理要背景层第一个画：前面已经有渲染器画过，跳过");
                None
            }
        };
        let draw_to = offscreen.clone().unwrap_or_else(|| target.clone());
        let timer = nge_ecs::pass_timer(world, "精灵");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sprites"),
            timestamp_writes: timer.as_ref().and_then(nge_ecs::PassTimer::writes),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &draw_to,
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
        if !self.batches.is_empty() {
            pass.set_stencil_reference(1);
            pass.set_bind_group(0, &gpu.globals_group, &[]);
            pass.set_vertex_buffer(0, gpu.instances.slice(..));
            let mut current = None;
            for b in &self.batches {
                if current != Some(b.pipe) {
                    pass.set_pipeline(pipes.get(b.pipe));
                    current = Some(b.pipe);
                }
                pass.set_bind_group(1, gpu.texture(b.texture, b.normal), &[]);
                pass.draw(0..6, b.range.clone());
            }
        }
        drop(pass);
        if let Some(source) = offscreen {
            let time = world.resource::<Time>().elapsed as f32;
            let timers = crate::post::post_timers(world, back_post.len());
            let count = self.post.run(gfx, world.resource::<Shaders>(), encoder, &back_post, &source, target, time, &timers);
            world.resource_mut::<RenderFrame>().count(count, 0);
        }
    }
}

/// 这一层的瓦片地图按视野拷出实例到 `out`，返回每张地图用的贴图和实例范围；裁剪的统计记进 `RenderFrame`。
fn draw_tiles(layer: Layer, out: &mut Vec<Instance>, world: &mut World, view: &View2d, interleave: bool, back_post: bool) -> Vec<(TextureId, std::ops::Range<usize>)> {
    out.clear();
    let elapsed = world.resource::<Time>().elapsed;
    let mut draws = Vec::new();
    let (mut chunks, mut tiles) = ([0u32; 2], 0u32);
    world.resource_scope(|world, atlases: Mut<Atlases>| {
        world.resource_scope(|world, images: Mut<Assets<Image>>| {
            let mut layers = world.query::<&mut TileLayer>();
            for mut map in layers.iter_mut(world).filter(|m| layer.contains(m.z, interleave, back_post)) {
                let image = map.image_for(0);
                let region = |f: u32| {
                    let r = atlases.region(&images, frame_image(image, f));
                    (r.texture, r.uv)
                };
                let start = out.len();
                let parallax = map.parallax;
                let (texture, stats) = map.draw(view, parallax, elapsed, &region, out);
                chunks[0] += stats.shown;
                chunks[1] += stats.total;
                tiles += stats.tiles;
                draws.push((texture, start..out.len()));
            }
        });
    });
    let mut frame = world.resource_mut::<RenderFrame>();
    let cull = frame.cull_mut();
    cull.chunks[0] += chunks[0];
    cull.chunks[1] += chunks[1];
    cull.tiles += tiles;
    draws
}

/// 背景层画到的离屏纹理，跟着目标的大小、格式走。
fn post_target(slot: &mut Option<wgpu::TextureView>, gfx: &Gfx, target: &wgpu::TextureView) -> wgpu::TextureView {
    let texture = target.texture();
    let (size, format) = (wgpu::Extent3d { depth_or_array_layers: 1, ..texture.size() }, gfx.target_format(target));
    if slot.as_ref().is_none_or(|v| v.texture().size() != size || v.texture().format() != format) {
        let view = gfx
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("back layer"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        *slot = Some(view);
    }
    slot.clone().expect("刚建好")
}

fn warn_once(warned: &mut bool, message: &str) {
    if !std::mem::replace(warned, true) {
        log::warn!("{message}");
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
        Self { pipes: shared.clone(), texture_layout, stencil: None, globals, globals_group, sampler, textures: HashMap::new(), generation: 0, instances, capacity: INITIAL_CAPACITY }
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

    /// 贴图和法线图都加载好了就建 bind group 缓存起来；没加载好的什么也不做（画的时候用白色）。
    fn cache_texture(&mut self, gfx: &Gfx, images: &GpuImages, color: TextureId, normal: TextureId) {
        if self.textures.contains_key(&(color, normal)) {
            return;
        }
        // 法线图按线性数据读
        let (Some(view), Some(normal_view)) = (images.view(color), images.linear_view(normal)) else { return };
        let group = gfx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite texture"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(normal_view) },
            ],
        });
        self.textures.insert((color, normal), group);
    }

    fn texture(&self, color: TextureId, normal: TextureId) -> &wgpu::BindGroup {
        self.textures.get(&(color, normal)).or(self.textures.get(&(TextureId::WHITE, TextureId::WHITE))).expect("白色贴图总是先上传")
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
    fn layers_split_by_z_unless_interleaved_without_back_post() {
        let (back, front) = (Layer::Back, Layer::Front);
        assert!(back.contains(-1.0, false, false) && !front.contains(-1.0, false, false));
        assert!(front.contains(0.0, false, false) && !back.contains(0.0, false, false));
        assert!(!back.contains(-1.0, true, false) && front.contains(-1.0, true, false), "混排时前面那层画全部");
        assert!(back.contains(-1.0, true, true) && !front.contains(-1.0, true, true), "有背景层后处理时照样分两层");
        assert!(front.contains(2.0, true, true) && !back.contains(2.0, true, true));
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
