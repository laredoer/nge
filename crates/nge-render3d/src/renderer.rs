//! 网格渲染器：深度缓冲、每次 draw 一段动态 uniform、贴图按 ImageId 缓存 bind group。

use std::num::NonZeroU64;

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};
use nge_core::{Gfx, wgpu};
use nge_ecs::{App, ClearColor, Interleave, Plugin, PostUpdate, PreUpdate, RenderFrame, Renderer, SCENE_DEPTH_FORMAT, SceneDepth};

use nge_assets::{AssetApp, Assets, GpuImages, Image, ImageId};

use crate::animation::{Pose, animate};
use crate::assets::{Assets3d, MaterialId, MeshId, ModelId};
use crate::gltf_load::{Gltf, GltfModels, resolve_gltf_models};
use crate::camera::{Camera3d, Transform3d};
use crate::light::{AmbientLight, DirectionalLight, Globals, PointLight};
use crate::mesh::{Influence, Vertex};
use crate::shadow::{ShadowGpu, Shadows, fit};

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
            .add_systems(PostUpdate, animate);
        app.insert_resource(Assets3d::default()).init_resource::<AmbientLight>().init_resource::<Shadows>().add_renderer(MeshRenderer::default());
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
    /// x：这个 draw 的关节矩阵从第几个开始。
    skin: [u32; 4],
}

struct Draw {
    mesh: MeshId,
    texture: ImageId,
    /// 0–2 按剔除方式，蒙皮的再加 3。
    pipeline: usize,
    /// 蒙皮的：关节矩阵在这一帧关节缓冲里的（起点, 个数）。
    joints: Option<(u32, u32)>,
    raw: DrawRaw,
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    /// 关节权重（第二个顶点缓冲），蒙皮的网格才有。
    skin: Option<wgpu::Buffer>,
    indices: wgpu::Buffer,
    count: u32,
    /// 本地坐标的包围盒，拟合阴影图用。
    bounds: (Vec3, Vec3),
}

struct Gpu {
    /// [背面剔除, 正面剔除（镜像变换，绕序反了）, 双面]，后三个是蒙皮的。
    pipelines: [wgpu::RenderPipeline; 6],
    globals: wgpu::Buffer,
    globals_layout: wgpu::BindGroupLayout,
    globals_group: wgpu::BindGroup,
    shadow: ShadowGpu,
    draw_layout: wgpu::BindGroupLayout,
    draws: wgpu::Buffer,
    draws_group: wgpu::BindGroup,
    draw_capacity: u64,
    stride: u64,
    /// 这一帧所有蒙皮 draw 的关节矩阵（storage buffer，和 draw 的 uniform 在一个 bind group）。
    joints: wgpu::Buffer,
    joint_capacity: u64,
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
    joints: Vec<Mat4>,
    warned_lights: bool,
}

impl Renderer for MeshRenderer {
    fn render(&mut self, world: &mut World, gfx: &Gfx, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let gpu = self.gpu.get_or_insert_with(|| Gpu::new(gfx));
        let mut globals = globals(world, gfx.viewport.size.into());
        if globals.1 && !std::mem::replace(&mut self.warned_lights, true) {
            log::warn!("灯光数量超过上限，多出来的已忽略");
        }

        world.resource_scope(|world, mut images: Mut<GpuImages>| images.sync(gfx, world.resource::<Assets<Image>>()));
        gpu.upload(gfx, world.resource::<Assets3d>());
        collect(world, &mut self.draws, &mut self.joints);
        self.draws.retain(|d| (d.mesh.0 as usize) < gpu.meshes.len());
        // 先按管线、再按贴图排，减少切换
        self.draws.sort_by_key(|d| (d.pipeline, d.texture));
        let images = world.resource::<GpuImages>();
        for id in std::iter::once(ImageId::WHITE).chain(self.draws.iter().map(|d| d.texture)) {
            gpu.cache_texture(gfx, images, id);
        }
        gpu.write_draws(gfx, &self.draws, &mut self.bytes, &self.joints);
        let shadowed = gpu.prepare_shadow(gfx, &mut globals.0, &self.draws, &self.joints, *world.resource::<Shadows>());
        gfx.queue.write_buffer(&gpu.globals, 0, bytemuck::bytes_of(&globals.0));
        if shadowed {
            gpu.draw_shadow(encoder, &self.draws);
        }
        let depth = gpu.depth_view(gfx, target.texture().size());
        // 混排时深度留给后面的精灵
        let interleave = world.resource::<Interleave>().0;
        if interleave {
            let view_proj = bytemuck::cast(globals.0.view_proj);
            world.resource_mut::<RenderFrame>().set_depth(SceneDepth { view: depth.clone(), view_proj });
        }

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
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: if interleave { wgpu::StoreOp::Store } else { wgpu::StoreOp::Discard } }),
                stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
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
            mesh.bind(&mut pass, draw);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        }
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

/// 收集这一帧的 draw；蒙皮的部件把关节矩阵追加到 `joints`。
fn collect(world: &mut World, draws: &mut Vec<Draw>, joints: &mut Vec<Mat4>) {
    draws.clear();
    joints.clear();
    let mut items: Vec<(MeshId, MaterialId, Mat4, Option<(u32, u32)>)> = Vec::new();
    for (mesh, t) in world.query::<(&Mesh3d, &Transform3d)>().iter(world) {
        items.push((mesh.mesh, mesh.material, t.matrix(), None));
    }
    // 先建查询再借资源（建查询要 &mut World）
    let mut models = world.query::<(&Model3d, &Transform3d, Option<&Pose>, Has<crate::Animator>)>();
    let assets = world.resource::<Assets3d>();
    for (model, t, pose, playing) in models.iter(world) {
        let Some(model) = assets.model(model.0) else { continue };
        let m = t.matrix();
        let skeleton = &model.skeleton;
        // 单独去掉 Animator 时 Pose 还在，不再按它画
        let posed = pose.filter(|_| playing).map(|p| &p.nodes[..]).filter(|n| n.len() == skeleton.nodes.len());
        let skinned = |p: &crate::Part| p.skin.is_some() && assets.mesh(p.mesh).is_some_and(|mesh| !mesh.skin.is_empty());
        // 蒙皮的部件要每个节点的矩阵，没在播动画时用不播动画的姿势
        let rest;
        let nodes = match posed {
            None if model.parts.iter().any(skinned) => {
                rest = skeleton.rest();
                Some(&rest[..])
            }
            nodes => nodes,
        };
        for p in &model.parts {
            match (p.skin, nodes) {
                (Some(skin), Some(nodes)) if skinned(p) => {
                    let start = joints.len();
                    skeleton.joints(skin as usize, nodes, joints);
                    items.push((p.mesh, p.material, m, Some((start as u32, (joints.len() - start) as u32))));
                }
                _ => {
                    let local = match (p.node, posed) {
                        (Some(n), Some(nodes)) => nodes[n as usize],
                        _ => p.local,
                    };
                    items.push((p.mesh, p.material, m * local, None));
                }
            }
        }
    }
    for (mesh, material, model, skin) in items {
        // 缩放为 0 的物体看不见，也没有法线矩阵
        let det = model.determinant();
        let inverse = model.inverse();
        if det == 0.0 || !inverse.is_finite() {
            continue;
        }
        // 行列式为负（镜像）时三角形绕序反了，单面材质改剔正面，着色器里也按此判断正反面
        let mirrored = det < 0.0;
        let m = assets.material(material);
        let cull = if m.double_sided { 2 } else { usize::from(mirrored) };
        draws.push(Draw {
            mesh,
            texture: m.base_color_texture.unwrap_or(ImageId::WHITE),
            pipeline: cull + if skin.is_some() { 3 } else { 0 },
            joints: skin,
            raw: DrawRaw {
                model: model.to_cols_array_2d(),
                normal: inverse.transpose().to_cols_array_2d(),
                base_color: m.base_color.into(),
                params: [m.metallic, m.roughness, m.alpha_cutoff.unwrap_or(-1.0), if mirrored { 1.0 } else { 0.0 }],
                emissive: m.emissive.extend(0.0).into(),
                skin: [skin.map_or(0, |s| s.0), 0, 0, 0],
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
            entries: &[
                uniform(false, size_of::<Globals>()),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
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
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let culls = [Some(wgpu::Face::Back), Some(wgpu::Face::Front), None];
        let pipelines = [false, true].map(|skinned| culls.map(|cull| pipeline(gfx, &layout, &module, cull, skinned)));
        let [[a, b, c], [d, e, f]] = pipelines;

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow = ShadowGpu::new(gfx, &draw_layout);
        let globals_group = globals_group(device, &globals_layout, &globals, &shadow);
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let stride = (size_of::<DrawRaw>() as u64).div_ceil(align) * align;
        let joints = joint_buffer(device, INITIAL_JOINTS);
        let (draws, draws_group) = draw_buffer(device, &draw_layout, INITIAL_DRAWS * stride, &joints);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("meshes"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipelines: [a, b, c, d, e, f],
            globals,
            globals_layout,
            globals_group,
            shadow,
            draw_layout,
            draws,
            draws_group,
            draw_capacity: INITIAL_DRAWS,
            stride,
            joints,
            joint_capacity: INITIAL_JOINTS,
            texture_layout,
            sampler,
            textures: Vec::new(),
            meshes: Vec::new(),
            depth: None,
        }
    }

    /// 有投影子的光时，按所有 draw 的世界包围盒拟合光的矩阵写进 `globals`，返回要不要画阴影 pass。
    /// 拟合不了（没有东西）时关掉阴影。
    fn prepare_shadow(&mut self, gfx: &Gfx, globals: &mut Globals, draws: &[Draw], joints: &[Mat4], settings: Shadows) -> bool {
        let index = globals.counts[2] as usize;
        if index == 0 {
            return false;
        }
        let direction = -Vec3::from_slice(&globals.dirs[index - 1].to_light[..3]);
        let size = settings.clamped_size();
        let boxes = draws.iter().filter_map(|d| {
            let (lo, hi) = self.meshes[d.mesh.0 as usize].bounds;
            let model = Mat4::from_cols_array_2d(&d.raw.model);
            let b = match d.joints {
                // 蒙皮的顶点是各关节变换后的加权平均，落在每个关节各自变换后的包围盒合起来的范围里
                Some((start, count)) => joints[start as usize..(start + count) as usize]
                    .iter()
                    .map(|j| world_box(model * *j, lo, hi))
                    .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(a, b), (c, e)| (a.min(c), b.max(e))),
                None => world_box(model, lo, hi),
            };
            // 一个算坏了的包围盒不该让整个场景的影子消失
            (b.0.is_finite() && b.1.is_finite()).then_some(b)
        });
        let Some((light, texel)) = fit(direction, boxes, size) else {
            globals.counts[2] = 0;
            return false;
        };
        if self.shadow.resize(gfx, size) {
            self.globals_group = globals_group(&gfx.device, &self.globals_layout, &self.globals, &self.shadow);
        }
        globals.light_view_proj = light.to_cols_array_2d();
        // 往法线方向推一个半像素，斜面上不长阴影痘
        globals.shadow = [texel * 1.5, 1.0 / size as f32, 0.0, 0.0];
        gfx.queue.write_buffer(&self.shadow.light, 0, bytemuck::bytes_of(&light.to_cols_array()));
        true
    }

    /// 从光那边只画深度。
    fn draw_shadow(&self, encoder: &mut wgpu::CommandEncoder, draws: &[Draw]) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shadow"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.shadow.map,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_bind_group(0, &self.shadow.light_group, &[]);
        for (i, draw) in draws.iter().enumerate() {
            let mesh = &self.meshes[draw.mesh.0 as usize];
            pass.set_pipeline(&self.shadow.pipelines[usize::from(draw.joints.is_some())]);
            pass.set_bind_group(1, &self.draws_group, &[(i as u64 * self.stride) as u32]);
            mesh.bind(&mut pass, draw);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        }
    }

    /// 上传还没上传过的网格（资源只增不减）。
    fn upload(&mut self, gfx: &Gfx, assets: &Assets3d) {
        use wgpu::util::DeviceExt;
        for mesh in assets.meshes.get(self.meshes.len()..).unwrap_or_default() {
            let buffer = |label, contents: &[u8], usage| {
                gfx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
            };
            let bounds = mesh.vertices.iter().fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), v| {
                let p = Vec3::from(v.position);
                (lo.min(p), hi.max(p))
            });
            self.meshes.push(GpuMesh {
                vertices: buffer("mesh vertices", bytemuck::cast_slice(&mesh.vertices), wgpu::BufferUsages::VERTEX),
                skin: (!mesh.skin.is_empty()).then(|| buffer("mesh skin", bytemuck::cast_slice(&mesh.skin), wgpu::BufferUsages::VERTEX)),
                indices: buffer("mesh indices", bytemuck::cast_slice(&mesh.indices), wgpu::BufferUsages::INDEX),
                count: mesh.indices.len() as u32,
                bounds,
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
            (self.draws, self.draws_group) = draw_buffer(&gfx.device, &self.draw_layout, self.draw_capacity * self.stride, &self.joints);
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

/// 主 pass 的全局 bind group：uniform 加阴影图。阴影图换了要重建。
fn globals_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, globals: &wgpu::Buffer, shadow: &ShadowGpu) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mesh globals"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&shadow.map) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&shadow.sampler) },
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

fn draw_buffer(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, size: u64, joints: &wgpu::Buffer) -> (wgpu::Buffer, wgpu::BindGroup) {
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

fn pipeline(gfx: &Gfx, layout: &wgpu::PipelineLayout, module: &wgpu::ShaderModule, cull: Option<wgpu::Face>, skinned: bool) -> wgpu::RenderPipeline {
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
        assert_eq!(size_of::<DrawRaw>(), 64 * 2 + 16 * 4);
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
