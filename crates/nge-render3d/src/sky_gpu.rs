//! 天空和环境光照的 GPU 部分（见 docs/render3d-quality.md）：天空换了（或贴图刚加载完）时在 CPU 上烘焙一次，
//! 上传天空立方体、预滤波的环境立方体，算 SH9；每帧只把参数写进 `Globals`、在不透明的网格后面画天空。

use glam::Vec3;
use nge_assets::{Assets, CubeImage, Image, LoadState};
use nge_core::{Gfx, wgpu};
use nge_ecs::SCENE_DEPTH_FORMAT;

use crate::ibl::{Cube, ENV_LEVELS, ENV_SAMPLES, ENV_SIZE, LUT_SAMPLES, LUT_SIZE, brdf_lut, linear_to_srgb, prefilter, sh9, srgb_to_linear, texel_dir, user_face_uv};
use crate::light::Globals;
use crate::sky::{SUN_GLOW, Sky, SkyGradient, SkySource};

/// 天空立方体最大边长：大图按最近的像素缩小。
const MAX_SKY_SIZE: u32 = 1024;
const CUBE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// 烘焙要的数据。
enum Input {
    None,
    Gradient(SkyGradient),
    /// 边长，6 个面的 RGBA8（sRGB），按用户的面约定。
    Faces(u32, Vec<Vec<u8>>),
    /// 贴图还在加载。
    Pending,
    Bad(String),
}

/// 这次烘焙出来的东西。
#[derive(Clone, Copy)]
struct Baked {
    /// 1 程序天空、2 立方体。
    mode: u32,
    sh: [Vec3; 9],
    gradient: SkyGradient,
}

pub(crate) struct SkyGpu {
    pipeline: wgpu::RenderPipeline,
    env: wgpu::Texture,
    pub env_view: wgpu::TextureView,
    pub sky_view: wgpu::TextureView,
    pub lut_view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    /// 烘焙过的来源；和这一帧的不一样时重新烘焙。
    source: Option<SkySource>,
    baked: Option<Baked>,
    warned: bool,
}

impl SkyGpu {
    pub fn new(gfx: &Gfx, globals_layout: &wgpu::BindGroupLayout, module: &wgpu::ShaderModule) -> Self {
        let device = &gfx.device;
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("sky"), bind_group_layouts: &[Some(globals_layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { module, entry_point: Some("vs_sky"), buffers: &[], compilation_options: Default::default() },
            primitive: Default::default(),
            // 画在远平面：深度清成 1 的地方才画，不写深度
            depth_stencil: Some(wgpu::DepthStencilState {
                format: SCENE_DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_sky"),
                targets: &[Some(wgpu::ColorTargetState { format: gfx.format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: gfx.pipeline_cache.as_ref(),
        });
        let env = cube_texture(gfx, "environment", ENV_SIZE, ENV_LEVELS);
        let env_view = cube_view(&env);
        let sky = cube_texture(gfx, "sky", 1, 1);
        write_level(gfx, &sky, 0, 1, &[0; 24]);
        let lut = gfx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("brdf lut"),
            size: wgpu::Extent3d { width: LUT_SIZE, height: LUT_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gfx.queue.write_texture(
            lut.as_image_copy(),
            bytemuck::cast_slice(&brdf_lut(LUT_SIZE, LUT_SAMPLES)),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * LUT_SIZE), rows_per_image: Some(LUT_SIZE) },
            lut.size(),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("environment"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            env,
            env_view,
            sky_view: cube_view(&sky),
            lut_view: lut.create_view(&Default::default()),
            sampler,
            source: None,
            baked: None,
            warned: false,
        }
    }

    /// 来源变了（或贴图刚加载完）就烘焙，再把这一帧的参数写进 `globals`。返回天空立方体有没有换（bind group 要重建）。
    pub fn prepare(&mut self, gfx: &Gfx, sky: &Sky, images: &Assets<Image>, cubes: &Assets<CubeImage>, globals: &mut Globals) -> bool {
        let mut rebuilt = false;
        if self.source.as_ref() != Some(&sky.source) {
            match input(&sky.source, images, cubes) {
                Input::Pending => self.baked = None,
                Input::None => self.finish(sky, None),
                Input::Bad(e) => {
                    if !std::mem::replace(&mut self.warned, true) {
                        log::warn!("天空用不了，不画：{e}");
                    }
                    self.finish(sky, None);
                }
                Input::Gradient(g) => {
                    let base = Cube::from_fn(ENV_SIZE, |d| g.radiance(d));
                    let baked = self.upload_env(gfx, &base, 1, g);
                    self.finish(sky, Some(baked));
                }
                Input::Faces(size, faces) => {
                    let lookup = |d: Vec3| {
                        let (f, u, v) = user_face_uv(d);
                        let (x, y) = (((u * size as f32) as u32).min(size - 1), ((v * size as f32) as u32).min(size - 1));
                        let at = ((y * size + x) * 4) as usize;
                        &faces[f][at..at + 4]
                    };
                    let linear = |d: Vec3| {
                        let p = lookup(d);
                        Vec3::new(srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2]))
                    };
                    let base = Cube::from_fn_supersampled(ENV_SIZE, (size / ENV_SIZE).clamp(1, 4), linear);
                    rebuilt |= self.upload_sky(gfx, size.min(MAX_SKY_SIZE), |d| lookup(d).try_into().expect("4 个字节"));
                    let baked = self.upload_env(gfx, &base, 2, SkyGradient::default());
                    self.finish(sky, Some(baked));
                }
            }
        }
        let Some(b) = self.baked else { return rebuilt };
        globals.env = [sky.diffuse.max(0.0), sky.specular.max(0.0), (ENV_LEVELS - 1) as f32, 1.0];
        globals.sh = b.sh.map(|c| c.extend(0.0).into());
        globals.casters[3] = if sky.background { b.mode } else { 0 };
        let g = b.gradient;
        let sun = (-g.sun).try_normalize();
        globals.sky = [
            g.top.extend(0.0).into(),
            g.horizon.extend(0.0).into(),
            g.ground.extend(0.0).into(),
            sun.unwrap_or(Vec3::Y).extend(g.sun_size.max(1e-4)).into(),
            g.sun_color.extend(if sun.is_some() { 1.0 } else { 0.0 }).into(),
        ];
        debug_assert_eq!(SUN_GLOW, (64.0, 0.25), "着色器里写死了光晕");
        rebuilt
    }

    /// 有天空（不管画不画）。
    pub fn has_sky(&self) -> bool {
        self.baked.is_some()
    }

    fn finish(&mut self, sky: &Sky, baked: Option<Baked>) {
        self.source = Some(sky.source.clone());
        self.baked = baked;
    }

    /// 画在不透明的网格之后：只盖住深度还是 1 的地方。
    pub fn draw(&self, pass: &mut wgpu::RenderPass, globals: &Globals) {
        if globals.casters[3] != 0 {
            pass.set_pipeline(&self.pipeline);
            pass.draw(0..3, 0..1);
        }
    }

    /// 预滤波、算 SH，写进环境立方体的每一级。
    fn upload_env(&mut self, gfx: &Gfx, base: &Cube, mode: u32, gradient: SkyGradient) -> Baked {
        for (level, cube) in prefilter(base, ENV_LEVELS, ENV_SAMPLES).iter().enumerate() {
            let bytes: Vec<u8> = cube.texels.iter().flat_map(|c| [linear_to_srgb(c.x), linear_to_srgb(c.y), linear_to_srgb(c.z), 255]).collect();
            write_level(gfx, &self.env, level as u32, cube.size, &bytes);
        }
        Baked { mode, sh: sh9(base), gradient }
    }

    /// 按 GPU 的面约定重排成 `size` 边长的新天空立方体（视图换了，bind group 要重建）。
    fn upload_sky(&mut self, gfx: &Gfx, size: u32, pixel: impl Fn(Vec3) -> [u8; 4]) -> bool {
        let n = size as usize;
        let bytes: Vec<u8> = (0..6 * n * n)
            .flat_map(|i| {
                let (face, y, x) = (i / (n * n), (i / n) % n, i % n);
                pixel(texel_dir(face, (x as f32 + 0.5) / size as f32, (y as f32 + 0.5) / size as f32))
            })
            .collect();
        let texture = cube_texture(gfx, "sky", size, 1);
        write_level(gfx, &texture, 0, size, &bytes);
        self.sky_view = cube_view(&texture);
        true
    }
}

/// 来源 → 烘焙要的数据。
fn input(source: &SkySource, images: &Assets<Image>, cubes: &Assets<CubeImage>) -> Input {
    match source {
        SkySource::None => Input::None,
        SkySource::Gradient(g) => Input::Gradient(*g),
        SkySource::Cube(handle) => match (cubes.get(*handle), cubes.state(*handle)) {
            (Some(c), _) => Input::Faces(c.size, c.faces.to_vec()),
            (None, LoadState::Failed(e)) => Input::Bad(format!("立方体贴图加载失败：{e}")),
            (None, _) => Input::Pending,
        },
        SkySource::Faces(ids) => {
            let mut faces = Vec::with_capacity(6);
            let mut size = None;
            for (i, id) in ids.iter().enumerate() {
                let image = match (images.get(*id), images.state(*id)) {
                    (Some(image), _) => image,
                    (None, LoadState::Failed(e)) => return Input::Bad(format!("第 {i} 面加载失败：{e}")),
                    (None, _) => return Input::Pending,
                };
                if image.size.x != image.size.y || size.is_some_and(|s| s != image.size.x) {
                    return Input::Bad(format!("6 个面要一样大的正方形，第 {i} 面是 {}", image.size));
                }
                size = Some(image.size.x);
                let rgba = match &image.compressed {
                    Some(c) => match c.decode_rgba(image.size) {
                        Ok(rgba) => rgba,
                        Err(e) => return Input::Bad(format!("第 {i} 面解不开：{e}")),
                    },
                    None => image.rgba.clone(),
                };
                faces.push(rgba);
            }
            Input::Faces(size.unwrap_or(1), faces)
        }
    }
}

fn cube_texture(gfx: &Gfx, label: &str, size: u32, levels: u32) -> wgpu::Texture {
    gfx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 6 },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: CUBE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn cube_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::Cube), ..Default::default() })
}

/// 写一级的 6 个面（`bytes` 是 6 个面连着的 RGBA8）。
fn write_level(gfx: &Gfx, texture: &wgpu::Texture, level: u32, size: u32, bytes: &[u8]) {
    gfx.queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        bytes,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * size), rows_per_image: Some(size) },
        wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 6 },
    );
}
