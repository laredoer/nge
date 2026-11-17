//! 阴影：方向光（一张图框住所有物体，或按相机视锥分段的级联）、一盏聚光灯、一盏点光源的六面。
//! 每层画一遍只写深度，主 pass 里 3×3 PCF 采样（见 docs/3d-more.md、docs/render3d-more.md）。

use std::num::NonZeroU64;

use bevy_ecs::prelude::Resource;
use glam::camera::rh::proj::directx;
use glam::{Mat4, Vec3};
use nge_core::{Gfx, wgpu};

use crate::camera::{Camera3d, Transform3d};
use crate::light::{Globals, LOCAL_LAYERS};
use crate::shadow_views::{MAX_CASCADES, cascade_matrix, intersects, light_view, point_matrices, slice_corners, splits, spot_matrix};

/// 阴影的设置。
/// `cascades = 1` 时方向光的阴影图按光的方向框住所有 3D 物体（模型少的场景够用；物体铺得很开时一个像素管的范围大，阴影边缘会粗）；
/// 大于 1 时把相机前方 `distance` 以内切成几段，各画一张，近处的影子细，最后一段以外没有影子。
/// 级联只对透视相机有用：正交相机每段的横截面都是整屏，近处不会更细，所以按 1 段处理。
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct Shadows {
    /// 方向光阴影图边长（像素），会被限制在 [64, 4096]。
    pub size: u32,
    /// 方向光分几段，会被限制在 [1, 4]。
    pub cascades: u32,
    /// 分段时覆盖到相机前方多远（不超过相机的 `far`）；不是正数时按默认值。
    pub distance: f32,
    /// 聚光灯、点光源阴影图边长（像素），会被限制在 [64, 4096]。
    pub local_size: u32,
}

impl Default for Shadows {
    fn default() -> Self {
        Self { size: 2048, cascades: 1, distance: 50.0, local_size: 512 }
    }
}

impl Shadows {
    pub(crate) fn clamped_size(&self) -> u32 {
        self.size.clamp(64, 4096)
    }

    pub(crate) fn clamped_cascades(&self) -> usize {
        (self.cascades as usize).clamp(1, MAX_CASCADES)
    }

    pub(crate) fn clamped_distance(&self) -> f32 {
        if self.distance > 0.0 { self.distance } else { Self::default().distance }
    }

    pub(crate) fn clamped_local_size(&self) -> u32 {
        self.local_size.clamp(64, 4096)
    }
}

pub(crate) const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// 光矩阵的槽位：方向光 4 段，接着聚光灯 1 个、点光源 6 面。
const SLOTS: usize = MAX_CASCADES + LOCAL_LAYERS;
/// 法线方向往外推几个像素，斜面上不长阴影痘。
const NORMAL_OFFSET: f32 = 1.5;

/// 光的方向 + 所有物体的世界包围盒 → 光的正交矩阵（世界 → 裁剪空间，深度 0–1）和一个阴影图像素在世界里多大。
/// 没有物体、方向是 0 时是 `None`。
pub(crate) fn fit(direction: Vec3, boxes: impl IntoIterator<Item = (Vec3, Vec3)>, size: u32) -> Option<(Mat4, f32)> {
    let view = light_view(direction)?;
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for (min, max) in boxes {
        for i in 0..8 {
            let corner = Vec3::new(if i & 1 == 0 { min.x } else { max.x }, if i & 2 == 0 { min.y } else { max.y }, if i & 4 == 0 { min.z } else { max.z });
            let p = view.transform_point3(corner);
            (lo, hi) = (lo.min(p), hi.max(p));
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        return None;
    }
    // 四周留一点，边上的物体不被切掉；扁平的方向至少一个单位
    let pad = ((hi - lo).max_element() * 0.02).max(1e-3);
    let (lo, hi) = (lo - Vec3::splat(pad), hi + Vec3::splat(pad));
    let side = (hi.x - lo.x).max(hi.y - lo.y);
    // 看向 -z：近的 z 大
    let proj = directx::orthographic(lo.x, hi.x, lo.y, hi.y, -hi.z, -lo.z);
    Some((proj * view, side / size as f32))
}

/// 方向光每段的（矩阵, 一个像素在世界里多大, 远端在相机前方多远）写进 `out`。一段时框住所有物体、没有远端；拟合不了时 `out` 是空的。
/// 正交相机每段的横截面都一样大，近处不会更细，按一段算。
fn directional(direction: Vec3, view: &CameraView, casters: &[Caster], settings: Shadows, out: &mut Vec<(Mat4, f32, f32)>) {
    out.clear();
    let size = settings.clamped_size();
    let boxes = casters.iter().map(|c| c.bounds);
    let count = if view.camera.ortho_height().is_some() { 1 } else { settings.clamped_cascades() };
    if count == 1 {
        out.extend(fit(direction, boxes, size).map(|(m, texel)| (m, texel, f32::MAX)));
        return;
    }
    let near = view.camera.near.max(0.0);
    let end = view.camera.far.min(settings.clamped_distance());
    if !(end > near) || casters.is_empty() {
        return;
    }
    let ends = splits(near, end, count);
    let mut from = near;
    for &to in &ends[..count] {
        let corners = slice_corners(&view.camera, view.aspect, from, to);
        let Some((m, texel)) = cascade_matrix(direction, &view.eye, &corners, boxes.clone(), size) else {
            out.clear();
            return;
        };
        out.push((m, texel, to));
        from = to;
    }
}

/// 拍这一帧的相机。
pub(crate) struct CameraView {
    pub camera: Camera3d,
    pub eye: Transform3d,
    pub aspect: f32,
}

/// 投影子的 draw：在这一帧 draw 列表里的下标、是否蒙皮、世界包围盒。
pub(crate) struct Caster {
    pub index: usize,
    pub skinned: bool,
    pub bounds: (Vec3, Vec3),
}

/// 一张深度数组：采样用整张，画的时候一层一层地画。
pub(crate) struct ShadowMap {
    pub array: wgpu::TextureView,
    layers: Vec<wgpu::TextureView>,
    size: u32,
}

impl ShadowMap {
    fn new(gfx: &Gfx, label: &str, size: u32, layers: u32) -> Self {
        let texture = gfx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: layers },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let array = texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let layers = (0..layers)
            .map(|i| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        Self { array, layers, size }
    }

    /// 尺寸或层数变了就重建，返回有没有重建。
    fn resize(&mut self, gfx: &Gfx, label: &str, size: u32, layers: u32) -> bool {
        if size == self.size && layers as usize == self.layers.len() {
            return false;
        }
        *self = Self::new(gfx, label, size, layers);
        true
    }
}

/// 这一帧要画的一层。
struct Pass {
    local: bool,
    layer: usize,
    slot: usize,
    view_proj: Mat4,
}

/// 阴影 pass 的 GPU 资源。主 pass 的全局 bind group 要引用阴影图，所以图换了时由渲染器重建它。
pub(crate) struct ShadowGpu {
    /// [普通, 蒙皮]
    pipelines: [wgpu::RenderPipeline; 2],
    /// 每层一个光矩阵，按 `stride` 对齐，动态 offset 选层。
    light: wgpu::Buffer,
    light_group: wgpu::BindGroup,
    stride: u64,
    bytes: Vec<u8>,
    pub sampler: wgpu::Sampler,
    /// 方向光：`size × size × 段数`。
    pub dir: ShadowMap,
    /// 聚光灯和点光源：`local_size × local_size × 7`。
    pub local: ShadowMap,
    passes: Vec<Pass>,
    /// 这一帧方向光每段的结果，每帧清空复用。
    cascades: Vec<(Mat4, f32, f32)>,
}

impl ShadowGpu {
    pub fn new(gfx: &Gfx, draw_layout: &wgpu::BindGroupLayout) -> Self {
        let device = &gfx.device;
        let light_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow light"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: NonZeroU64::new(64) },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow"),
            bind_group_layouts: &[Some(&light_layout), Some(draw_layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("shadow"), source: wgpu::ShaderSource::Wgsl(include_str!("shadow.wgsl").into()) });
        let pipeline = |skinned: bool| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some(if skinned { "vs_skinned" } else { "vs" }),
                buffers: &crate::renderer::vertex_layouts(skinned),
                compilation_options: Default::default(),
            },
            // 两面都画：薄片、单面的平面也挡光
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: SHADOW_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                // 斜着照的面按斜率多推一点，免得自己挡自己（阴影痘）
                bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
            }),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: gfx.pipeline_cache.as_ref(),
        });
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let stride = 64u64.div_ceil(align) * align;
        let light = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow light"),
            size: stride * SLOTS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow light"),
            layout: &light_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &light, offset: 0, size: NonZeroU64::new(64) }),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        Self {
            pipelines: [pipeline(false), pipeline(true)],
            light,
            light_group,
            stride,
            bytes: Vec::new(),
            sampler,
            dir: ShadowMap::new(gfx, "shadow map", 1, 1),
            local: ShadowMap::new(gfx, "local shadow map", 1, LOCAL_LAYERS as u32),
            passes: Vec::new(),
            cascades: Vec::new(),
        }
    }

    /// 算出这一帧每层的光矩阵写进 `globals`，记下要画的层；拟合不了（没有东西、灯的参数坏了）的灯关掉阴影。
    /// 返回阴影图有没有重建（主 pass 的 bind group 要跟着换）。
    pub fn prepare(&mut self, gfx: &Gfx, globals: &mut Globals, view: &CameraView, casters: &[Caster], settings: Shadows) -> bool {
        self.passes.clear();
        let mut rebuilt = false;
        let forward = view.eye.rotation * Vec3::NEG_Z;
        globals.view_forward = forward.extend(-forward.dot(view.eye.translation)).into();

        let index = globals.counts[2] as usize;
        let mut cascades = std::mem::take(&mut self.cascades);
        cascades.clear();
        if index > 0 {
            directional(-Vec3::from_slice(&globals.dirs[index - 1].to_light[..3]), view, casters, settings, &mut cascades);
        }
        if cascades.is_empty() {
            globals.counts[2] = 0;
        } else {
            let size = settings.clamped_size();
            rebuilt |= self.dir.resize(gfx, "shadow map", size, cascades.len() as u32);
            for (i, (m, texel, end)) in cascades.iter().enumerate() {
                globals.cascades[i] = m.to_cols_array_2d();
                globals.splits[i] = *end;
                globals.cascade_offset[i] = texel * NORMAL_OFFSET;
                self.passes.push(Pass { local: false, layer: i, slot: i, view_proj: *m });
            }
            globals.casters[2] = cascades.len() as u32;
            globals.shadow[0] = 1.0 / size as f32;
        }
        self.cascades = cascades;

        let local_size = settings.clamped_local_size();
        // 透视的图一个像素在世界里多大 = 到灯的（沿视线的）距离 × 2 tan(半张角) / 边长
        let spot = (globals.casters[0] as usize).checked_sub(1).filter(|_| !casters.is_empty()).and_then(|i| {
            let s = globals.spots[i];
            let outer = s.direction[3].clamp(-1.0, 1.0).acos();
            spot_matrix(Vec3::from_slice(&s.position_range[..3]), Vec3::from_slice(&s.direction[..3]), outer, s.position_range[3]).map(|m| (m, outer))
        });
        match spot {
            Some((m, outer)) => {
                globals.locals[0] = m.to_cols_array_2d();
                globals.shadow[2] = 2.0 * outer.tan() / local_size as f32 * NORMAL_OFFSET;
                self.passes.push(Pass { local: true, layer: 0, slot: MAX_CASCADES, view_proj: m });
            }
            None => globals.casters[0] = 0,
        }
        let point = (globals.casters[1] as usize).checked_sub(1).filter(|_| !casters.is_empty()).and_then(|i| {
            let p = globals.points[i];
            point_matrices(Vec3::from_slice(&p.position_range[..3]), p.position_range[3])
        });
        match point {
            Some(faces) => {
                for (i, m) in faces.into_iter().enumerate() {
                    globals.locals[1 + i] = m.to_cols_array_2d();
                    self.passes.push(Pass { local: true, layer: 1 + i, slot: MAX_CASCADES + 1 + i, view_proj: m });
                }
                globals.shadow[3] = 2.0 / local_size as f32 * NORMAL_OFFSET;
            }
            None => globals.casters[1] = 0,
        }
        if globals.casters[0] != 0 || globals.casters[1] != 0 {
            rebuilt |= self.local.resize(gfx, "local shadow map", local_size, LOCAL_LAYERS as u32);
            globals.shadow[1] = 1.0 / local_size as f32;
        }

        if !self.passes.is_empty() {
            self.bytes.clear();
            self.bytes.resize(self.stride as usize * SLOTS, 0);
            for p in &self.passes {
                let at = p.slot * self.stride as usize;
                self.bytes[at..at + 64].copy_from_slice(bytemuck::bytes_of(&p.view_proj.to_cols_array()));
            }
            gfx.queue.write_buffer(&self.light, 0, &self.bytes);
        }
        rebuilt
    }

    /// 每层画一遍（半透明的不在 `casters` 里），只画包围盒和这一层视野相交的。`bind` 绑定第 `i` 个 draw 并提交。
    /// 返回一共提交了几次 draw。
    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, casters: &[Caster], bind: impl Fn(&mut wgpu::RenderPass, usize)) -> u32 {
        let mut count = 0;
        for p in &self.passes {
            let map = if p.local { &self.local } else { &self.dir };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &map.layers[p.layer],
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.light_group, &[(p.slot as u64 * self.stride) as u32]);
            for c in casters.iter().filter(|c| intersects(p.view_proj, c.bounds)) {
                pass.set_pipeline(&self.pipelines[usize::from(c.skinned)]);
                bind(&mut pass, c.index);
                count += 1;
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ndc(m: Mat4, p: Vec3) -> Vec3 {
        let c = m * p.extend(1.0);
        c.truncate() / c.w
    }

    #[test]
    fn fit_frames_every_box_in_the_light_view() {
        let boxes = [(Vec3::new(-3.0, 0.0, -3.0), Vec3::new(3.0, 0.1, 3.0)), (Vec3::new(0.5, 1.0, 0.5), Vec3::new(1.0, 2.0, 1.0))];
        for direction in [Vec3::NEG_Y, Vec3::new(1.0, -1.0, 0.3), Vec3::X] {
            let (m, texel) = fit(direction, boxes, 1024).unwrap();
            for (min, max) in boxes {
                for p in [min, max, Vec3::new(min.x, max.y, min.z)] {
                    let q = ndc(m, p);
                    assert!(q.x.abs() <= 1.0 && q.y.abs() <= 1.0 && (0.0..=1.0).contains(&q.z), "{direction}: {p} → {q}");
                }
            }
            assert!(texel > 0.0 && texel < 0.02, "{texel}");
        }
        // 正上方照下来：高的点离光近（深度小）
        let (m, _) = fit(Vec3::NEG_Y, boxes, 1024).unwrap();
        assert!(ndc(m, Vec3::new(0.7, 2.0, 0.7)).z < ndc(m, Vec3::new(0.7, 0.0, 0.7)).z);
    }

    #[test]
    fn settings_are_clamped() {
        let d = Shadows::default();
        assert_eq!((d.size, d.cascades, d.distance, d.local_size), (2048, 1, 50.0, 512));
        let low = Shadows { size: 1, cascades: 0, distance: -3.0, local_size: 2 };
        assert_eq!((low.clamped_size(), low.clamped_cascades(), low.clamped_distance(), low.clamped_local_size()), (64, 1, 50.0, 64));
        let high = Shadows { size: 100_000, cascades: 9, distance: 300.0, local_size: 100_000 };
        assert_eq!((high.clamped_size(), high.clamped_cascades(), high.clamped_distance(), high.clamped_local_size()), (4096, 4, 300.0, 4096));
        assert_eq!(Shadows { distance: f32::NAN, ..d }.clamped_distance(), 50.0);
    }

    /// 正交相机每段的横截面都是整屏，分段没用：按一段画，框住所有物体。
    #[test]
    fn ortho_cameras_draw_a_single_cascade() {
        let casters = [Caster { index: 0, skinned: false, bounds: (Vec3::splat(-1.0), Vec3::splat(1.0)) }];
        let settings = Shadows { cascades: 3, ..Default::default() };
        let eye = Transform3d::from_xyz(0.0, 2.0, 10.0);
        let mut out = Vec::new();
        let view = CameraView { camera: Camera3d::default(), eye, aspect: 1.0 };
        directional(Vec3::NEG_Y, &view, &casters, settings, &mut out);
        assert_eq!(out.len(), 3, "透视相机照常分段");
        let view = CameraView { camera: Camera3d { ortho: Some(8.0), ..Camera3d::default() }, eye, aspect: 1.0 };
        directional(Vec3::NEG_Y, &view, &casters, settings, &mut out);
        assert_eq!(out.len(), 1, "正交相机只画一段");
        assert_eq!(out[0].2, f32::MAX, "着色器不分段");
    }

    #[test]
    fn nothing_to_fit() {
        assert!(fit(Vec3::NEG_Y, [], 1024).is_none());
        assert!(fit(Vec3::ZERO, [(Vec3::ZERO, Vec3::ONE)], 1024).is_none());
    }
}
