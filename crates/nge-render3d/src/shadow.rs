//! 阴影：方向光（一张图框住所有物体，或按相机视锥分段的级联）、几盏聚光灯、点光源的六面。
//! 每层画一遍只写深度，主 pass 里按 `filter` 采样：3×3 PCF、泊松盘、PCSS（见 docs/3d-more.md、docs/render3d-more.md、docs/render3d-quality.md）。

use std::num::NonZeroU64;

use bevy_ecs::prelude::Resource;
use glam::camera::rh::proj::directx;
use glam::{Mat4, Vec3};
use nge_core::{Gfx, wgpu};

use crate::camera::{Camera3d, Transform3d};
use crate::light::Globals;
use crate::shadow_soft::{Candidate, MAX_LOCAL_LAYERS, array_layers, depth_range, fit_local_size, pick};
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
    /// 影子边缘怎么过滤。
    pub filter: ShadowFilter,
    /// 泊松盘的半径（阴影图像素），限制在 [0.5, 16]。
    pub softness: f32,
    /// PCSS：太阳的角直径（度），越大半影越宽，限制在 [0, 10]。
    pub sun_size: f32,
    /// 比较前从片元深度里减掉多少（阴影图的 0–1 深度），治阴影痘；太大影子会和物体脱开（漏光）。
    pub bias: f32,
    /// 沿法线往外推几个阴影图像素，限制在 [0, 8]。
    pub normal_bias: f32,
    /// 聚光灯、点光源最多几盏投影子（按 [`ShadowPriority`](crate::ShadowPriority) 挑），限制在 [0, 4]；多的照常照明。
    pub max_lights: u32,
    /// 相邻级联交界前多长一段（占这一段的比例）里混合，限制在 [0, 0.5]；最后一段在末尾淡出。
    pub blend: f32,
    /// 按级联给画面染色：红、绿、蓝、黄。
    pub debug_cascades: bool,
}

/// 影子边缘的过滤。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShadowFilter {
    /// 3×3 PCF。
    #[default]
    Pcf,
    /// 16 点的采样盘，每个像素转一个角度。
    Poisson,
    /// 方向光按遮挡距离算半影宽（PCSS）；聚光灯、点光源按 `Poisson`。
    Pcss,
}

impl Default for Shadows {
    fn default() -> Self {
        Self {
            size: 2048,
            cascades: 1,
            distance: 50.0,
            local_size: 512,
            filter: ShadowFilter::Pcf,
            softness: 1.5,
            sun_size: 1.0,
            bias: 0.0,
            normal_bias: NORMAL_OFFSET,
            max_lights: 2,
            blend: 0.1,
            debug_cascades: false,
        }
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

    /// 算坏了的数（NaN、无穷）按默认值，其余限制在范围里。
    fn finite(v: f32, default: f32, lo: f32, hi: f32) -> f32 {
        if v.is_finite() { v.clamp(lo, hi) } else { default }
    }

    pub(crate) fn clamped_softness(&self) -> f32 {
        Self::finite(self.softness, 1.5, 0.5, MAX_PENUMBRA)
    }

    pub(crate) fn clamped_normal_bias(&self) -> f32 {
        Self::finite(self.normal_bias, NORMAL_OFFSET, 0.0, 8.0)
    }

    pub(crate) fn clamped_bias(&self) -> f32 {
        Self::finite(self.bias, 0.0, -0.1, 0.1)
    }

    pub(crate) fn clamped_blend(&self) -> f32 {
        Self::finite(self.blend, 0.1, 0.0, 0.5)
    }

    pub(crate) fn clamped_max_lights(&self) -> usize {
        (self.max_lights as usize).min(crate::shadow_soft::MAX_SHADOWED_LOCAL)
    }

    /// PCSS：太阳角直径的 tan。
    pub(crate) fn sun_tan(&self) -> f32 {
        Self::finite(self.sun_size, 1.0, 0.0, 10.0).to_radians().tan()
    }
}

pub(crate) const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// 光矩阵的槽位：方向光 4 段，接着聚光灯、点光源的各层。
const SLOTS: usize = MAX_CASCADES + MAX_LOCAL_LAYERS;
/// 法线方向往外推几个像素（默认），斜面上不长阴影痘。
const NORMAL_OFFSET: f32 = 1.5;
/// PCSS 半影、泊松盘半径最大多少像素。
pub(crate) const MAX_PENUMBRA: f32 = 16.0;

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
    /// 聚光灯和点光源：`local_size × local_size × 层数`，层数只增不减（见 `array_layers`）。
    pub local: ShadowMap,
    passes: Vec<Pass>,
    /// 这一帧方向光每段的结果，每帧清空复用。
    cascades: Vec<(Mat4, f32, f32)>,
    /// 上一帧和这一帧投影子的灯（挑灯的滞后）。
    previous: Vec<u64>,
    picked: Vec<u64>,
    over_budget: bool,
    /// GL 后端：方向光的阴影图也避开 1 层和 6 的倍数层。
    gl: bool,
}

/// 聚光灯、点光源阴影图最少几层（避开正好 6 层）。
const LOCAL_MIN_LAYERS: usize = 7;

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
            dir: ShadowMap::new(gfx, "shadow map", 1, array_layers(1, 0, 1) as u32),
            local: ShadowMap::new(gfx, "local shadow map", 1, LOCAL_MIN_LAYERS as u32),
            passes: Vec::new(),
            cascades: Vec::new(),
            previous: Vec::new(),
            picked: Vec::new(),
            over_budget: false,
            gl: gfx.device.adapter_info().backend == wgpu::Backend::Gl,
        }
    }

    /// 算出这一帧每层的光矩阵写进 `globals`，记下要画的层；拟合不了（没有东西、灯的参数坏了）的灯关掉阴影。
    /// 聚光灯、点光源从 `candidates` 里按优先级挑最多 `max_lights` 盏。返回阴影图有没有重建（主 pass 的 bind group 要跟着换）。
    pub fn prepare(&mut self, gfx: &Gfx, globals: &mut Globals, view: &CameraView, casters: &[Caster], candidates: &[Candidate], settings: Shadows) -> bool {
        self.passes.clear();
        let mut rebuilt = false;
        let forward = view.eye.rotation * Vec3::NEG_Z;
        globals.view_forward = forward.extend(-forward.dot(view.eye.translation)).into();
        globals.casters[0] = settings.filter as u32;
        globals.casters[1] = u32::from(settings.debug_cascades);
        globals.soft = [settings.clamped_softness(), settings.clamped_blend(), settings.clamped_bias(), MAX_PENUMBRA];
        let normal_bias = settings.clamped_normal_bias();

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
            let layers = if self.gl { array_layers(cascades.len(), 0, 1) } else { cascades.len() };
            rebuilt |= self.dir.resize(gfx, "shadow map", size, layers as u32);
            for (i, (m, texel, end)) in cascades.iter().enumerate() {
                globals.cascades[i] = m.to_cols_array_2d();
                globals.splits[i] = *end;
                globals.cascade_offset[i] = texel * normal_bias;
                // PCSS：深度差 × 深度范围 = 世界里的距离，× tan(太阳角直径) = 半影宽，再换成像素
                globals.pcss[i] = depth_range(*m) * settings.sun_tan() / texel;
                self.passes.push(Pass { local: false, layer: i, slot: i, view_proj: *m });
            }
            globals.casters[2] = cascades.len() as u32;
            globals.shadow[0] = 1.0 / size as f32;
        }
        self.cascades = cascades;

        // 显存按最多能用到的层数算：超了边长减半
        let wanted = settings.clamped_local_size();
        let local_size = fit_local_size(wanted, array_layers(MAX_LOCAL_LAYERS, 0, LOCAL_MIN_LAYERS));
        if local_size != wanted && !std::mem::replace(&mut self.over_budget, true) {
            log::warn!("聚光灯、点光源阴影图 {wanted} 边长超过显存预算，按 {local_size}");
        }
        let mut layers = 0;
        self.picked.clear();
        if !casters.is_empty() {
            let picked = pick(candidates, settings.clamped_max_lights(), &self.previous);
            self.picked.extend(picked.iter().map(|&(c, _)| candidates[c].id));
            for (c, first) in picked {
                let light = candidates[c];
                if light.point {
                    let p = globals.points[light.index];
                    let Some(faces) = point_matrices(Vec3::from_slice(&p.position_range[..3]), p.position_range[3]) else { continue };
                    for (i, m) in faces.into_iter().enumerate() {
                        self.local_layer(globals, first + i, m);
                    }
                    globals.point_layers[light.index] = first as u32 + 1;
                    layers = layers.max(first + 6);
                } else {
                    let s = globals.spots[light.index];
                    let outer = s.direction[3].clamp(-1.0, 1.0).acos();
                    let Some(m) = spot_matrix(Vec3::from_slice(&s.position_range[..3]), Vec3::from_slice(&s.direction[..3]), outer, s.position_range[3]) else { continue };
                    self.local_layer(globals, first, m);
                    // 透视的图一个像素在世界里多大 = 到灯的（沿视线的）距离 × 2 tan(半张角) / 边长
                    globals.spot_offset[light.index] = 2.0 * outer.tan() / local_size as f32 * normal_bias;
                    globals.spot_layers[light.index] = first as u32 + 1;
                    layers = layers.max(first + 1);
                }
            }
        }
        std::mem::swap(&mut self.previous, &mut self.picked);
        if layers > 0 {
            let layers = array_layers(layers, self.local.layers.len(), LOCAL_MIN_LAYERS);
            rebuilt |= self.local.resize(gfx, "local shadow map", local_size, layers as u32);
            globals.shadow[1] = 1.0 / local_size as f32;
            globals.shadow[3] = 2.0 / local_size as f32 * normal_bias;
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

    /// 局部光阴影图的第 `layer` 层用矩阵 `m`。
    fn local_layer(&mut self, globals: &mut Globals, layer: usize, m: Mat4) {
        globals.locals[layer] = m.to_cols_array_2d();
        self.passes.push(Pass { local: true, layer, slot: MAX_CASCADES + layer, view_proj: m });
    }

    /// 这一帧画几层。
    pub fn pass_count(&self) -> usize {
        self.passes.len()
    }

    /// 每层画一遍（半透明的不在 `casters` 里），只画包围盒和这一层视野相交的。`bind` 绑定第 `i` 个 draw 并提交。
    /// `timers` 是每层的 GPU 计时（见 [`Self::pass_count`]）。返回一共提交了几次 draw。
    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, casters: &[Caster], timers: &[Option<nge_ecs::PassTimer>], bind: impl Fn(&mut wgpu::RenderPass, usize)) -> u32 {
        let mut count = 0;
        for (i, p) in self.passes.iter().enumerate() {
            let map = if p.local { &self.local } else { &self.dir };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                timestamp_writes: timers.get(i).and_then(Option::as_ref).and_then(nge_ecs::PassTimer::writes),
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
        let low = Shadows { size: 1, cascades: 0, distance: -3.0, local_size: 2, ..d };
        assert_eq!((low.clamped_size(), low.clamped_cascades(), low.clamped_distance(), low.clamped_local_size()), (64, 1, 50.0, 64));
        let high = Shadows { size: 100_000, cascades: 9, distance: 300.0, local_size: 100_000, ..d };
        assert_eq!((high.clamped_size(), high.clamped_cascades(), high.clamped_distance(), high.clamped_local_size()), (4096, 4, 300.0, 4096));
        assert_eq!(Shadows { distance: f32::NAN, ..d }.clamped_distance(), 50.0);
        assert_eq!((d.filter, d.softness, d.normal_bias, d.max_lights, d.blend, d.debug_cascades), (ShadowFilter::Pcf, 1.5, 1.5, 2, 0.1, false));
        let wild = Shadows { softness: 99.0, normal_bias: -1.0, bias: 3.0, blend: 2.0, max_lights: 9, sun_size: 90.0, ..d };
        assert_eq!((wild.clamped_softness(), wild.clamped_normal_bias(), wild.clamped_bias(), wild.clamped_blend(), wild.clamped_max_lights()), (16.0, 0.0, 0.1, 0.5, 4));
        assert!((wild.sun_tan() - 10f32.to_radians().tan()).abs() < 1e-6);
        let nan = Shadows { softness: f32::NAN, normal_bias: f32::INFINITY, bias: f32::NAN, blend: f32::NAN, sun_size: f32::NAN, ..d };
        assert_eq!((nan.clamped_softness(), nan.clamped_normal_bias(), nan.clamped_bias(), nan.clamped_blend()), (1.5, 1.5, 0.0, 0.1), "算坏了的按默认");
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
