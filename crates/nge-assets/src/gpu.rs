//! GPU 上的贴图：每张图只上传一次，2D 和 3D 的渲染器都从这里拿纹理视图。

use std::collections::{HashMap, HashSet};

use bevy_ecs::prelude::Resource;
use glam::UVec2;
use nge_core::{Gfx, wgpu};

use crate::image::{Image, ImageId, mip_levels};
use crate::ktx2::{BlockFormat, Compressed, level_size};
use crate::server::Assets;

#[derive(Resource, Default)]
pub struct GpuImages {
    /// 平常的视图、按线性数据读的视图（sRGB 的贴图另开一个不带 sRGB 的；本来就不是 sRGB 的两个一样），和上传时的方式。
    views: Vec<Option<(wgpu::TextureView, wgpu::TextureView, Upload)>>,
    /// 按贴图开关 mipmap（优先于 `Image::mipmaps`）。
    mipmaps: HashMap<u32, bool>,
    /// 存的是数据不是颜色的贴图（法线图）：mip 按存的值平均，不按 sRGB 解码。
    linear: HashSet<u32>,
    /// 有贴图重传过就加一：渲染器看到变了就丢掉缓存的绑定组。
    generation: u64,
    mipmapper: Option<Mipmapper>,
}

/// 一张贴图按什么方式上传的：生成 mip 链，按数据（线性）生成。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Upload {
    mips: bool,
    linear: bool,
}

impl GpuImages {
    /// 上传新加载好的贴图，开关变了的重传。渲染器每帧画之前调用，没有要传的时只是一次遍历。
    pub fn sync(&mut self, gfx: &Gfx, images: &Assets<Image>) {
        if self.views.len() < images.len() {
            self.views.resize_with(images.len(), || None);
        }
        for (id, image) in images.iter() {
            let index = id.index();
            let wanted = self.mipmaps.get(&index).copied().unwrap_or(image.mipmaps) && image.compressed.is_none() && image.size.max_element() > 1;
            let how = Upload { mips: wanted, linear: wanted && self.linear.contains(&index) };
            let slot = &mut self.views[index as usize];
            match slot {
                Some((.., before)) if *before == how => continue,
                Some(_) => self.generation += 1,
                None => {}
            }
            let view = upload(gfx, image, how, &mut self.mipmapper);
            let linear = linear_view(&view);
            *slot = Some((view, linear, how));
        }
    }

    /// 这张贴图要不要生成 mip 链；`None` 是按 `Image::mipmaps`。贴图加载好之前设也行；
    /// 上传以后改了，下一次 [`Self::sync`] 重传（代数加一）。
    pub fn set_mipmaps(&mut self, id: ImageId, on: Option<bool>) {
        match on {
            Some(on) => self.mipmaps.insert(id.index(), on),
            None => self.mipmaps.remove(&id.index()),
        };
    }

    /// [`Self::set_mipmaps`] 设过的值。
    pub fn mipmaps(&self, id: ImageId) -> Option<bool> {
        self.mipmaps.get(&id.index()).copied()
    }

    /// 标成存数据的贴图（法线图）：生成 mip 时按存的值平均。上传以后改了也重传。
    pub fn set_linear(&mut self, id: ImageId, on: bool) {
        if on {
            self.linear.insert(id.index());
        } else {
            self.linear.remove(&id.index());
        }
    }

    /// [`Self::set_linear`] 标过。
    pub fn linear(&self, id: ImageId) -> bool {
        self.linear.contains(&id.index())
    }

    /// 有贴图重传过就变。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 还没加载完（或者没上传）的贴图返回 None，调用方用 [`ImageId::WHITE`] 代替，并且不要缓存。
    pub fn view(&self, id: ImageId) -> Option<&wgpu::TextureView> {
        self.views.get(id.index() as usize).and_then(Option::as_ref).map(|(v, ..)| v)
    }

    /// 按线性数据读的视图（法线图之类）：sRGB 的贴图取样时不做 sRGB → 线性的转换，拿到存着的原值。
    pub fn linear_view(&self, id: ImageId) -> Option<&wgpu::TextureView> {
        self.views.get(id.index() as usize).and_then(Option::as_ref).map(|(_, v, _)| v)
    }
}

fn linear_view(view: &wgpu::TextureView) -> wgpu::TextureView {
    let format = view.texture().format();
    if !format.is_srgb() {
        return view.clone();
    }
    view.texture().create_view(&wgpu::TextureViewDescriptor { format: Some(format.remove_srgb_suffix()), ..Default::default() })
}

/// 块压缩格式对应的 wgpu 格式和需要的设备特性。
fn wgpu_format(format: BlockFormat, srgb: bool) -> (wgpu::TextureFormat, wgpu::Features) {
    use wgpu::{AstcBlock, AstcChannel, Features, TextureFormat as F};
    let astc = |block| (F::Astc { block, channel: if srgb { AstcChannel::UnormSrgb } else { AstcChannel::Unorm } }, Features::TEXTURE_COMPRESSION_ASTC);
    match format {
        BlockFormat::Bc1 => (if srgb { F::Bc1RgbaUnormSrgb } else { F::Bc1RgbaUnorm }, Features::TEXTURE_COMPRESSION_BC),
        BlockFormat::Bc3 => (if srgb { F::Bc3RgbaUnormSrgb } else { F::Bc3RgbaUnorm }, Features::TEXTURE_COMPRESSION_BC),
        BlockFormat::Bc7 => (if srgb { F::Bc7RgbaUnormSrgb } else { F::Bc7RgbaUnorm }, Features::TEXTURE_COMPRESSION_BC),
        BlockFormat::Etc2Rgba8 => (if srgb { F::Etc2Rgba8UnormSrgb } else { F::Etc2Rgba8Unorm }, Features::TEXTURE_COMPRESSION_ETC2),
        BlockFormat::Astc4x4 => astc(AstcBlock::B4x4),
        BlockFormat::Astc6x6 => astc(AstcBlock::B6x6),
        BlockFormat::Astc8x8 => astc(AstcBlock::B8x8),
    }
}

/// 这台设备能不能直接用这张压缩贴图：有对应特性、宽高是块的整数倍（wgpu 的要求）。
pub(crate) fn direct_format(c: &Compressed, size: UVec2, features: wgpu::Features) -> Option<wgpu::TextureFormat> {
    let (format, need) = wgpu_format(c.format, c.srgb);
    (features.contains(need) && size % c.format.block() == UVec2::ZERO).then_some(format)
}

fn upload(gfx: &Gfx, image: &Image, how: Upload, mipmapper: &mut Option<Mipmapper>) -> wgpu::TextureView {
    let Some(c) = &image.compressed else {
        if how.mips {
            return upload_mipmapped(gfx, image.size, &image.rgba, how.linear, mipmapper);
        }
        return upload_rgba(gfx, image.size, &image.rgba, true);
    };
    if let Some(format) = direct_format(c, image.size, gfx.device.features()) {
        return upload_compressed(gfx, image.size, c, format);
    }
    // 设备不支持这种格式：在 CPU 上把每一级解成 RGBA8，画出来一样（mipmap 也在），只是没省显存
    log::info!("设备不支持 {:?}（或尺寸 {} 不是块的整数倍），贴图解成 RGBA8 上传", c.format, image.size);
    match (0..c.levels.len()).map(|level| c.decode_level(image.size, level)).collect::<Result<Vec<_>, _>>() {
        Ok(levels) => upload_rgba_levels(gfx, image.size, &levels, c.srgb),
        Err(e) => {
            log::error!("{e}，用白色代替");
            upload_rgba(gfx, UVec2::ONE, &[255; 4], true)
        }
    }
}

fn texture(gfx: &Gfx, size: UVec2, levels: u32, format: wgpu::TextureFormat, extra: wgpu::TextureUsages) -> wgpu::Texture {
    let linear = [format.remove_srgb_suffix()];
    gfx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("image"),
        size: wgpu::Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC | extra,
        // sRGB 的贴图也能按线性数据读（法线图）
        view_formats: if format.is_srgb() { &linear } else { &[] },
    })
}

fn upload_rgba(gfx: &Gfx, size: UVec2, rgba: &[u8], srgb: bool) -> wgpu::TextureView {
    upload_rgba_levels(gfx, size, &[rgba], srgb)
}

fn upload_rgba_levels(gfx: &Gfx, size: UVec2, levels: &[impl AsRef<[u8]>], srgb: bool) -> wgpu::TextureView {
    let format = if srgb { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
    let texture = texture(gfx, size, levels.len() as u32, format, wgpu::TextureUsages::empty());
    for (level, rgba) in levels.iter().enumerate() {
        let s = level_size(size, level);
        gfx.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            rgba.as_ref(),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * s.x), rows_per_image: Some(s.y) },
            wgpu::Extent3d { width: s.x, height: s.y, depth_or_array_layers: 1 },
        );
    }
    texture.create_view(&Default::default())
}

/// 写第 0 级，再在 GPU 上一级一级往下生成，直到 1×1。颜色是 sRGB 贴图；存数据的（`linear`）不带 sRGB，按存的值平均。
fn upload_mipmapped(gfx: &Gfx, size: UVec2, rgba: &[u8], linear: bool, mipmapper: &mut Option<Mipmapper>) -> wgpu::TextureView {
    let format = if linear { wgpu::TextureFormat::Rgba8Unorm } else { wgpu::TextureFormat::Rgba8UnormSrgb };
    let levels = mip_levels(size);
    let texture = texture(gfx, size, levels, format, wgpu::TextureUsages::RENDER_ATTACHMENT);
    gfx.queue.write_texture(
        texture.as_image_copy(),
        rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * size.x), rows_per_image: Some(size.y) },
        wgpu::Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
    );
    mipmapper.get_or_insert_with(|| Mipmapper::new(&gfx.device)).generate(gfx, &texture);
    texture.create_view(&Default::default())
}

/// 每级取上一级的 2×2：越界的取边上的（奇数边丢掉最后一列 / 行，边长是 1 的重复用）。
/// sRGB 纹理 `textureLoad` 读出来已经是线性的，写回 sRGB 视图时再编码，所以平均在线性空间里算；颜色按 alpha 加权。
const MIP_SHADER: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let last = vec2i(textureDimensions(src)) - 1;
    let base = vec2i(pos.xy) * 2;
    var weighted = vec3f(0.0);
    var plain = vec3f(0.0);
    var alpha = 0.0;
    for (var i = 0; i < 4; i++) {
        let c = textureLoad(src, min(base + vec2i(i & 1, i >> 1u), last), 0);
        weighted += c.rgb * c.a;
        plain += c.rgb;
        alpha += c.a;
    }
    if alpha > 0.0 {
        return vec4f(weighted / alpha, alpha / 4.0);
    }
    return vec4f(plain / 4.0, 0.0);
}
"#;

/// 生成 mip 链的管线，按目标格式缓存。
struct Mipmapper {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl Mipmapper {
    fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mipmap"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("mipmap"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("mipmap"), source: wgpu::ShaderSource::Wgsl(MIP_SHADER.into()) });
        Self { layout, pipeline_layout, module, pipelines: HashMap::new() }
    }

    fn pipeline(&mut self, gfx: &Gfx, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        let Self { pipeline_layout, module, .. } = self;
        self.pipelines.entry(format).or_insert_with(|| {
            gfx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mipmap"),
                layout: Some(pipeline_layout),
                vertex: wgpu::VertexState { module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState { module, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(format.into())] }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: gfx.pipeline_cache.as_ref(),
            })
        })
    }

    fn generate(&mut self, gfx: &Gfx, texture: &wgpu::Texture) {
        let format = texture.format();
        let level_view = |level: u32| texture.create_view(&wgpu::TextureViewDescriptor { base_mip_level: level, mip_level_count: Some(1), ..Default::default() });
        let mut encoder = gfx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("mipmap") });
        for level in 1..texture.mip_level_count() {
            let source = level_view(level - 1);
            let group = gfx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mipmap"),
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&source) }],
            });
            let target = level_view(level);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mipmap"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(self.pipeline(gfx, format));
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        gfx.queue.submit([encoder.finish()]);
    }
}

fn upload_compressed(gfx: &Gfx, size: UVec2, c: &Compressed, format: wgpu::TextureFormat) -> wgpu::TextureView {
    let texture = texture(gfx, size, c.levels.len() as u32, format, wgpu::TextureUsages::empty());
    let block = c.format.block();
    for (level, data) in c.levels.iter().enumerate() {
        // 小于一块的级按一整块拷（物理尺寸）
        let blocks = c.format.blocks(level_size(size, level));
        let physical = blocks * block;
        gfx.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(blocks.x * c.format.block_bytes() as u32), rows_per_image: Some(blocks.y) },
            wgpu::Extent3d { width: physical.x, height: physical.y, depth_or_array_layers: 1 },
        );
    }
    texture.create_view(&Default::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compressed(format: BlockFormat, srgb: bool) -> Compressed {
        Compressed { format, srgb, levels: vec![] }
    }

    #[test]
    fn picks_the_format_only_when_the_device_has_it_and_the_size_is_aligned() {
        use wgpu::{Features, TextureFormat as F};
        let all = Features::TEXTURE_COMPRESSION_BC | Features::TEXTURE_COMPRESSION_ETC2 | Features::TEXTURE_COMPRESSION_ASTC;
        let bc3 = compressed(BlockFormat::Bc3, true);
        assert_eq!(direct_format(&bc3, UVec2::new(8, 4), all), Some(F::Bc3RgbaUnormSrgb));
        assert_eq!(direct_format(&bc3, UVec2::new(8, 4), Features::TEXTURE_COMPRESSION_ASTC), None, "没有 BC 特性");
        assert_eq!(direct_format(&bc3, UVec2::new(8, 6), all), None, "高不是 4 的倍数");
        assert_eq!(direct_format(&bc3, UVec2::new(6, 4), all), None, "宽不是 4 的倍数");
        let astc = compressed(BlockFormat::Astc6x6, false);
        assert_eq!(direct_format(&astc, UVec2::new(12, 6), all), Some(F::Astc { block: wgpu::AstcBlock::B6x6, channel: wgpu::AstcChannel::Unorm }));
        assert_eq!(direct_format(&astc, UVec2::new(8, 8), all), None, "6×6 的块，8 不对齐");
        assert_eq!(direct_format(&compressed(BlockFormat::Etc2Rgba8, true), UVec2::splat(4), Features::TEXTURE_COMPRESSION_ETC2), Some(F::Etc2Rgba8UnormSrgb));
        // 每种格式都映射到块大小一样的 wgpu 格式
        for f in [BlockFormat::Bc1, BlockFormat::Bc3, BlockFormat::Bc7, BlockFormat::Etc2Rgba8, BlockFormat::Astc4x4, BlockFormat::Astc6x6, BlockFormat::Astc8x8] {
            for srgb in [false, true] {
                let (w, _) = wgpu_format(f, srgb);
                assert_eq!(w.block_dimensions(), (f.block().x, f.block().y), "{f:?}");
                assert_eq!(w.block_copy_size(None), Some(f.block_bytes() as u32), "{f:?}");
                assert_eq!(w.is_srgb(), srgb, "{f:?}");
            }
        }
    }

    #[test]
    fn the_mip_shader_translates_to_webgl2_glsl() {
        use naga::back::glsl;
        let module = naga::front::wgsl::parse_str(MIP_SHADER).expect("WGSL 解析");
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).expect("校验");
        let options = glsl::Options { version: glsl::Version::Embedded { version: 300, is_webgl: true }, ..Default::default() };
        for (stage, entry) in [(naga::ShaderStage::Vertex, "vs"), (naga::ShaderStage::Fragment, "fs")] {
            let pipeline = glsl::PipelineOptions { shader_stage: stage, entry_point: entry.into(), multiview: None };
            let mut out = String::new();
            let mut writer = glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default()).expect(entry);
            writer.write().expect(entry);
            assert!(out.starts_with("#version 300 es"), "{out}");
        }
    }
}
