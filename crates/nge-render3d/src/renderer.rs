//! 网格渲染器：深度缓冲、每次 draw 一段动态 uniform、贴图按 ImageId 缓存 bind group。

use std::num::NonZeroU64;

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};
use nge_core::{Gfx, wgpu};
use nge_ecs::{App, ClearColor, Plugin, PreUpdate, RenderFrame, Renderer};

use nge_assets::{AssetApp, Assets, GpuImages, Image, ImageId};

use crate::assets::{Assets3d, MaterialId, MeshId, ModelId};
use crate::gltf_load::{Gltf, GltfModels, resolve_gltf_models};
use crate::camera::{Camera3d, Transform3d};
use crate::light::{AmbientLight, DirectionalLight, Globals, PointLight};
use crate::mesh::Vertex;

const SHADER: &str = include_str!("shader.wgsl");
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const INITIAL_DRAWS: u64 = 64;

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
            .add_systems(PreUpdate, resolve_gltf_models.after(nge_assets::AssetsApplied));
        app.insert_resource(Assets3d::default()).init_resource::<AmbientLight>().add_renderer(MeshRenderer::default());
    }
}

/// 和着色器里的 `Draw` 一一对应。
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct DrawRaw {
    model: [[f32; 4]; 4],
    normal: [[f32; 4]; 4],
    base_color: [f32; 4],
    params: [f32; 4],
    emissive: [f32; 4],
}

struct Draw {
    mesh: MeshId,
    texture: ImageId,
    pipeline: usize,
    raw: DrawRaw,
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

struct Gpu {
    /// [背面剔除, 正面剔除（镜像变换，绕序反了）, 双面]
    pipelines: [wgpu::RenderPipeline; 3],
    globals: wgpu::Buffer,
    globals_group: wgpu::BindGroup,
    draw_layout: wgpu::BindGroupLayout,
    draws: wgpu::Buffer,
    draws_group: wgpu::BindGroup,
    draw_capacity: u64,
    stride: u64,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// 按贴图编号缓存；贴图还没加载完的不缓存，先用白色贴图的。
    textures: Vec<Option<wgpu::BindGroup>>,
    meshes: Vec<GpuMesh>,
    depth: Option<(wgpu::TextureView, wgpu::Extent3d)>,
}

#[derive(Default)]
struct MeshRenderer {
    gpu: Option<Gpu>,
    draws: Vec<Draw>,
    bytes: Vec<u8>,
    warned_lights: bool,
}

impl Renderer for MeshRenderer {
    fn render(&mut self, world: &mut World, gfx: &Gfx, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let gpu = self.gpu.get_or_insert_with(|| Gpu::new(gfx));
        let globals = globals(world, gfx.viewport.size.into());
        if globals.1 && !std::mem::replace(&mut self.warned_lights, true) {
            log::warn!("灯光数量超过上限，多出来的已忽略");
        }
        gfx.queue.write_buffer(&gpu.globals, 0, bytemuck::bytes_of(&globals.0));

        world.resource_scope(|world, mut images: Mut<GpuImages>| images.sync(gfx, world.resource::<Assets<Image>>()));
        gpu.upload(gfx, world.resource::<Assets3d>());
        collect(world, &mut self.draws);
        self.draws.retain(|d| (d.mesh.0 as usize) < gpu.meshes.len());
        // 先按管线、再按贴图排，减少切换
        self.draws.sort_by_key(|d| (d.pipeline, d.texture));
        let images = world.resource::<GpuImages>();
        for id in std::iter::once(ImageId::WHITE).chain(self.draws.iter().map(|d| d.texture)) {
            gpu.cache_texture(gfx, images, id);
        }
        gpu.write_draws(gfx, &self.draws, &mut self.bytes);
        let depth = gpu.depth_view(gfx, target.texture().size());

        let clear = *world.resource::<ClearColor>();
        let load = world.resource_mut::<RenderFrame>().color_load(clear);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("meshes"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_bind_group(0, &gpu.globals_group, &[]);
        for (i, draw) in self.draws.iter().enumerate() {
            let mesh = &gpu.meshes[draw.mesh.0 as usize];
            let texture = gpu.texture(draw.texture);
            pass.set_pipeline(&gpu.pipelines[draw.pipeline]);
            pass.set_bind_group(1, &gpu.draws_group, &[(i as u64 * gpu.stride) as u32]);
            pass.set_bind_group(2, texture, &[]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        }
    }
}

/// 没有相机实体时，按默认相机放在 (0, 0, 5) 看向原点。
fn globals(world: &mut World, size: Vec2) -> (Globals, bool) {
    let (camera, eye) = world
        .query::<(&Camera3d, &Transform3d)>()
        .iter(world)
        .next()
        .map(|(c, t)| (*c, *t))
        .unwrap_or_else(|| (Camera3d::default(), Transform3d::from_xyz(0.0, 0.0, 5.0)));
    let view_proj = camera.view_proj(&eye, size.x / size.y.max(1.0));
    let ambient = *world.resource::<AmbientLight>();
    let dirs: Vec<DirectionalLight> = world.query::<&DirectionalLight>().iter(world).copied().collect();
    let points: Vec<(PointLight, Vec3)> = world.query::<(&PointLight, &Transform3d)>().iter(world).map(|(l, t)| (*l, t.translation)).collect();
    Globals::new(view_proj, camera.eye(&eye), ambient, &dirs, points.iter().map(|(l, p)| (l, *p)))
}

fn collect(world: &mut World, draws: &mut Vec<Draw>) {
    draws.clear();
    let mut items: Vec<(MeshId, MaterialId, Mat4)> = Vec::new();
    for (mesh, t) in world.query::<(&Mesh3d, &Transform3d)>().iter(world) {
        items.push((mesh.mesh, mesh.material, t.matrix()));
    }
    // 先建查询再借资源（建查询要 &mut World）
    let mut models = world.query::<(&Model3d, &Transform3d)>();
    let assets = world.resource::<Assets3d>();
    for (model, t) in models.iter(world) {
        let Some(model) = assets.model(model.0) else { continue };
        let m = t.matrix();
        items.extend(model.parts.iter().map(|p| (p.mesh, p.material, m * p.local)));
    }
    for (mesh, material, model) in items {
        // 缩放为 0 的物体看不见，也没有法线矩阵
        let det = model.determinant();
        let inverse = model.inverse();
        if det == 0.0 || !inverse.is_finite() {
            continue;
        }
        // 行列式为负（镜像）时三角形绕序反了，单面材质改剔正面，着色器里也按此判断正反面
        let mirrored = det < 0.0;
        let m = assets.material(material);
        draws.push(Draw {
            mesh,
            texture: m.base_color_texture.unwrap_or(ImageId::WHITE),
            pipeline: if m.double_sided { 2 } else { usize::from(mirrored) },
            raw: DrawRaw {
                model: model.to_cols_array_2d(),
                normal: inverse.transpose().to_cols_array_2d(),
                base_color: m.base_color.into(),
                params: [m.metallic, m.roughness, m.alpha_cutoff.unwrap_or(-1.0), if mirrored { 1.0 } else { 0.0 }],
                emissive: m.emissive.extend(0.0).into(),
            },
        });
    }
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
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh globals"),
            entries: &[uniform(false, size_of::<Globals>())],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh draw"),
            entries: &[uniform(true, size_of::<DrawRaw>())],
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
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipelines = [Some(wgpu::Face::Back), Some(wgpu::Face::Front), None].map(|cull| pipeline(gfx, &layout, &module, cull));

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let stride = (size_of::<DrawRaw>() as u64).div_ceil(align) * align;
        let (draws, draws_group) = draw_buffer(device, &draw_layout, INITIAL_DRAWS * stride);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("meshes"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipelines,
            globals,
            globals_group,
            draw_layout,
            draws,
            draws_group,
            draw_capacity: INITIAL_DRAWS,
            stride,
            texture_layout,
            sampler,
            textures: Vec::new(),
            meshes: Vec::new(),
            depth: None,
        }
    }

    /// 上传还没上传过的网格（资源只增不减）。
    fn upload(&mut self, gfx: &Gfx, assets: &Assets3d) {
        use wgpu::util::DeviceExt;
        for mesh in assets.meshes.get(self.meshes.len()..).unwrap_or_default() {
            let buffer = |label, contents: &[u8], usage| {
                gfx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
            };
            self.meshes.push(GpuMesh {
                vertices: buffer("mesh vertices", bytemuck::cast_slice(&mesh.vertices), wgpu::BufferUsages::VERTEX),
                indices: buffer("mesh indices", bytemuck::cast_slice(&mesh.indices), wgpu::BufferUsages::INDEX),
                count: mesh.indices.len() as u32,
            });
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

    /// 每个 draw 占 `stride` 字节；容量不够时按两倍扩容。
    fn write_draws(&mut self, gfx: &Gfx, draws: &[Draw], bytes: &mut Vec<u8>) {
        if draws.is_empty() {
            return;
        }
        let needed = draws.len() as u64;
        if needed > self.draw_capacity {
            self.draw_capacity = needed.next_power_of_two();
            (self.draws, self.draws_group) = draw_buffer(&gfx.device, &self.draw_layout, self.draw_capacity * self.stride);
        }
        bytes.clear();
        bytes.resize(draws.len() * self.stride as usize, 0);
        for (chunk, draw) in bytes.chunks_exact_mut(self.stride as usize).zip(draws) {
            chunk[..size_of::<DrawRaw>()].copy_from_slice(bytemuck::bytes_of(&draw.raw));
        }
        gfx.queue.write_buffer(&self.draws, 0, bytes);
    }

    /// 深度缓冲跟着渲染目标的大小走。
    fn depth_view(&mut self, gfx: &Gfx, size: wgpu::Extent3d) -> &wgpu::TextureView {
        let size = wgpu::Extent3d { depth_or_array_layers: 1, ..size };
        if self.depth.as_ref().is_none_or(|(_, s)| *s != size) {
            let texture = gfx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("mesh depth"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.depth = Some((texture.create_view(&Default::default()), size));
        }
        &self.depth.as_ref().expect("刚创建").0
    }
}

fn draw_buffer(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, size: u64) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mesh draws"),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mesh draws"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buffer,
                offset: 0,
                size: NonZeroU64::new(size_of::<DrawRaw>() as u64),
            }),
        }],
    });
    (buffer, group)
}

fn pipeline(gfx: &Gfx, layout: &wgpu::PipelineLayout, module: &wgpu::ShaderModule, cull: Option<wgpu::Face>) -> wgpu::RenderPipeline {
    let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
    gfx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("meshes"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            })],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState { cull_mode: cull, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState { format: gfx.format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_layout_matches_shader() {
        assert_eq!(size_of::<DrawRaw>(), 64 * 2 + 16 * 3);
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
}
