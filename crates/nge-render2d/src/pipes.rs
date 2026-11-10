//! 精灵管线：按（着色器，遮罩模式）编号、后台预热、没热好的按需编译、着色器库改了就丢掉重来（见 docs/effects2d.md）。

use std::collections::{HashMap, HashSet};

use nge_core::{Gfx, wgpu};

use crate::batch::Instance;
use crate::effects::{Masking, Shaders, sprite_source, validate};
use crate::warm::Warm;

/// 精灵管线的画法：普通、只写遮罩、只画在遮罩里 / 外。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Normal,
    Write,
    Inside,
    Outside,
}

impl Mode {
    const ALL: [Mode; 4] = [Mode::Normal, Mode::Write, Mode::Inside, Mode::Outside];

    pub fn of(masking: Option<&Masking>) -> Self {
        match masking {
            None => Mode::Normal,
            Some(Masking::Write) => Mode::Write,
            Some(Masking::Inside) => Mode::Inside,
            Some(Masking::Outside) => Mode::Outside,
        }
    }

    /// 模板的参考值是 1：遮罩写 1，里面的要等于 1，外面的要不等于 1。
    fn stencil(self) -> wgpu::StencilState {
        let (compare, pass_op) = match self {
            Mode::Normal => (wgpu::CompareFunction::Always, wgpu::StencilOperation::Keep),
            Mode::Write => (wgpu::CompareFunction::Always, wgpu::StencilOperation::Replace),
            Mode::Inside => (wgpu::CompareFunction::Equal, wgpu::StencilOperation::Keep),
            Mode::Outside => (wgpu::CompareFunction::NotEqual, wgpu::StencilOperation::Keep),
        };
        // 镜像的精灵是背面，两面一样
        let face = wgpu::StencilFaceState { compare, fail_op: wgpu::StencilOperation::Keep, depth_fail_op: wgpu::StencilOperation::Keep, pass_op };
        wgpu::StencilState { front: face, back: face, read_mask: 0xff, write_mask: 0xff }
    }
}

pub(crate) const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Stencil8;

/// 管线号：着色器编号 × 8 + 和 3D 混排 × 4 + 画法（所以 `pipe % 4` 是画法）。
pub(crate) fn pipe(module: u32, mode: Mode, depth: bool) -> u32 {
    module * 8 + u32::from(depth) * 4 + mode as u32
}

/// 后台编好的精灵着色器和管线。
enum Warmed {
    Module(String, wgpu::ShaderModule),
    /// 着色器名（`None` 是原样画的）、画法、和 3D 混排。
    Pipeline(Option<String>, Mode, bool, wgpu::RenderPipeline),
}

pub(crate) struct Pipes {
    pub globals_layout: wgpu::BindGroupLayout,
    pub texture_layout: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    /// 编过的着色器，0 是原样画的。
    modules: Vec<wgpu::ShaderModule>,
    /// 名字 → 着色器编号；不在库里、编不过的记成 0。
    names: HashMap<String, u32>,
    generation: u64,
    pipelines: HashMap<u32, wgpu::RenderPipeline>,
    /// 这一代里提示过的名字，免得每帧都报。
    warned: HashSet<String>,
    warm: Warm<Warmed>,
    /// 后台在为哪一代预热。
    warming: Option<u64>,
}

impl Pipes {
    pub fn new(gfx: &Gfx, globals_size: u64) -> Self {
        let device = &gfx.device;
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(globals_size),
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite texture"),
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
                // 法线图；没有的绑白色
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprites"),
            bind_group_layouts: &[Some(&globals_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let plain = compile(device, "sprites", &sprite_source(None));
        Self { globals_layout, texture_layout, layout, modules: vec![plain], names: HashMap::new(), generation: 0, pipelines: HashMap::new(), warned: HashSet::new(), warm: Warm::new(), warming: None }
    }

    /// 每帧画之前调：着色器库换了一代时编过的全丢掉、后台重新预热（`depth` 这种先热）；收下后台编好的。
    pub fn sync(&mut self, gfx: &Gfx, shaders: &Shaders, depth: bool) {
        if shaders.generation() != self.generation {
            self.generation = shaders.generation();
            self.modules.truncate(1);
            self.names.clear();
            self.pipelines.clear();
            self.warned.clear();
        }
        if self.warming != Some(self.generation) {
            self.warming = Some(self.generation);
            self.start_warm(gfx, shaders, depth);
        }
        for warmed in self.warm.drain() {
            self.take(warmed);
        }
    }

    /// 后台把库里每个精灵着色器 × 每种画法 × 混排与否的管线都编一遍。编不过的跳过，用到时再当场校验、提示。
    fn start_warm(&mut self, gfx: &Gfx, shaders: &Shaders, depth: bool) {
        let (device, cache, layout, format, plain) = (gfx.device.clone(), gfx.pipeline_cache.clone(), self.layout.clone(), gfx.format, self.modules[0].clone());
        let library = shaders.library();
        self.warm.start(move |send| {
            let mut modules = vec![(None, plain)];
            for (name, code) in library {
                let source = sprite_source(Some(&code));
                if validate(&source, &code).is_err() {
                    continue;
                }
                let module = compile(&device, &name, &source);
                if !send(Warmed::Module(name.clone(), module.clone())) {
                    return;
                }
                modules.push((Some(name), module));
            }
            // 先热这一帧的混排方式；同一种里先热普通画法，遮罩的后热
            for depth in [depth, !depth] {
                for mode in Mode::ALL {
                    for (name, module) in &modules {
                        if !send(Warmed::Pipeline(name.clone(), mode, depth, build(&device, cache.as_ref(), &layout, format, module, mode, depth))) {
                            return;
                        }
                    }
                }
            }
        });
    }

    fn take(&mut self, warmed: Warmed) {
        match warmed {
            Warmed::Module(name, module) => {
                // 渲染线程等不及已经当场编了的，用它的编号
                if !self.names.contains_key(&name) {
                    self.modules.push(module);
                    self.names.insert(name, self.modules.len() as u32 - 1);
                }
            }
            Warmed::Pipeline(name, mode, depth, pipeline) => {
                let index = match name {
                    None => 0,
                    Some(name) => match self.names.get(&name) {
                        Some(&index) if index != 0 => index,
                        _ => return,
                    },
                };
                self.pipelines.entry(pipe(index, mode, depth)).or_insert(pipeline);
            }
        }
    }

    /// 名字对应的着色器编号；第一次用到时校验、编译，不在库里或编不过的提示一次、按原样画。
    pub fn module(&mut self, gfx: &Gfx, shaders: &Shaders, name: &str) -> u32 {
        if let Some(&index) = self.names.get(name) {
            return index;
        }
        let index = match shaders.code(name) {
            None => {
                self.warn(name, || format!("精灵着色器 {name} 不在着色器库里：按原样画"));
                0
            }
            Some(code) => {
                let source = sprite_source(Some(code));
                match validate(&source, code) {
                    Err(e) => {
                        self.warn(name, || format!("精灵着色器 {name} 编不过，按原样画：{e}"));
                        0
                    }
                    Ok(()) => {
                        self.modules.push(compile(&gfx.device, name, &source));
                        self.modules.len() as u32 - 1
                    }
                }
            }
        };
        self.names.insert(name.to_owned(), index);
        index
    }

    fn warn(&mut self, name: &str, message: impl FnOnce() -> String) {
        if self.warned.insert(name.to_owned()) {
            log::error!("{}", message());
        }
    }

    /// 准备好这条管线（画之前调）。
    pub fn prepare(&mut self, gfx: &Gfx, pipe: u32) {
        if self.pipelines.contains_key(&pipe) {
            return;
        }
        let pipeline = build(&gfx.device, gfx.pipeline_cache.as_ref(), &self.layout, gfx.format, &self.modules[(pipe / 8) as usize], Mode::ALL[(pipe % 4) as usize], pipe / 4 % 2 == 1);
        self.pipelines.insert(pipe, pipeline);
    }

    /// 等后台这一批编完并收下。
    #[cfg(test)]
    fn settle(&mut self) {
        for warmed in self.warm.finish() {
            self.take(warmed);
        }
    }

    pub fn get(&self, pipe: u32) -> &wgpu::RenderPipeline {
        &self.pipelines[&pipe]
    }
}

fn compile(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(source.into()) })
}

/// 一条精灵管线。混排时用 3D 留下的深度（深度 + 模板），比深度不写深度（精灵是半透明的，靠 z 排序）。
fn build(device: &wgpu::Device, cache: Option<&wgpu::PipelineCache>, layout: &wgpu::PipelineLayout, color: wgpu::TextureFormat, module: &wgpu::ShaderModule, mode: Mode, depth: bool) -> wgpu::RenderPipeline {
    let (format, depth_compare) = if depth { (nge_ecs::SCENE_DEPTH_FORMAT, wgpu::CompareFunction::LessEqual) } else { (STENCIL_FORMAT, wgpu::CompareFunction::Always) };
    let write = mode == Mode::Write;
    let attributes = wgpu::vertex_attr_array![
        0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4, 4 => Unorm8x4, 5 => Uint32, 6 => Float32x4
    ];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sprites"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            buffers: &[Some(wgpu::VertexBufferLayout { array_stride: size_of::<Instance>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &attributes })],
            compilation_options: Default::default(),
        },
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format,
            depth_write_enabled: Some(false),
            depth_compare: Some(depth_compare),
            stencil: mode.stencil(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(if write { "fs_mask" } else { "fs" }),
            targets: &[Some(wgpu::ColorTargetState {
                format: color,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: if write { wgpu::ColorWrites::empty() } else { wgpu::ColorWrites::ALL },
            })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use nge_core::wgpu;

    use super::*;
    use crate::effects::Globals;
    use crate::test_gfx;

    const GLOW: &str = "fn effect(e: Effect) -> vec4f {\n    return e.color * 2.0;\n}";
    const POST: &str = "fn post(p: Post) -> vec4f {\n    return p.color;\n}";
    /// 内置的精灵着色器有几个。
    const BUILTIN_SPRITES: usize = 4;

    fn gfx() -> Option<nge_core::Gfx> {
        test_gfx(wgpu::TextureFormat::Rgba8UnormSrgb)
    }

    fn library(user: &[(&str, &str)]) -> Shaders {
        let mut shaders = Shaders::default();
        shaders.set(user.iter().map(|(n, c)| (n.to_string(), c.to_string())).collect::<HashMap<_, _>>());
        shaders
    }

    fn all_variants(pipes: &Pipes, index: u32) -> bool {
        [false, true].into_iter().all(|depth| Mode::ALL.into_iter().all(|mode| pipes.pipelines.contains_key(&pipe(index, mode, depth))))
    }

    #[test]
    fn warms_every_sprite_shader_in_every_mode_before_it_is_used() {
        let Some(gfx) = gfx() else { return };
        let shaders = library(&[("glow", GLOW), ("bad", "fn effect("), ("tint", POST)]);
        let mut pipes = Pipes::new(&gfx, size_of::<Globals>() as u64);
        pipes.sync(&gfx, &shaders, false);
        pipes.settle();
        assert!(all_variants(&pipes, 0), "原样画的");
        for name in ["flash", "outline", "wave", "circle", "glow"] {
            let index = pipes.names[name];
            assert!(index > 0 && all_variants(&pipes, index), "{name}");
        }
        // 编不过的、后处理不预热，用到时再当场校验、提示
        assert!(!pipes.names.contains_key("bad") && !pipes.names.contains_key("tint") && !pipes.names.contains_key("vignette"));
        assert_eq!(pipes.modules.len(), 1 + BUILTIN_SPRITES + 1);
        assert_eq!(pipes.pipelines.len(), (1 + BUILTIN_SPRITES + 1) * 8);
        // 用的时候直接拿热好的，不再编
        let index = pipes.names["glow"];
        assert_eq!(pipes.module(&gfx, &shaders, "glow"), index);
        assert_eq!(pipes.modules.len(), 1 + BUILTIN_SPRITES + 1);
        assert_eq!(pipes.module(&gfx, &shaders, "bad"), 0);
    }

    #[test]
    fn a_new_library_drops_what_was_warmed_for_the_old_one() {
        let Some(gfx) = gfx() else { return };
        let mut shaders = library(&[("glow", GLOW)]);
        let mut pipes = Pipes::new(&gfx, size_of::<Globals>() as u64);
        pipes.sync(&gfx, &shaders, true);
        pipes.settle();
        assert!(pipes.names.contains_key("glow"));
        shaders.set(HashMap::from([("shine".to_string(), GLOW.to_string())]));
        pipes.sync(&gfx, &shaders, true);
        pipes.settle();
        assert!(!pipes.names.contains_key("glow"));
        assert!(all_variants(&pipes, pipes.names["shine"]));
        assert_eq!(pipes.pipelines.len(), (1 + BUILTIN_SPRITES + 1) * 8);
        // 同一代不重新预热
        pipes.sync(&gfx, &shaders, true);
        assert!(!pipes.warm.busy());
    }

    #[test]
    fn switching_libraries_mid_warm_does_not_mix_the_old_names_in() {
        let Some(gfx) = gfx() else { return };
        let mut shaders = library(&[("glow", GLOW)]);
        let mut pipes = Pipes::new(&gfx, size_of::<Globals>() as u64);
        pipes.sync(&gfx, &shaders, false);
        shaders.set(HashMap::from([("shine".to_string(), GLOW.to_string())]));
        pipes.sync(&gfx, &shaders, false);
        pipes.settle();
        assert!(!pipes.names.contains_key("glow"));
        assert_eq!(pipes.modules.len(), 1 + BUILTIN_SPRITES + 1);
        assert_eq!(pipes.pipelines.len(), (1 + BUILTIN_SPRITES + 1) * 8);
        assert!(all_variants(&pipes, pipes.names["shine"]));
    }

    #[test]
    fn warmed_results_follow_what_was_compiled_on_the_spot() {
        let Some(gfx) = gfx() else { return };
        let shaders = library(&[("glow", GLOW)]);
        let mut pipes = Pipes::new(&gfx, size_of::<Globals>() as u64);
        let index = pipes.module(&gfx, &shaders, "glow");
        assert_eq!(pipes.module(&gfx, &shaders, "missing"), 0);
        let plain = pipes.modules[0].clone();
        let pipeline = || build(&gfx.device, gfx.pipeline_cache.as_ref(), &pipes.layout, gfx.format, &plain, Mode::Inside, true);
        let (late, a, b) = (Warmed::Module("glow".into(), plain.clone()), pipeline(), pipeline());
        pipes.take(late);
        assert_eq!((pipes.names["glow"], pipes.modules.len()), (index, 2), "等不及当场编了的不换");
        pipes.take(Warmed::Pipeline(Some("glow".into()), Mode::Inside, true, a));
        assert!(pipes.pipelines.contains_key(&pipe(index, Mode::Inside, true)));
        // 不在库里的（编号是 0）不能占了原样画的位置
        pipes.take(Warmed::Pipeline(Some("missing".into()), Mode::Inside, true, b));
        assert!(!pipes.pipelines.contains_key(&pipe(0, Mode::Inside, true)));
    }
}
