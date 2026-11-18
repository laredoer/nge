//! 天空和环境光照的 GPU 部分（见 docs/render3d-quality.md）：天空换了（或贴图刚加载完）时在后台线程烘焙一次
//! （wasm 上当场烘焙），好了再上传天空立方体、预滤波的环境立方体和 SH9；烘焙好之前留着上一次的。
//! 程序天空的太阳不参与烘焙（镜面高光归方向光），只在 `fs_sky` 里画：太阳转动不重新烘焙。

use std::sync::mpsc::{Receiver, channel};

use bevy_ecs::prelude::Resource;
use glam::Vec3;
use nge_assets::{Assets, CubeImage, Image, LoadState};
use nge_core::{Gfx, wgpu};
use nge_ecs::SCENE_DEPTH_FORMAT;

use crate::ibl::{Cube, ENV_LEVELS, ENV_SAMPLES, ENV_SIZE, LUT_SAMPLES, LUT_SIZE, brdf_lut, dir_face_uv, linear_to_srgb, prefilter, sh9, srgb_to_linear, texel_dir, user_face_uv};
use crate::light::Globals;
use crate::sky::{SUN_GLOW, Sky, SkyGradient, SkySource};

/// 天空立方体最大边长：大图按最近的像素缩小。
const MAX_SKY_SIZE: u32 = 1024;
/// 贴图烘焙环境立方体时先按这个边长（最多 ENV_SIZE × 4）每格平均最多 8 × 8 个像素，再逐级减半到 ENV_SIZE。
const MAX_SUPERSAMPLE: u32 = 8;
const CUBE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// 天空烘焙的状态：`ready` 是这一帧要的天空已经烘焙、上传好了（没有天空也算），`count` 是一共开始烘焙过几次。
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SkyBake {
    pub ready: bool,
    pub count: u32,
}

/// 烘焙要的数据。
enum Input {
    None,
    Job(Job),
    /// 贴图还在加载。
    Pending,
    Bad(String),
}

enum Job {
    Gradient(SkyGradient),
    /// 边长，6 个面的 RGBA8（sRGB）；`native` 是按 GPU（KTX2 规范）的面约定，否则按引擎给 6 张图定的约定。
    Faces { size: u32, faces: Vec<Vec<u8>>, native: bool },
}

/// 烘焙的结果（后台线程算好，主线程上传）。
struct Output {
    key: SkySource,
    mode: u32,
    /// 每一级的边长和 6 个面的 RGBA8。
    env: Vec<(u32, Vec<u8>)>,
    sh: [Vec3; 9],
    /// 天空立方体（只有贴图有）。
    sky: Option<(u32, Vec<u8>)>,
}

/// 上传好的。
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
    /// 天空立方体的边长：1 是没有贴图时的空立方体。
    sky_size: u32,
    pub lut_view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    /// 要的（在烘焙或已经好了的）来源，按 [`bake_key`]。
    wanted: Option<SkySource>,
    /// 后台在烘焙的。
    job: Option<Receiver<Output>>,
    baked: Option<Baked>,
    /// 这个来源已经 warn 过（换了来源再说）。
    warned: Option<SkySource>,
    status: SkyBake,
}

/// 烘焙只看这些：程序天空去掉太阳。
pub(crate) fn bake_key(source: &SkySource) -> SkySource {
    match source {
        SkySource::Gradient(g) => SkySource::Gradient(without_sun(*g)),
        other => other.clone(),
    }
}

fn without_sun(g: SkyGradient) -> SkyGradient {
    SkyGradient { sun: Vec3::ZERO, sun_color: Vec3::ZERO, sun_size: 0.0, ..g }
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
            sky_view: empty_sky(gfx),
            sky_size: 1,
            lut_view: lut.create_view(&Default::default()),
            sampler,
            wanted: None,
            job: None,
            baked: None,
            warned: None,
            status: SkyBake::default(),
        }
    }

    /// 来源变了（或贴图刚加载完）就开始烘焙，烘焙好了上传，再把这一帧的参数写进 `globals`。
    /// 返回天空立方体有没有换（bind group 要重建）。
    pub fn prepare(&mut self, gfx: &Gfx, sky: &Sky, images: &Assets<Image>, cubes: &Assets<CubeImage>, globals: &mut Globals) -> bool {
        let key = bake_key(&sky.source);
        let mut rebuilt = false;
        let mut pending = false;
        if self.wanted.as_ref() != Some(&key) {
            match input(&sky.source, images, cubes) {
                // 还在加载：留着上一次的，下一帧再看
                Input::Pending => pending = true,
                Input::None => {
                    self.settle(key);
                    rebuilt |= self.release_sky(gfx);
                }
                Input::Bad(e) => {
                    if self.warned.as_ref() != Some(&key) {
                        log::warn!("天空用不了，不画：{e}");
                        self.warned = Some(key.clone());
                    }
                    self.settle(key);
                    rebuilt |= self.release_sky(gfx);
                }
                Input::Job(job) => {
                    self.status.count += 1;
                    self.job = Some(start(key.clone(), job));
                    self.wanted = Some(key);
                }
            }
        }
        if let Some(out) = self.job.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.job = None;
            if Some(&out.key) == self.wanted.as_ref() {
                rebuilt |= self.upload(gfx, out);
            }
        }
        self.status.ready = !pending && self.job.is_none();
        let Some(b) = self.baked else { return rebuilt };
        globals.env = [sky.diffuse.max(0.0), sky.specular.max(0.0), (ENV_LEVELS - 1) as f32, 1.0];
        globals.sh = b.sh.map(|c| c.extend(0.0).into());
        globals.casters[3] = if sky.background { b.mode } else { 0 };
        // 太阳每帧按这一帧的写，不用等烘焙
        let g = match &sky.source {
            SkySource::Gradient(g) if b.mode == 1 => *g,
            _ => b.gradient,
        };
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

    pub fn status(&self) -> SkyBake {
        self.status
    }

    /// 有天空（不管画不画）。
    pub fn has_sky(&self) -> bool {
        self.baked.is_some()
    }

    /// 没有天空：扔掉烘焙的和在烘焙的。
    fn settle(&mut self, key: SkySource) {
        self.wanted = Some(key);
        self.job = None;
        self.baked = None;
    }

    /// 换回 1 × 1 的空天空立方体，放掉大的。
    fn release_sky(&mut self, gfx: &Gfx) -> bool {
        if self.sky_size == 1 {
            return false;
        }
        self.sky_view = empty_sky(gfx);
        self.sky_size = 1;
        true
    }

    /// 画在不透明的网格之后：只盖住深度还是 1 的地方。
    pub fn draw(&self, pass: &mut wgpu::RenderPass, globals: &Globals) {
        if globals.casters[3] != 0 {
            pass.set_pipeline(&self.pipeline);
            pass.draw(0..3, 0..1);
        }
    }

    fn upload(&mut self, gfx: &Gfx, out: Output) -> bool {
        for (level, (size, bytes)) in out.env.iter().enumerate() {
            write_level(gfx, &self.env, level as u32, *size, bytes);
        }
        let gradient = match &out.key {
            SkySource::Gradient(g) => *g,
            _ => SkyGradient::default(),
        };
        self.baked = Some(Baked { mode: out.mode, sh: out.sh, gradient });
        match out.sky {
            Some((size, bytes)) => {
                let texture = cube_texture(gfx, "sky", size, 1);
                write_level(gfx, &texture, 0, size, &bytes);
                self.sky_view = cube_view(&texture);
                self.sky_size = size;
                true
            }
            None => self.release_sky(gfx),
        }
    }
}

/// 在后台线程烘焙（wasm 上没有线程，当场烘焙）。
fn start(key: SkySource, job: Job) -> Receiver<Output> {
    let (tx, rx) = channel();
    let work = move || drop(tx.send(bake(key, job)));
    #[cfg(not(target_arch = "wasm32"))]
    if let Err(e) = std::thread::Builder::new().name("sky bake".into()).spawn(work) {
        log::warn!("开不了烘焙天空的线程：{e}");
    }
    #[cfg(target_arch = "wasm32")]
    work();
    rx
}

fn bake(key: SkySource, job: Job) -> Output {
    let (mode, base, sky) = match job {
        Job::Gradient(g) => (1, Cube::from_fn(ENV_SIZE, |d| without_sun(g).radiance(d)), None),
        Job::Faces { size, faces, native } => {
            let face_uv = if native { dir_face_uv } else { user_face_uv };
            let lookup = |d: Vec3| {
                let (f, u, v) = face_uv(d);
                let (x, y) = (((u * size as f32) as u32).min(size - 1), ((v * size as f32) as u32).min(size - 1));
                let at = ((y * size + x) * 4) as usize;
                [faces[f][at], faces[f][at + 1], faces[f][at + 2], faces[f][at + 3]]
            };
            let linear = |d: Vec3| {
                let p = lookup(d);
                Vec3::new(srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2]))
            };
            let n = size.min(MAX_SKY_SIZE);
            let s = n as usize;
            let bytes: Vec<u8> = (0..6 * s * s)
                .flat_map(|i| {
                    let (face, y, x) = (i / (s * s), (i / s) % s, i % s);
                    lookup(texel_dir(face, (x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32))
                })
                .collect();
            (2, env_base(size, linear), Some((n, bytes)))
        }
    };
    // 和镜面的立方体（LDR）一样截断，漫反射和镜面一致
    let base = Cube { size: base.size, texels: base.texels.iter().map(|c| c.clamp(Vec3::ZERO, Vec3::ONE)).collect() };
    let env = prefilter(&base, ENV_LEVELS, ENV_SAMPLES)
        .iter()
        .map(|cube| (cube.size, cube.texels.iter().flat_map(|c| [linear_to_srgb(c.x), linear_to_srgb(c.y), linear_to_srgb(c.z), 255]).collect()))
        .collect();
    Output { key, mode, env, sh: sh9(&base), sky }
}

/// 边长 `size` 的贴图天空 → ENV_SIZE 的线性立方体：先在 ENV_SIZE × 2^k（不超过 `size`、最多 × 4）上每格平均覆盖它的像素，
/// 再逐级减半，大图不会只取到零星几个像素（高 mip 上一闪一闪）。
pub(crate) fn env_base(size: u32, linear: impl Fn(Vec3) -> Vec3) -> Cube {
    let mut target = ENV_SIZE;
    while target * 2 <= size.min(ENV_SIZE * 4) {
        target *= 2;
    }
    let ss = size.div_ceil(target).clamp(1, MAX_SUPERSAMPLE);
    let mut cube = Cube::from_fn_supersampled(target, ss, linear);
    while cube.size > ENV_SIZE {
        cube = cube.half();
    }
    cube
}

/// 来源 → 烘焙要的数据。
fn input(source: &SkySource, images: &Assets<Image>, cubes: &Assets<CubeImage>) -> Input {
    match source {
        SkySource::None => Input::None,
        SkySource::Gradient(g) => Input::Job(Job::Gradient(*g)),
        // KTX2 立方体按规范的 GPU 面约定
        SkySource::Cube(handle) => match (cubes.get(*handle), cubes.state(*handle)) {
            (Some(c), _) => Input::Job(Job::Faces { size: c.size, faces: c.faces.to_vec(), native: true }),
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
            Input::Job(Job::Faces { size: size.unwrap_or(1), faces, native: false })
        }
    }
}

fn empty_sky(gfx: &Gfx) -> wgpu::TextureView {
    let sky = cube_texture(gfx, "sky", 1, 1);
    write_level(gfx, &sky, 0, 1, &[0; 24]);
    cube_view(&sky)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turning_the_sun_does_not_change_what_is_baked() {
        let g = SkyGradient::default();
        let turned = SkyGradient { sun: Vec3::new(3.0, -1.0, 0.0), sun_color: Vec3::ONE, sun_size: 0.2, ..g };
        assert_eq!(bake_key(&SkySource::Gradient(g)), bake_key(&SkySource::Gradient(turned)));
        let other = SkyGradient { top: Vec3::ONE, ..g };
        assert_ne!(bake_key(&SkySource::Gradient(g)), bake_key(&SkySource::Gradient(other)), "换颜色要重新烘焙");
    }

    #[test]
    fn baking_leaves_the_sun_out_and_clamps_like_the_cube() {
        // 太阳在 +X 的地平线上（光往 -X 照）、50 倍亮；天顶 3 倍亮
        let g = SkyGradient { top: Vec3::splat(3.0), horizon: Vec3::splat(0.5), ground: Vec3::splat(0.5), sun: Vec3::NEG_X, sun_color: Vec3::splat(50.0), sun_size: 0.3 };
        let out = bake(bake_key(&SkySource::Gradient(g)), Job::Gradient(g));
        let (size, bytes) = &out.env[0];
        let n = *size as usize;
        let center = |face: usize| bytes[(face * n * n + n / 2 * n + n / 2) * 4];
        assert_eq!(center(2), 255, "天顶截到 1");
        assert!(center(0).abs_diff(linear_to_srgb(0.5)) <= 3, "太阳不进环境立方体：+X 是地平线色 {}", center(0));
        // SH 也按截断后的算：正上方的辐射度不超过 1 多少
        let up: Vec3 = out.sh.iter().zip(crate::ibl::sh_basis(Vec3::Y)).map(|(s, y)| *s * y).sum();
        assert!(up.x < 1.15, "截断后再算 SH {up}");
        assert!(out.sky.is_none() && out.mode == 1);
    }

    #[test]
    fn big_skies_are_averaged_down_not_point_sampled() {
        // 1024 的棋盘格（一格一像素）：平均下来每格都是灰的
        let size = 1024;
        let checker = |d: Vec3| {
            let (_, u, v) = dir_face_uv(d);
            let (x, y) = ((u * size as f32) as u32, (v * size as f32) as u32);
            Vec3::splat(((x + y) % 2) as f32)
        };
        let base = env_base(size, checker);
        assert_eq!(base.size, ENV_SIZE);
        let (lo, hi) = base.texels.iter().fold((1.0f32, 0.0f32), |(lo, hi), c| (lo.min(c.x), hi.max(c.x)));
        assert!(lo > 0.4 && hi < 0.6, "每格都覆盖到：{lo} … {hi}");
        // 小图照样到 ENV_SIZE
        assert_eq!(env_base(16, |_| Vec3::ONE).size, ENV_SIZE);
    }
}
