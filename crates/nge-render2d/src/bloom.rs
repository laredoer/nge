//! bloom：取亮部 → 一级级缩小 → 一级级放大叠加 → 和原画面合成（见 docs/render2d-more.md）。

use glam::{UVec2, Vec4};
use nge_core::{Gfx, wgpu};

/// 中间纹理的格式：叠加时不在 1 处截断。
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// 级数的上限。
pub(crate) const MAX_LEVELS: u32 = 8;

const SHADER: &str = r#"
struct G { params: vec4f, texel: vec2f, levels: f32, _pad: f32 }
@group(0) @binding(0) var<uniform> g: G;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var base: texture_2d<f32>;

struct Out {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> Out {
    let p = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: Out;
    out.clip = vec4f(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2f(p.x, 1.0 - p.y);
    return out;
}

fn at(uv: vec2f) -> vec3f {
    return textureSampleLevel(src, samp, uv, 0.0).rgb;
}

/// 中心 4 份、四个斜角各 1 份（`texel` 是源纹理一个像素在 uv 里多大）。
fn down(uv: vec2f) -> vec3f {
    let t = g.texel;
    let corners = at(uv + vec2f(-t.x, -t.y)) + at(uv + vec2f(t.x, -t.y)) + at(uv + vec2f(-t.x, t.y)) + at(uv + vec2f(t.x, t.y));
    return (at(uv) * 4.0 + corners) / 8.0;
}

/// 取亮部：亮度（Rec.709）超过阈值的部分按比例留下。
@fragment
fn fs_prefilter(in: Out) -> @location(0) vec4f {
    let c = down(in.uv);
    let m = dot(c, vec3f(0.2126, 0.7152, 0.0722));
    let w = max(m - g.params.x, 0.0) / max(m, 1e-4);
    return vec4f(c * w, 1.0);
}

@fragment
fn fs_down(in: Out) -> @location(0) vec4f {
    return vec4f(down(in.uv), 1.0);
}

/// 帐篷滤波放大，加法混合进大一级。
@fragment
fn fs_up(in: Out) -> @location(0) vec4f {
    let t = g.texel;
    let diagonal = at(in.uv + vec2f(-t.x, -t.y)) + at(in.uv + vec2f(t.x, -t.y)) + at(in.uv + vec2f(-t.x, t.y)) + at(in.uv + vec2f(t.x, t.y));
    let axis = at(in.uv + vec2f(-t.x * 2.0, 0.0)) + at(in.uv + vec2f(t.x * 2.0, 0.0)) + at(in.uv + vec2f(0.0, -t.y * 2.0)) + at(in.uv + vec2f(0.0, t.y * 2.0));
    return vec4f((diagonal * 2.0 + axis) / 12.0, 1.0);
}

/// 原画面加上亮部 × 强度。各级是加起来的，除以级数，级数只改晕开的范围、不改亮度。
@fragment
fn fs_composite(in: Out) -> @location(0) vec4f {
    let c = textureSampleLevel(base, samp, in.uv, 0.0);
    return vec4f(c.rgb + at(in.uv) * g.params.y / g.levels, c.a);
}
"#;

/// 和 WGSL 的 `G` 一样。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    params: [f32; 4],
    texel: [f32; 2],
    levels: f32,
    _pad: f32,
}

/// 半分辨率开始、一级级减半的尺寸，最多 `levels` 级，到 1×1 为止。
pub(crate) fn chain(size: UVec2, levels: u32) -> Vec<UVec2> {
    let mut out = Vec::new();
    let mut s = (size / 2).max(UVec2::ONE);
    for _ in 0..levels.clamp(1, MAX_LEVELS) {
        out.push(s);
        if s == UVec2::ONE {
            break;
        }
        s = (s / 2).max(UVec2::ONE);
    }
    out
}

/// 参数里的级数：不是有限数时用默认的 5。
pub(crate) fn levels(params: Vec4) -> u32 {
    if params.z.is_finite() { params.z.round().clamp(1.0, MAX_LEVELS as f32) as u32 } else { 5 }
}

pub(crate) struct Bloom {
    layout: wgpu::BindGroupLayout,
    prefilter: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    /// 合成的管线，按输出格式建。
    composite: Option<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    pipeline_layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    /// 每一级一张中间纹理，跟着画面大小走。
    mips: Vec<(UVec2, wgpu::TextureView)>,
}

impl Bloom {
    pub fn new(gfx: &Gfx) -> Self {
        let device = &gfx.device;
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: std::num::NonZeroU64::new(size_of::<Globals>() as u64) },
                    count: None,
                },
                texture(1),
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                texture(3),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("bloom"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("bloom"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let add = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent::REPLACE,
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bloom"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            prefilter: pipeline(device, gfx.pipeline_cache.as_ref(), &pipeline_layout, &module, "fs_prefilter", FORMAT, None),
            down: pipeline(device, gfx.pipeline_cache.as_ref(), &pipeline_layout, &module, "fs_down", FORMAT, None),
            up: pipeline(device, gfx.pipeline_cache.as_ref(), &pipeline_layout, &module, "fs_up", FORMAT, Some(add)),
            composite: None,
            layout,
            pipeline_layout,
            module,
            sampler,
            mips: Vec::new(),
        }
    }

    /// 从 `input` 画到 `output`（尺寸 `size`、格式 `format`）。`uniform` 每遍要一个新的 uniform（同一帧里不能复用）。
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        gfx: &Gfx,
        encoder: &mut wgpu::CommandEncoder,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
        size: UVec2,
        format: wgpu::TextureFormat,
        params: Vec4,
        uniform: &mut dyn FnMut(&[u8]) -> wgpu::Buffer,
    ) -> u32 {
        let sizes = chain(size, levels(params));
        self.ensure_mips(gfx, &sizes);
        self.prepare(gfx, format);
        let count = sizes.len() as f32;
        let mips: Vec<wgpu::TextureView> = self.mips[..sizes.len()].iter().map(|(_, v)| v.clone()).collect();
        let mut passes = 0;
        let mut pass = |encoder: &mut wgpu::CommandEncoder, pipeline: &wgpu::RenderPipeline, src: &wgpu::TextureView, src_size: UVec2, dst: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>| {
            let globals = Globals { params: params.to_array(), texel: (1.0 / src_size.as_vec2()).to_array(), levels: count, _pad: 0.0 };
            let buffer = uniform(bytemuck::bytes_of(&globals));
            let group = gfx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bloom"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(src) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(input) },
                ],
            });
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bloom"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: dst, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })],
                ..Default::default()
            });
            rp.set_pipeline(pipeline);
            rp.set_bind_group(0, &group, &[]);
            rp.draw(0..3, 0..1);
            passes += 1;
        };
        let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
        pass(encoder, &self.prefilter, input, size, &mips[0], clear);
        for i in 1..mips.len() {
            pass(encoder, &self.down, &mips[i - 1], sizes[i - 1], &mips[i], clear);
        }
        for i in (1..mips.len()).rev() {
            pass(encoder, &self.up, &mips[i], sizes[i], &mips[i - 1], wgpu::LoadOp::Load);
        }
        let composite = &self.composite.as_ref().expect("刚建好").1;
        pass(encoder, composite, &mips[0], sizes[0], output, clear);
        passes
    }

    /// 按输出格式建好合成的管线（预热时就调，用到时不卡）。
    pub fn prepare(&mut self, gfx: &Gfx, format: wgpu::TextureFormat) {
        if self.composite.as_ref().is_none_or(|(f, _)| *f != format) {
            self.composite = Some((format, pipeline(&gfx.device, gfx.pipeline_cache.as_ref(), &self.pipeline_layout, &self.module, "fs_composite", format, None)));
        }
    }

    fn ensure_mips(&mut self, gfx: &Gfx, sizes: &[UVec2]) {
        for (i, &s) in sizes.iter().enumerate() {
            if self.mips.get(i).is_some_and(|(have, _)| *have == s) {
                continue;
            }
            let view = gfx
                .device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("bloom mip"),
                    size: wgpu::Extent3d { width: s.x, height: s.y, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default());
            if i < self.mips.len() {
                self.mips[i] = (s, view);
            } else {
                self.mips.push((s, view));
            }
        }
    }
}

fn pipeline(device: &wgpu::Device, cache: Option<&wgpu::PipelineCache>, layout: &wgpu::PipelineLayout, module: &wgpu::ShaderModule, entry: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(entry),
        layout: Some(layout),
        vertex: wgpu::VertexState { module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(entry),
            targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chain_halves_from_half_size_down_to_one_pixel() {
        assert_eq!(chain(UVec2::new(64, 32), 5), [UVec2::new(32, 16), UVec2::new(16, 8), UVec2::new(8, 4), UVec2::new(4, 2), UVec2::new(2, 1)]);
        assert_eq!(chain(UVec2::new(8, 2), 8), [UVec2::new(4, 1), UVec2::new(2, 1), UVec2::ONE], "到 1×1 为止");
        assert_eq!(chain(UVec2::ONE, 3), [UVec2::ONE]);
        assert_eq!(chain(UVec2::splat(1024), 0).len(), 1, "至少一级");
        assert_eq!(chain(UVec2::splat(1 << 20), 99).len(), MAX_LEVELS as usize);
        assert_eq!(levels(Vec4::new(0.0, 0.0, 2.6, 0.0)), 3);
        assert_eq!(levels(Vec4::new(0.0, 0.0, f32::NAN, 0.0)), 5);
        assert_eq!(levels(Vec4::new(0.0, 0.0, -4.0, 0.0)), 1);
    }

    #[test]
    fn the_shader_compiles() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("解析");
        naga::valid::Validator::new(Default::default(), Default::default()).validate(&module).expect("校验");
    }
}
