//! 精灵管线：按（着色器，遮罩模式）编号、按需编译、着色器库改了就丢掉重来（见 docs/effects2d.md）。

use std::collections::{HashMap, HashSet};

use nge_core::{Gfx, wgpu};

use crate::batch::Instance;
use crate::effects::{Masking, Shaders, sprite_source, validate};

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
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprites"),
            bind_group_layouts: &[Some(&globals_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let plain = compile(gfx, "sprites", &sprite_source(None));
        Self { globals_layout, texture_layout, layout, modules: vec![plain], names: HashMap::new(), generation: 0, pipelines: HashMap::new(), warned: HashSet::new() }
    }

    /// 着色器库换了一代：编过的全丢掉。
    pub fn sync(&mut self, shaders: &Shaders) {
        if shaders.generation() != self.generation {
            self.generation = shaders.generation();
            self.modules.truncate(1);
            self.names.clear();
            self.pipelines.clear();
            self.warned.clear();
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
                        self.modules.push(compile(gfx, name, &source));
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
        let module = &self.modules[(pipe / 8) as usize];
        let mode = Mode::ALL[(pipe % 4) as usize];
        // 混排：用 3D 留下的深度（深度 + 模板），比深度不写深度（精灵是半透明的，靠 z 排序）
        let (format, depth_compare) =
            if pipe / 4 % 2 == 1 { (nge_ecs::SCENE_DEPTH_FORMAT, wgpu::CompareFunction::LessEqual) } else { (STENCIL_FORMAT, wgpu::CompareFunction::Always) };
        let write = mode == Mode::Write;
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4, 4 => Unorm8x4, 5 => Uint32, 6 => Float32x4
        ];
        let pipeline = gfx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprites"),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                })],
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
                    format: gfx.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: if write { wgpu::ColorWrites::empty() } else { wgpu::ColorWrites::ALL },
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        self.pipelines.insert(pipe, pipeline);
    }

    pub fn get(&self, pipe: u32) -> &wgpu::RenderPipeline {
        &self.pipelines[&pipe]
    }
}

fn compile(gfx: &Gfx, label: &str, source: &str) -> wgpu::ShaderModule {
    gfx.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(source.into()) })
}
