//! 后处理：整帧画完后按 [`PostEffects`] 的顺序过一遍着色器（见 docs/effects2d.md）。

use std::collections::{HashMap, HashSet};

use bevy_ecs::prelude::*;
use nge_core::{Gfx, wgpu};
use nge_ecs::{PostProcess, Time};

use glam::Vec4;

use crate::bloom::Bloom;
use crate::effects::{PostEffect, PostEffects, PostLayer, Shaders, post_source, validate};
use crate::warm::Warm;

/// 什么都不做的一遍：有后处理但一个都用不了时，把画好的原样搬到屏幕上。
const IDENTITY: &str = "fn post(p: Post) -> vec4f {\n    return p.color;\n}";

/// 和 WGSL 的 `PostGlobals` 一样。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PostGlobals {
    params: [f32; 4],
    size: [f32; 2],
    time: f32,
    _pad: f32,
}

/// 整屏的后处理：只看 [`PostLayer::Screen`] 的。
#[derive(Default)]
pub(crate) struct PostRenderer {
    chain: PostChain,
}

impl PostProcess for PostRenderer {
    fn active(&self, world: &World) -> bool {
        world.get_resource::<PostEffects>().is_some_and(|p| p.layer(PostLayer::Screen).next().is_some())
    }

    fn warm(&mut self, world: &World, gfx: &Gfx) {
        self.chain.warm(gfx, world.resource::<Shaders>());
    }

    fn process(&mut self, world: &mut World, gfx: &Gfx, encoder: &mut wgpu::CommandEncoder, source: &wgpu::TextureView, target: &wgpu::TextureView) {
        let effects: Vec<PostEffect> = world.resource::<PostEffects>().layer(PostLayer::Screen).cloned().collect();
        let time = world.resource::<Time>().elapsed as f32;
        let timers = post_timers(world, effects.len());
        let count = self.chain.run(gfx, world.resource::<Shaders>(), encoder, &effects, source, target, time, &timers);
        world.resource_mut::<nge_ecs::RenderFrame>().count(count, 0);
    }
}

/// 每遍后处理一个 GPU 计时（至少一个：没有能用的效果时原样搬一遍）。bloom 里面的几遍不计时。
pub(crate) fn post_timers(world: &mut World, effects: usize) -> Vec<Option<nge_ecs::PassTimer>> {
    (0..effects.max(1)).map(|_| nge_ecs::pass_timer(world, "后处理")).collect()
}

/// 一串后处理：按顺序一遍遍画（两张中间纹理来回用），最后一遍画到目标上。整屏和背景层各用一个（uniform 不共用）。
#[derive(Default)]
pub(crate) struct PostChain {
    gpu: Option<Gpu>,
}

/// 一遍：着色器库里的（`None` 是原样搬），或者内置的 bloom。
enum Pass<'a> {
    Shader(Option<&'a str>, Vec4),
    Bloom(Vec4),
}

impl PostChain {
    /// 每帧调（这一帧用不用都调）：后台把库里的后处理编好，bloom 的管线也提前建好，用到时不卡。
    pub fn warm(&mut self, gfx: &Gfx, shaders: &Shaders) {
        let gpu = self.gpu.get_or_insert_with(|| Gpu::new(gfx));
        gpu.sync(gfx, shaders, gfx.format);
        gpu.bloom.get_or_insert_with(|| Bloom::new(gfx)).prepare(gfx, gpu.format);
    }

    /// 从 `source` 画到 `target`；用不了的跳过，一遍都用不了时原样搬。返回画了几遍。
    #[allow(clippy::too_many_arguments)]
    pub fn run(&mut self, gfx: &Gfx, shaders: &Shaders, encoder: &mut wgpu::CommandEncoder, effects: &[PostEffect], source: &wgpu::TextureView, target: &wgpu::TextureView, time: f32, timers: &[Option<nge_ecs::PassTimer>]) -> u32 {
        let gpu = self.gpu.get_or_insert_with(|| Gpu::new(gfx));
        // 管线、中间纹理都按目标的格式（平常就是 gfx.format）
        let format = gfx.target_format(target);
        gpu.sync(gfx, shaders, format);
        let mut passes: Vec<Pass> = Vec::new();
        for e in effects {
            if e.shader == Shaders::BLOOM {
                passes.push(Pass::Bloom(e.params));
            } else if gpu.prepare(gfx, shaders, &e.shader) {
                passes.push(Pass::Shader(Some(e.shader.as_str()), e.params));
            }
        }
        if passes.is_empty() {
            passes.push(Pass::Shader(None, Vec4::ZERO));
        }
        if passes.iter().any(|p| matches!(p, Pass::Shader(None, _))) && gpu.identity.is_none() {
            gpu.identity = Some(build(&gfx.device, gfx.pipeline_cache.as_ref(), &gpu.pipeline_layout, format, "post identity", &post_source(IDENTITY)));
        }
        let size = target.texture().size();
        let ping = gpu.ping_views(gfx, size, format);
        gpu.used = 0;
        let mut count = 0;
        let mut input = source.clone();
        for (i, pass) in passes.iter().enumerate() {
            let output = if i + 1 == passes.len() { target.clone() } else { ping[i % 2].clone() };
            match pass {
                Pass::Bloom(params) => {
                    let mut bloom = gpu.bloom.take().unwrap_or_else(|| Bloom::new(gfx));
                    let pixels = glam::UVec2::new(size.width, size.height);
                    count += bloom.draw(gfx, encoder, &input, &output, pixels, format, *params, &mut |bytes| gpu.uniform(gfx, bytes));
                    gpu.bloom = Some(bloom);
                }
                Pass::Shader(name, params) => {
                    let globals = PostGlobals { params: params.to_array(), size: [size.width as f32, size.height as f32], time, _pad: 0.0 };
                    let uniform = gpu.uniform(gfx, bytemuck::bytes_of(&globals));
                    let group = gfx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("post"),
                        layout: &gpu.layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&input) },
                            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&gpu.sampler) },
                        ],
                    });
                    let pipeline = match name {
                        Some(name) => gpu.pipelines[*name].as_ref().expect("只留了编好的"),
                        None => gpu.identity.as_ref().expect("刚编好"),
                    };
                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("post"),
                        timestamp_writes: timers.get(i).and_then(Option::as_ref).and_then(nge_ecs::PassTimer::writes),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &output,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                        })],
                        ..Default::default()
                    });
                    rp.set_pipeline(pipeline);
                    rp.set_bind_group(0, &group, &[]);
                    rp.draw(0..3, 0..1);
                    count += 1;
                }
            }
            input = output;
        }
        count
    }
}

struct Gpu {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    /// 名字 → 管线；不在库里、编不过的是 `None`（跳过）。
    pipelines: HashMap<String, Option<wgpu::RenderPipeline>>,
    identity: Option<wgpu::RenderPipeline>,
    generation: u64,
    format: wgpu::TextureFormat,
    /// 两张来回用的中间纹理，跟着渲染目标的大小、格式走。
    ping: Option<([wgpu::TextureView; 2], wgpu::Extent3d)>,
    /// uniform 池：同一帧里每一遍用一个新的（`used` 是这一帧用到第几个）。
    uniforms: Vec<wgpu::Buffer>,
    used: usize,
    bloom: Option<Bloom>,
    warned: HashSet<String>,
    /// 后台编好的（名字，管线），名字是 `None` 的是原样搬的那一遍。
    warm: Warm<(Option<String>, wgpu::RenderPipeline)>,
    /// 后台在为哪一代、哪种格式预热。
    warming: Option<(u64, wgpu::TextureFormat)>,
}

impl Gpu {
    fn new(gfx: &Gfx) -> Self {
        let device = &gfx.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(size_of::<PostGlobals>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            layout,
            pipeline_layout,
            sampler,
            pipelines: HashMap::new(),
            identity: None,
            generation: 0,
            format: gfx.format,
            ping: None,
            uniforms: Vec::new(),
            used: 0,
            bloom: None,
            warned: HashSet::new(),
            warm: Warm::new(),
            warming: None,
        }
    }

    /// 每帧调：着色器库换了一代、目标格式变了时编过的全丢掉、后台重新预热；收下后台编好的。
    fn sync(&mut self, gfx: &Gfx, shaders: &Shaders, format: wgpu::TextureFormat) {
        if format != self.format {
            self.format = format;
            self.identity = None;
            self.pipelines.clear();
        }
        if shaders.generation() != self.generation {
            self.generation = shaders.generation();
            self.pipelines.clear();
            self.warned.clear();
        }
        if self.warming != Some((self.generation, self.format)) {
            self.warming = Some((self.generation, self.format));
            self.start_warm(gfx, shaders);
        }
        for warmed in self.warm.drain() {
            self.take(warmed);
        }
    }

    /// 收下后台编好的；等不及已经当场编了的不换。
    fn take(&mut self, (name, pipeline): (Option<String>, wgpu::RenderPipeline)) {
        match name {
            Some(name) => {
                self.pipelines.entry(name).or_insert(Some(pipeline));
            }
            None => {
                self.identity.get_or_insert(pipeline);
            }
        }
    }

    /// 后台把原样搬的那一遍和库里每个后处理的管线都编一遍。编不过的（多半是精灵着色器）跳过，用到时再当场校验、提示。
    fn start_warm(&mut self, gfx: &Gfx, shaders: &Shaders) {
        let (device, cache, layout, format) = (gfx.device.clone(), gfx.pipeline_cache.clone(), self.pipeline_layout.clone(), self.format);
        let library = shaders.library();
        self.warm.start(move |send| {
            if !send((None, build(&device, cache.as_ref(), &layout, format, "post identity", &post_source(IDENTITY)))) {
                return;
            }
            for (name, code) in library {
                let source = post_source(&code);
                if validate(&source, &code).is_ok() && !send((Some(name.clone()), build(&device, cache.as_ref(), &layout, format, &name, &source))) {
                    return;
                }
            }
        });
    }

    /// 准备好这个后处理的管线，能用时返回 true；不在库里或编不过的提示一次。
    fn prepare(&mut self, gfx: &Gfx, shaders: &Shaders, name: &str) -> bool {
        if let Some(p) = self.pipelines.get(name) {
            return p.is_some();
        }
        let pipeline = match shaders.code(name) {
            None => {
                self.warn(name, || format!("后处理 {name} 不在着色器库里：跳过"));
                None
            }
            Some(code) => {
                let source = post_source(code);
                match validate(&source, code) {
                    Err(e) => {
                        self.warn(name, || format!("后处理 {name} 编不过（是不是精灵着色器？），跳过：{e}"));
                        None
                    }
                    Ok(()) => Some(build(&gfx.device, gfx.pipeline_cache.as_ref(), &self.pipeline_layout, self.format, name, &source)),
                }
            }
        };
        let ok = pipeline.is_some();
        self.pipelines.insert(name.to_owned(), pipeline);
        ok
    }

    /// 等后台这一批编完并收下。
    #[cfg(test)]
    fn settle(&mut self) {
        for warmed in self.warm.finish() {
            self.take(warmed);
        }
    }

    fn warn(&mut self, name: &str, message: impl FnOnce() -> String) {
        if self.warned.insert(name.to_owned()) {
            log::error!("{}", message());
        }
    }

    /// 这一帧下一个 uniform（32 字节），写上 `bytes`。
    fn uniform(&mut self, gfx: &Gfx, bytes: &[u8]) -> wgpu::Buffer {
        if self.used == self.uniforms.len() {
            self.uniforms.push(gfx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("post globals"),
                size: size_of::<PostGlobals>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let buffer = self.uniforms[self.used].clone();
        self.used += 1;
        gfx.queue.write_buffer(&buffer, 0, bytes);
        buffer
    }

    fn ping_views(&mut self, gfx: &Gfx, size: wgpu::Extent3d, format: wgpu::TextureFormat) -> [wgpu::TextureView; 2] {
        let size = wgpu::Extent3d { depth_or_array_layers: 1, ..size };
        if self.ping.as_ref().is_none_or(|(views, s)| *s != size || views[0].texture().format() != format) {
            let view = || {
                gfx.device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("post ping"),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            self.ping = Some(([view(), view()], size));
        }
        self.ping.as_ref().expect("刚创建").0.clone()
    }
}

/// 一遍后处理的管线。
fn build(device: &wgpu::Device, cache: Option<&wgpu::PipelineCache>, layout: &wgpu::PipelineLayout, format: wgpu::TextureFormat, label: &str, source: &str) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(source.into()) });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::test_gfx;

    fn library() -> Shaders {
        let mut shaders = Shaders::default();
        shaders.set(HashMap::from([
            ("dim".to_string(), "fn post(p: Post) -> vec4f {\n    return p.color * 0.5;\n}".to_string()),
            ("glow".to_string(), "fn effect(e: Effect) -> vec4f {\n    return e.color;\n}".to_string()),
        ]));
        shaders
    }

    fn warmed(gpu: &Gpu) -> Vec<&str> {
        let mut names: Vec<_> = gpu.pipelines.iter().filter(|(_, p)| p.is_some()).map(|(n, _)| n.as_str()).collect();
        names.sort();
        names
    }

    #[test]
    fn screen_post_only_turns_on_for_screen_effects() {
        let mut world = World::new();
        let renderer = PostRenderer::default();
        assert!(!renderer.active(&world));
        world.insert_resource(PostEffects(vec![PostEffect { shader: "grayscale".into(), layer: PostLayer::Back, ..Default::default() }]));
        assert!(!renderer.active(&world), "只有背景层的不开整屏后处理");
        world.resource_mut::<PostEffects>().0.push(PostEffect { shader: "vignette".into(), ..Default::default() });
        assert!(renderer.active(&world));
    }

    #[test]
    fn warms_the_identity_pass_and_every_post_shader() {
        let Some(gfx) = test_gfx(wgpu::TextureFormat::Rgba8UnormSrgb) else { return };
        let shaders = library();
        let mut gpu = Gpu::new(&gfx);
        gpu.sync(&gfx, &shaders, gfx.format);
        gpu.settle();
        assert!(gpu.identity.is_some());
        assert_eq!(warmed(&gpu), ["dim", "fxaa", "grayscale", "pixelate", "tonemap", "vignette"]);
        // 精灵着色器不预热，用到时照常提示、跳过
        assert!(!gpu.pipelines.contains_key("glow"));
        assert!(!gpu.prepare(&gfx, &shaders, "glow"));
        assert!(gpu.prepare(&gfx, &shaders, "dim"));
    }

    #[test]
    fn warms_again_when_the_target_format_changes() {
        let Some(srgb) = test_gfx(wgpu::TextureFormat::Rgba8UnormSrgb) else { return };
        let bgra = Gfx { screen: None, pipeline_cache: None, device: srgb.device.clone(), queue: srgb.queue.clone(), format: wgpu::TextureFormat::Bgra8Unorm, viewport: srgb.viewport };
        let shaders = library();
        let mut gpu = Gpu::new(&srgb);
        gpu.sync(&srgb, &shaders, srgb.format);
        gpu.settle();
        gpu.sync(&bgra, &shaders, bgra.format);
        gpu.settle();
        assert!(gpu.identity.is_some());
        assert_eq!(warmed(&gpu), ["dim", "fxaa", "grayscale", "pixelate", "tonemap", "vignette"]);
    }

    #[test]
    fn only_the_screen_texture_is_drawn_through_its_srgb_view() {
        let Some(mut gfx) = test_gfx(wgpu::TextureFormat::Rgba8UnormSrgb) else { return };
        let texture = |gfx: &Gfx| {
            gfx.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
        };
        let own = texture(&gfx).create_view(&Default::default());
        assert_eq!(gfx.target_format(&own), wgpu::TextureFormat::Rgba8Unorm, "游戏自己的纹理按它自己的格式");
        let screen = texture(&gfx);
        gfx.screen = Some(screen.clone());
        assert_eq!(gfx.target_format(&screen.create_view(&Default::default())), gfx.format, "画面按 sRGB view 的格式");
        assert_eq!(gfx.target_format(&own), wgpu::TextureFormat::Rgba8Unorm);
    }
}
