//! 网格渲染器：深度缓冲、每次 draw 一段动态 uniform、贴图按 ImageId 缓存 bind group。

use std::num::NonZeroU64;

use bevy_ecs::prelude::*;
use glam::{Mat4, Vec3};
use nge_core::{Gfx, wgpu};
use nge_ecs::{App, ClearColor, Interleave, Plugin, PostUpdate, PreUpdate, RenderFrame, Renderer, SCENE_DEPTH_FORMAT, SceneDepth};

use nge_assets::{AssetApp, Assets, GpuImages, Image, ImageId};

use crate::animator::animate;
use crate::billboard_gpu::BillboardGpu;
use crate::billboards::{Batch, BillboardVertex, Billboards, Scratch as BillboardScratch};
use crate::gizmos::Gizmos3d;
use crate::line_gpu::LineGpu;
use crate::assets::{Assets3d, MaterialId, MeshId, ModelId};
use crate::depth_target::DepthTarget;
use crate::collect::{Draw, DrawRaw, Scratch, blend_depth, collect, globals, sort_draws};
use crate::gltf_load::{Gltf, GltfModels, resolve_gltf_models};
use crate::light::{AmbientLight, Globals};
use crate::mesh::{Influence, Vertex};
use crate::morph::MorphBuffer;
use crate::fog::{FOG_WGSL, Fog};
use crate::shadow::{Caster, ShadowGpu, Shadows};
use crate::sky::Sky;
use crate::sky_gpu::SkyGpu;

const SHADER: &str = include_str!("shader.wgsl");
/// 和精灵共用（混排时精灵读它，遮罩写它的模板）。
const DEPTH_FORMAT: wgpu::TextureFormat = SCENE_DEPTH_FORMAT;
const INITIAL_DRAWS: u64 = 64;
const INITIAL_JOINTS: u64 = 64;

/// 一个网格加一个材质，画在同一实体的 [`Transform3d`] 上。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Mesh3d {
    pub mesh: MeshId,
    pub material: MaterialId,
}

/// 一个模型（通常来自 glTF），画在同一实体的 [`Transform3d`] 上。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Model3d(pub ModelId);

/// 注册 [`Assets3d`]、[`AmbientLight`] 和网格渲染器。要叠 2D 精灵时，在 `Render2dPlugin` 之前添加。
pub struct Render3dPlugin;

impl Plugin for Render3dPlugin {
    fn build(&self, app: &mut App) {
        nge_assets::ensure_plugin(app);
        app.init_asset::<Gltf>()
            .init_resource::<GltfModels>()
            .add_systems(PreUpdate, resolve_gltf_models.after(nge_assets::AssetsApplied))
            .add_systems(PostUpdate, animate)
            // 状态机（见 docs/anim-graph.md）
            .add_message::<nge_anim::AnimEvent>()
            .add_systems(PostUpdate, crate::graph_pose::animate_graphs.in_set(nge_anim::Animated))
            .init_resource::<crate::billboards::Billboards>()
            .add_systems(PreUpdate, crate::billboards::clear_billboards)
            // 调试画线（见 docs/gameplay3d.md）
            .init_resource::<Gizmos3d>()
            .add_systems(PreUpdate, crate::gizmos::clear_gizmos);
        // 一插上状态机就有 Pose：第一帧就按状态机的姿势画（见 docs/anim-graph.md）
        app.world_mut().register_required_components::<nge_anim::AnimGraph, crate::animator::Pose>();
        // 天空、雾（见 docs/render3d-quality.md）
        app.init_asset::<nge_assets::CubeImage>().init_resource::<Sky>().init_resource::<Fog>().init_resource::<crate::sky_gpu::SkyBake>();
        app.insert_resource(Assets3d::default()).init_resource::<AmbientLight>().init_resource::<Shadows>().add_renderer(MeshRenderer::default());
    }
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    /// 关节权重（第二个顶点缓冲），蒙皮的网格才有。
    skin: Option<wgpu::Buffer>,
    indices: wgpu::Buffer,
    count: u32,
    /// 本地坐标的包围盒（含每个变形目标加满的位置），拟合阴影图、给半透明排序用。
    bounds: (Vec3, Vec3),
    /// 填进 `DrawRaw::morph`。
    morph: [u32; 4],
}

struct Gpu {
    /// [背面剔除, 正面剔除（镜像变换，绕序反了）, 双面]，再按蒙皮、半透明各翻一倍：下标 = 剔除 + 3 × 蒙皮 + 6 × 半透明。
    pipelines: Vec<wgpu::RenderPipeline>,
    globals: wgpu::Buffer,
    globals_layout: wgpu::BindGroupLayout,
    globals_group: wgpu::BindGroup,
    shadow: ShadowGpu,
    sky: SkyGpu,
    draw_layout: wgpu::BindGroupLayout,
    draws: wgpu::Buffer,
    draws_group: wgpu::BindGroup,
    draw_capacity: u64,
    stride: u64,
    /// 这一帧所有蒙皮 draw 的关节矩阵（storage buffer，和 draw 的 uniform 在一个 bind group）。
    joints: wgpu::Buffer,
    joint_capacity: u64,
    /// 所有网格的变形差（和关节矩阵在同一个 bind group）。
    morphs: MorphBuffer,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// 按贴图编号缓存；贴图还没加载完的不缓存，先用白色贴图的。
    textures: Vec<Option<wgpu::BindGroup>>,
    generation: u64,
    meshes: Vec<GpuMesh>,
    depth: DepthTarget,
    billboards: BillboardGpu,
    lines: LineGpu,
}

#[derive(Default)]
struct MeshRenderer {
    gpu: Option<Gpu>,
    draws: Vec<Draw>,
    bytes: Vec<u8>,
    joints: Vec<Mat4>,
    warned_lights: bool,
    casters: Vec<Caster>,
    billboard_vertices: Vec<BillboardVertex>,
    billboard_batches: Vec<Batch>,
    billboard_scratch: BillboardScratch,
    /// 这一帧有没有要软化的方片。
    billboard_soft: bool,
    scratch: Scratch,
    /// 每个 draw 在不在相机视锥里（复用）。
    visible: Vec<bool>,
}

/// 包围盒和视锥相交的才画；包围盒算坏了的照画（不该让东西凭空消失）。
pub fn in_view(view_proj: Mat4, bounds: (Vec3, Vec3)) -> bool {
    !(bounds.0.is_finite() && bounds.1.is_finite()) || crate::shadow_views::intersects(view_proj, bounds)
}

impl Renderer for MeshRenderer {
    fn render(&mut self, world: &mut World, gfx: &Gfx, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let gpu = self.gpu.get_or_insert_with(|| Gpu::new(gfx));
        let (mut globals, view, camera) = globals(world, gfx.viewport.size.into(), &mut self.scratch);
        if globals.1 && !std::mem::replace(&mut self.warned_lights, true) {
            log::warn!("灯光数量超过上限，多出来的已忽略");
        }

        world.resource_scope(|world, mut images: Mut<GpuImages>| images.sync(gfx, world.resource::<Assets<Image>>()));
        gpu.upload(gfx, world.resource::<Assets3d>());
        collect(world, &mut self.draws, &mut self.joints, &mut self.scratch);
        self.draws.retain(|d| (d.mesh.0 as usize) < gpu.meshes.len());
        for d in &mut self.draws {
            d.raw.morph = gpu.meshes[d.mesh.0 as usize].morph;
        }
        let view_proj = Mat4::from_cols_array_2d(&globals.0.view_proj);
        for i in 0..self.draws.len() {
            if self.draws[i].blend {
                let b = gpu.world_bounds(&self.draws[i], &self.joints);
                self.draws[i].depth = blend_depth(&camera.eye, b);
            }
        }
        sort_draws(&mut self.draws);
        self.billboard_soft = crate::billboards::build(world.resource::<Billboards>(), view, &mut self.billboard_scratch, &mut self.billboard_vertices, &mut self.billboard_batches);
        let images = world.resource::<GpuImages>();
        // 贴图重传过：缓存的绑定组指着旧纹理
        if gpu.generation != images.generation() {
            gpu.generation = images.generation();
            gpu.textures.clear();
        }
        let textures = self.draws.iter().map(|d| d.texture).chain(self.billboard_batches.iter().map(|b| b.texture));
        for id in std::iter::once(ImageId::WHITE).chain(textures) {
            gpu.cache_texture(gfx, images, id);
        }
        gpu.lines.prepare(gfx, view_proj, world.resource::<Gizmos3d>());
        gpu.write_draws(gfx, &self.draws, &mut self.bytes, &self.joints);
        // 半透明的不投影子；一个算坏了的包围盒不该让整个场景的影子消失。视锥外的不画，影子照样投
        self.casters.clear();
        self.visible.clear();
        for (index, d) in self.draws.iter().enumerate() {
            let bounds = gpu.world_bounds(d, &self.joints);
            self.visible.push(in_view(view_proj, bounds));
            if !d.blend && bounds.0.is_finite() && bounds.1.is_finite() {
                self.casters.push(Caster { index, skinned: d.joints.is_some(), bounds });
            }
        }
        let mut rebuilt = gpu.shadow.prepare(gfx, &mut globals.0, &camera, &self.casters, &self.scratch.candidates, *world.resource::<Shadows>());
        rebuilt |= gpu.sky.prepare(gfx, world.resource::<Sky>(), world.resource::<Assets<Image>>(), world.resource::<Assets<nge_assets::CubeImage>>(), &mut globals.0);
        let bake = gpu.sky.status();
        if *world.resource::<crate::sky_gpu::SkyBake>() != bake {
            world.insert_resource(bake);
        }
        if rebuilt {
            gpu.globals_group = globals_group(&gfx.device, &gpu.globals_layout, &gpu.globals, &gpu.shadow, &gpu.sky);
        }
        let forward = camera.eye.rotation * Vec3::NEG_Z;
        globals.0.fog = world.resource::<Fog>().raw(camera.eye.translation, camera.camera.ortho_height().is_none(), forward, gpu.sky.has_sky());
        let billboard_view = crate::billboard_gpu::ViewRaw::new(&globals.0);
        gpu.billboards.prepare(gfx, &billboard_view, &self.billboard_vertices);
        gfx.queue.write_buffer(&gpu.globals, 0, bytemuck::bytes_of(&globals.0));
        let timers: Vec<Option<nge_ecs::PassTimer>> = (0..gpu.shadow.pass_count()).map(|_| nge_ecs::pass_timer(world, "阴影")).collect();
        let g = &*gpu;
        let shadow_draws = g.shadow.draw(encoder, &self.casters, &timers, |pass, i| {
            let draw = &self.draws[i];
            let mesh = &g.meshes[draw.mesh.0 as usize];
            pass.set_bind_group(1, &g.draws_group, &[(i as u64 * g.stride) as u32]);
            mesh.bind(pass, draw);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        });
        let n = self.visible.iter().filter(|&&v| v).count() as u32;
        let mut frame = world.resource_mut::<RenderFrame>();
        frame.count(n + shadow_draws, n);
        let cull = frame.cull_mut();
        cull.meshes[0] += n;
        cull.meshes[1] += self.draws.len() as u32;
        let billboards = world.resource::<Billboards>();
        let quads = (billboards.quads() + billboards.ribbons()) as u32;
        world.resource_mut::<RenderFrame>().count(self.billboard_batches.len() as u32, quads);
        let lines = world.resource::<Gizmos3d>();
        let segments = (lines.depth_tested().len() + lines.top().len()) as u32;
        world.resource_mut::<RenderFrame>().count(gpu.lines.draws(), segments);
        // 有软粒子时方片另开一个 pass，深度只读、同时绑成纹理（GL 上不行，退成硬边）
        let split = self.billboard_soft && !gpu.billboards.gl;
        if let Some(depth_only) = gpu.depth.prepare(&gfx.device, target.texture().size(), split) {
            gpu.billboards.set_depth(&gfx.device, &depth_only);
        }
        let depth = gpu.depth.view().clone();
        let depth = &depth;
        // 混排时深度留给后面的精灵
        let interleave = world.resource::<Interleave>().0;
        if interleave {
            let view_proj = bytemuck::cast(globals.0.view_proj);
            world.resource_mut::<RenderFrame>().set_depth(SceneDepth { view: depth.clone(), view_proj });
        }

        let clear = *world.resource::<ClearColor>();
        let load = world.resource_mut::<RenderFrame>().color_load(clear);
        let timer = nge_ecs::pass_timer(world, "网格");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("meshes"),
            timestamp_writes: timer.as_ref().and_then(nge_ecs::PassTimer::writes),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: if interleave || split { wgpu::StoreOp::Store } else { wgpu::StoreOp::Discard } }),
                stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
            }),
            ..Default::default()
        });
        pass.set_bind_group(0, &gpu.globals_group, &[]);
        // 不透明的、天空、半透明的：天空只盖住深度还是 1 的地方，半透明的叠在天空上
        let opaque = self.draws.iter().take_while(|d| !d.blend).count();
        for (i, draw) in self.draws.iter().enumerate() {
            if i == opaque {
                gpu.sky.draw(&mut pass, &globals.0);
            }
            if !self.visible[i] {
                continue;
            }
            let mesh = &gpu.meshes[draw.mesh.0 as usize];
            let texture = gpu.texture(draw.texture);
            pass.set_pipeline(&gpu.pipelines[draw.pipeline]);
            pass.set_bind_group(1, &gpu.draws_group, &[(i as u64 * gpu.stride) as u32]);
            pass.set_bind_group(2, texture, &[]);
            mesh.bind(&mut pass, draw);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        }
        if opaque == self.draws.len() {
            gpu.sky.draw(&mut pass, &globals.0);
        }
        if !split {
            // 透明的方片和带子画在所有网格后面
            gpu.billboards.draw(&mut pass, &self.billboard_batches, false, |id| gpu.texture(id));
            // 调试画线最后画：半透明的方片不会盖住它
            gpu.lines.draw(&mut pass);
            return;
        }
        drop(pass);
        let timer = nge_ecs::pass_timer(world, "软粒子");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("soft billboards"),
            timestamp_writes: timer.as_ref().and_then(nge_ecs::PassTimer::writes),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
            // 只读：同一张深度纹理同时绑在 billboards 的 bind group 里
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: depth, depth_ops: None, stencil_ops: None }),
            ..Default::default()
        });
        gpu.billboards.draw(&mut pass, &self.billboard_batches, true, |id| gpu.texture(id));
        drop(pass);
        if gpu.lines.draws() == 0 {
            return;
        }
        let timer = nge_ecs::pass_timer(world, "调试线");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("lines"),
            timestamp_writes: timer.as_ref().and_then(nge_ecs::PassTimer::writes),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: if interleave { wgpu::StoreOp::Store } else { wgpu::StoreOp::Discard } }),
                stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
            }),
            ..Default::default()
        });
        gpu.lines.draw(&mut pass);
    }
}

impl GpuMesh {
    fn bind(&self, pass: &mut wgpu::RenderPass, draw: &Draw) {
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        if let (Some(skin), Some(_)) = (&self.skin, draw.joints) {
            pass.set_vertex_buffer(1, skin.slice(..));
        }
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
    }
}

/// 本地包围盒经过 `m` 以后的世界包围盒（取 8 个角）。空网格的包围盒是反的，原样留着（拟合时角都是无穷，算没有）。
fn world_box(m: Mat4, lo: Vec3, hi: Vec3) -> (Vec3, Vec3) {
    (0..8).fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(a, b), i| {
        let corner = Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
        let p = m.transform_point3(corner);
        (a.min(p), b.max(p))
    })
}

impl Gpu {
    fn new(gfx: &Gfx) -> Self {
        let device = &gfx.device;
        let uniform = |dynamic: bool, size: usize| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: NonZeroU64::new(size as u64),
            },
            count: None,
        };
        let shadow_map = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
            count: None,
        };
        let cube = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::Cube, multisampled: false },
            count: None,
        };
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh globals"),
            entries: &[
                uniform(false, size_of::<Globals>()),
                shadow_map(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                shadow_map(3),
                cube(4),
                cube(5),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh draw"),
            entries: &[
                uniform(true, size_of::<DrawRaw>()),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(64) },
                    count: None,
                },
                // 变形差；阴影 pass 也用这个布局，所以只给顶点着色器
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(16) },
                    count: None,
                },
            ],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("meshes"),
            bind_group_layouts: &[Some(&globals_layout), Some(&draw_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("meshes"),
            source: wgpu::ShaderSource::Wgsl(shader_source(gfx.device.adapter_info().backend == wgpu::Backend::Gl).into()),
        });
        let culls = [Some(wgpu::Face::Back), Some(wgpu::Face::Front), None];
        let pipelines = [false, true]
            .into_iter()
            .flat_map(|blend| [false, true].into_iter().flat_map(move |skinned| culls.map(move |cull| (cull, skinned, blend))))
            .map(|(cull, skinned, blend)| pipeline(gfx, &layout, &module, cull, skinned, blend))
            .collect();

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow = ShadowGpu::new(gfx, &draw_layout);
        let sky = SkyGpu::new(gfx, &globals_layout, &module);
        let globals_group = globals_group(device, &globals_layout, &globals, &shadow, &sky);
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let stride = (size_of::<DrawRaw>() as u64).div_ceil(align) * align;
        let joints = joint_buffer(device, INITIAL_JOINTS);
        let morphs = MorphBuffer::new(device);
        let (draws, draws_group) = draw_buffer(device, &draw_layout, INITIAL_DRAWS * stride, &joints, &morphs.buffer);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("meshes"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let billboards = BillboardGpu::new(gfx, &texture_layout);
        let lines = LineGpu::new(gfx);
        Self {
            pipelines,
            globals,
            globals_layout,
            globals_group,
            shadow,
            sky,
            draw_layout,
            draws,
            draws_group,
            draw_capacity: INITIAL_DRAWS,
            stride,
            joints,
            joint_capacity: INITIAL_JOINTS,
            morphs,
            texture_layout,
            sampler,
            textures: Vec::new(),
            generation: 0,
            meshes: Vec::new(),
            depth: DepthTarget::default(),
            billboards,
            lines,
        }
    }

    fn world_bounds(&self, d: &Draw, joints: &[Mat4]) -> (Vec3, Vec3) {
        let (lo, hi) = self.meshes[d.mesh.0 as usize].bounds;
        let model = Mat4::from_cols_array_2d(&d.raw.model);
        match d.joints {
            // 蒙皮的顶点是各关节变换后的加权平均，落在每个关节各自变换后的包围盒合起来的范围里
            Some((start, count)) => joints[start as usize..(start + count) as usize]
                .iter()
                .map(|j| world_box(model * *j, lo, hi))
                .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(a, b), (c, e)| (a.min(c), b.max(e))),
            None => world_box(model, lo, hi),
        }
    }

    /// 上传还没上传过的网格（资源只增不减）；变形差的缓冲换了时重建 draw 的 bind group。
    fn upload(&mut self, gfx: &Gfx, assets: &Assets3d) {
        use wgpu::util::DeviceExt;
        let mut grew = false;
        for mesh in assets.meshes.get(self.meshes.len()..).unwrap_or_default() {
            let buffer = |label, contents: &[u8], usage| {
                gfx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
            };
            let (morph, g) = self.morphs.append(gfx, mesh);
            grew |= g;
            self.meshes.push(GpuMesh {
                vertices: buffer("mesh vertices", bytemuck::cast_slice(&mesh.vertices), wgpu::BufferUsages::VERTEX),
                skin: (!mesh.skin.is_empty()).then(|| buffer("mesh skin", bytemuck::cast_slice(&mesh.skin), wgpu::BufferUsages::VERTEX)),
                indices: buffer("mesh indices", bytemuck::cast_slice(&mesh.indices), wgpu::BufferUsages::INDEX),
                count: mesh.indices.len() as u32,
                bounds: crate::morph::bounds(mesh),
                morph,
            });
        }
        if grew {
            (self.draws, self.draws_group) = draw_buffer(&gfx.device, &self.draw_layout, self.draw_capacity * self.stride, &self.joints, &self.morphs.buffer);
        }
    }

    /// 贴图加载好了就建 bind group 缓存起来；没加载好的什么也不做（画的时候用白色）。
    fn cache_texture(&mut self, gfx: &Gfx, images: &GpuImages, id: ImageId) {
        let index = id.index() as usize;
        if self.textures.get(index).is_some_and(Option::is_some) {
            return;
        }
        let Some(view) = images.view(id) else { return };
        if self.textures.len() <= index {
            self.textures.resize_with(index + 1, || None);
        }
        self.textures[index] = Some(gfx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh texture"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        }));
    }

    fn texture(&self, id: ImageId) -> &wgpu::BindGroup {
        self.textures.get(id.index() as usize).and_then(Option::as_ref).or(self.textures[0].as_ref()).expect("白色贴图总是先上传")
    }

    /// 每个 draw 占 `stride` 字节，关节矩阵另放一个缓冲；容量不够时按两倍扩容。
    fn write_draws(&mut self, gfx: &Gfx, draws: &[Draw], bytes: &mut Vec<u8>, joints: &[Mat4]) {
        if draws.is_empty() {
            return;
        }
        let (needed, needed_joints) = (draws.len() as u64, joints.len() as u64);
        if needed > self.draw_capacity || needed_joints > self.joint_capacity {
            self.draw_capacity = self.draw_capacity.max(needed.next_power_of_two());
            if needed_joints > self.joint_capacity {
                self.joint_capacity = needed_joints.next_power_of_two();
                self.joints = joint_buffer(&gfx.device, self.joint_capacity);
            }
            (self.draws, self.draws_group) = draw_buffer(&gfx.device, &self.draw_layout, self.draw_capacity * self.stride, &self.joints, &self.morphs.buffer);
        }
        if !joints.is_empty() {
            gfx.queue.write_buffer(&self.joints, 0, bytemuck::cast_slice(joints));
        }
        bytes.clear();
        bytes.resize(draws.len() * self.stride as usize, 0);
        for (chunk, draw) in bytes.chunks_exact_mut(self.stride as usize).zip(draws) {
            chunk[..size_of::<DrawRaw>()].copy_from_slice(bytemuck::bytes_of(&draw.raw));
        }
        gfx.queue.write_buffer(&self.draws, 0, bytes);
    }
}

/// 着色器源码：前面拼上雾，采样盘换成 Rust 算的。GL 后端上 PCSS 退成泊松盘：GLSL ES 里同一张深度纹理
/// 不能既按比较采样又直接读，找挡光的那一步写不出来。
fn shader_source(gl: bool) -> String {
    let source = format!("{FOG_WGSL}{SHADER}").replace("POISSON_DISK", &crate::shadow_soft::disk_wgsl());
    if !gl {
        return source;
    }
    let (start, end) = (source.find("fn pcss(").expect("有 pcss"), source.find("/// 法线为").expect("有 receiver_slope"));
    let fallback = "fn pcss(layer: u32, c: vec4f, n: vec3f, texel: f32, scale: f32) -> f32 {\n    return filtered(shadow_map, layer, c, texel);\n}\n\n";
    format!("{}{fallback}{}", &source[..start], &source[end..])
}

/// 主 pass 的全局 bind group：uniform、两张阴影图、环境和天空立方体、BRDF LUT。阴影图、天空换了要重建。
fn globals_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, globals: &wgpu::Buffer, shadow: &ShadowGpu, sky: &SkyGpu) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mesh globals"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&shadow.dir.array) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&shadow.sampler) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&shadow.local.array) },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&sky.env_view) },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&sky.sky_view) },
            wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&sky.lut_view) },
            wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(&sky.sampler) },
        ],
    })
}

fn joint_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mesh joints"),
        size: capacity * size_of::<Mat4>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn draw_buffer(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, size: u64, joints: &wgpu::Buffer, morphs: &wgpu::Buffer) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mesh draws"),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mesh draws"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: NonZeroU64::new(size_of::<DrawRaw>() as u64),
                }),
            },
            wgpu::BindGroupEntry { binding: 1, resource: joints.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: morphs.as_entire_binding() },
        ],
    });
    (buffer, group)
}

/// 顶点缓冲的布局：普通的只有 `Vertex`，蒙皮的再加第二个缓冲放 [`Influence`]。
pub(crate) fn vertex_layouts(skinned: bool) -> Vec<Option<wgpu::VertexBufferLayout<'static>>> {
    const VERTEX: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
    const SKIN: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![3 => Uint16x4, 4 => Float32x4];
    let layout = |stride: usize, attributes: &'static [wgpu::VertexAttribute]| {
        Some(wgpu::VertexBufferLayout { array_stride: stride as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes })
    };
    let mut layouts = vec![layout(size_of::<Vertex>(), &VERTEX)];
    if skinned {
        layouts.push(layout(size_of::<Influence>(), &SKIN));
    }
    layouts
}

/// 半透明的：按 alpha 混合，深度照常比较但不写（后面的半透明不会被前面的挡掉）。
fn pipeline(gfx: &Gfx, layout: &wgpu::PipelineLayout, module: &wgpu::ShaderModule, cull: Option<wgpu::Face>, skinned: bool, blend: bool) -> wgpu::RenderPipeline {
    let alpha = wgpu::BlendState {
        color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
        alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
    };
    gfx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("meshes"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(if skinned { "vs_skinned" } else { "vs" }),
            buffers: &vertex_layouts(skinned),
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState { cull_mode: cull, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(!blend),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState { format: gfx.format, blend: blend.then_some(alpha), write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: gfx.pipeline_cache.as_ref(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_boxes_in_the_frustum_are_drawn() {
        let proj = crate::Camera3d { near: 0.1, far: 100.0, ..Default::default() }.view_proj(&crate::Transform3d::default(), 1.0);
        let unit = |c: Vec3| (c - Vec3::splat(0.5), c + Vec3::splat(0.5));
        assert!(in_view(proj, unit(Vec3::new(0.0, 0.0, -5.0))), "正前方");
        assert!(!in_view(proj, unit(Vec3::new(0.0, 0.0, 5.0))), "背后");
        assert!(!in_view(proj, unit(Vec3::new(20.0, 0.0, -5.0))), "旁边");
        assert!(!in_view(proj, unit(Vec3::new(0.0, 0.0, -200.0))), "远平面以外");
        assert!(in_view(proj, (Vec3::new(-100.0, -0.5, -3.0), Vec3::new(100.0, 0.5, -2.0))), "横穿视野、角都在外面");
        assert!(in_view(proj, (Vec3::splat(f32::NAN), Vec3::ONE)), "算坏了的照画");
    }

    #[test]
    fn gltf_models_resolve_once_loaded() {
        use std::sync::Arc;

        use nge_assets::{AssetPlugin, AssetServer, MemorySource};

        use crate::gltf_load::GltfModel;

        let source = MemorySource::default().with("asset:/@t/crate.glb", include_bytes!("../tests/data/test.glb").to_vec());
        let mut app = App::new();
        app.add_plugins(AssetPlugin::new().source(Arc::new(source))).add_plugins(Render3dPlugin);
        let (ok, missing) = app.world_mut().resource_scope(|world, mut server: Mut<AssetServer>| {
            let mut gltfs = world.resource_mut::<Assets<Gltf>>();
            (server.load(&mut gltfs, "asset:/@t/crate.glb"), server.load(&mut gltfs, "asset:/@t/missing.glb"))
        });
        let a = app.world_mut().spawn(GltfModel(ok)).id();
        let b = app.world_mut().spawn(GltfModel(ok)).id();
        let c = app.world_mut().spawn(GltfModel(missing)).id();
        let viewport = nge_core::Viewport { size: [100.0, 100.0], pixels: [100, 100] };
        app.tick(viewport, 0.016);
        AssetServer::apply(app.world_mut(), true);
        app.tick(viewport, 0.016);

        let world = app.world();
        let (ma, mb) = (world.get::<Model3d>(a).expect("加载完换成 Model3d"), world.get::<Model3d>(b).unwrap());
        assert_eq!(ma.0, mb.0, "同一个 glTF 只展开一次");
        assert_eq!(world.resource::<Assets3d>().model(ma.0).unwrap().parts.len(), 2);
        assert!(world.get::<GltfModel>(c).is_none() && world.get::<Model3d>(c).is_none(), "加载失败的去掉组件");
    }

    /// 翻译成 GLES 3.1 的 GLSL（顶点着色器读 storage buffer，3.0 不够）；返回出错的入口。
    fn glsl_errors(source: &str) -> Vec<String> {
        use naga::back::glsl;
        let module = naga::front::wgsl::parse_str(source).expect("WGSL 解析");
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).expect("校验");
        let options = glsl::Options { version: glsl::Version::Embedded { version: 310, is_webgl: false }, ..Default::default() };
        let mut errors = Vec::new();
        for (stage, entry) in [(naga::ShaderStage::Vertex, "vs"), (naga::ShaderStage::Fragment, "fs"), (naga::ShaderStage::Fragment, "fs_sky")] {
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
    fn the_gl_shader_drops_the_pcss_blocker_search() {
        let (gl, other) = (shader_source(true), shader_source(false));
        assert!(!gl.contains("textureLoad(shadow_map") && other.contains("textureLoad(shadow_map"), "GL 上不直接读深度图");
        assert_eq!(glsl_errors(&gl), Vec::<String>::new(), "GL 版转得成 GLSL ES");
        assert!(glsl_errors(&other).iter().any(|e| e.starts_with("fs:")), "别的后端用的版本在 GLSL 里转不了（所以要退）");
    }
}
