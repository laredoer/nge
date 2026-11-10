//! 2D 特效的离屏渲染：着色器、遮罩、2D 光照、后处理（见 docs/effects2d.md）。没有 GPU 时跳过。

use glam::{Vec2, Vec4};
use nge_core::{Ctx, Emit, Gfx, Viewport, wgpu};
use nge_ecs::prelude::*;
use nge_render2d::prelude::*;
use nge_render2d::{Light2d, Lighting2d, Masking, PostEffect, PostEffects, PostLayer, Shaders, SpriteImage, SpriteInstance, SpriteInstances, SpriteShader, TextureId, Unlit};
use glam::Vec3;

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn gfx() -> Option<Gfx> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    let viewport = Viewport { size: [SIZE as f32; 2], pixels: [SIZE; 2] };
    Some(Gfx { screen: None, pipeline_cache: None, device, queue, format: FORMAT, viewport })
}

/// 跑一帧并读回 RGBA 像素。
fn render(gfx: &Gfx, setup: impl Fn(&mut World) + Send + Sync + 'static) -> Vec<[u8; 4]> {
    let setup = std::sync::Arc::new(setup);
    render_app(gfx, move |app| {
        let setup = setup.clone();
        app.add_systems(Startup, move |world: &mut World| setup(world));
    })
}

/// 同 [`render`]，但可以往 App 里加系统。
fn render_app(gfx: &Gfx, build: impl Fn(&mut App) + Send + Sync + 'static) -> Vec<[u8; 4]> {
    render_frames(gfx, 1, build)
}

/// 跑 `frames` 帧（每帧都画），读回最后一帧。
fn render_frames(gfx: &Gfx, frames: usize, build: impl Fn(&mut App) + Send + Sync + 'static) -> Vec<[u8; 4]> {
    let factory = nge_ecs::factory(move || {
        let mut app = App::new();
        app.add_plugins(Render2dPlugin);
        build(&mut app);
        app
    });
    let mut game = factory(gfx);
    let emit: Emit = Box::new(|_, _| {});

    let texture = gfx.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let readback = gfx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let view = texture.create_view(&Default::default());
    for _ in 1..frames {
        game.update(&mut Ctx::new(gfx.viewport, &emit), 0.016);
        let mut encoder = gfx.device.create_command_encoder(&Default::default());
        game.render(gfx, &mut encoder, &view);
        gfx.queue.submit([encoder.finish()]);
        // 每帧都满一个统计窗口，下一帧的系统能读到这一帧的汇总
        game.timing(nge_core::FrameTiming { frame: 0.6, ..Default::default() });
    }
    game.update(&mut Ctx::new(gfx.viewport, &emit), 0.016);
    let mut encoder = gfx.device.create_command_encoder(&Default::default());
    game.render(gfx, &mut encoder, &view);
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE * 4), rows_per_image: None },
        },
        wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
    );
    gfx.queue.submit([encoder.finish()]);
    readback.map_async(wgpu::MapMode::Read, .., |r| r.expect("map"));
    gfx.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let data = readback.get_mapped_range(..).expect("mapped");
    bytemuck::cast_slice::<u8, [u8; 4]>(&data).to_vec()
}

fn at(pixels: &[[u8; 4]], x: u32, y: u32) -> [u8; 4] {
    pixels[(y * SIZE + x) as usize]
}

fn gray(v: u8) -> [u8; 4] {
    [v, v, v, 255]
}

/// 颜色各通道差不超过 `tol`。
fn near(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tol)
}

fn square(world: &mut World, x: f32, y: f32, size: f32, color: Vec4) -> Entity {
    world.spawn((Sprite::color(Vec2::splat(size), color), Transform2d::from_xy(x, y))).id()
}

const RED: Vec4 = Vec4::new(1.0, 0.0, 0.0, 1.0);

#[test]
fn builtin_flash_mixes_toward_the_param_color() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        let e = square(world, -16.0, 0.0, 32.0, RED);
        world.entity_mut(e).insert(SpriteShader { name: "flash".into(), params: Vec4::new(0.0, 0.0, 1.0, 1.0) });
        let half = square(world, 16.0, 0.0, 32.0, RED);
        world.entity_mut(half).insert(SpriteShader { name: "flash".into(), params: Vec4::new(1.0, 1.0, 1.0, 0.5) });
    });
    assert_eq!(at(&pixels, 16, 32), [0, 0, 255, 255], "全混成蓝");
    // 线性的 (1, 0.5, 0.5) 存成 sRGB 是 (255, 188, 188)
    assert!(near(at(&pixels, 48, 32), [255, 188, 188, 255], 2), "{:?}", at(&pixels, 48, 32));
}

#[test]
fn custom_shaders_from_the_library_and_bad_ones_fall_back() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        let mut shaders = world.resource_mut::<Shaders>();
        shaders.set(
            [
                ("green".to_owned(), "fn effect(e: Effect) -> vec4f {\n    return vec4f(0.0, e.params.y, 0.0, 1.0);\n}".to_owned()),
                ("broken".to_owned(), "fn effect(e: Effect) -> vec4f { return e.nope; }".to_owned()),
            ]
            .into(),
        );
        let a = square(world, -16.0, 0.0, 32.0, RED);
        world.entity_mut(a).insert(SpriteShader { name: "green".into(), params: Vec4::new(0.0, 1.0, 0.0, 0.0) });
        let b = square(world, 16.0, 0.0, 32.0, RED);
        world.entity_mut(b).insert(SpriteShader { name: "broken".into(), params: Vec4::ZERO });
        let c = square(world, 0.0, 24.0, 8.0, RED);
        world.entity_mut(c).insert(SpriteShader { name: "missing".into(), params: Vec4::ZERO });
    });
    assert_eq!(at(&pixels, 16, 40), [0, 255, 0, 255]);
    assert_eq!(at(&pixels, 48, 40), [255, 0, 0, 255], "编不过的按原样画");
    assert_eq!(at(&pixels, 32, 8), [255, 0, 0, 255], "库里没有的按原样画");
}

#[test]
fn masks_clip_inside_and_outside() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        world.insert_resource(ClearColor(wgpu::Color::BLACK));
        // 遮罩是左半边，z 比被遮的大也照样先记
        let mask = world.spawn((Sprite::color(Vec2::new(32.0, 64.0), Vec4::ONE), Transform2d::from_xy(-16.0, 0.0).with_z(5.0), Masking::Write)).id();
        let _ = mask;
        // 上半屏红色只在遮罩里，下半屏绿色只在遮罩外
        world.spawn((Sprite::color(Vec2::new(64.0, 32.0), RED), Transform2d::from_xy(0.0, 16.0), Masking::Inside));
        world.spawn((Sprite::color(Vec2::new(64.0, 32.0), Vec4::new(0.0, 1.0, 0.0, 1.0)), Transform2d::from_xy(0.0, -16.0), Masking::Outside));
    });
    assert_eq!(at(&pixels, 16, 16), [255, 0, 0, 255], "遮罩里的红");
    assert_eq!(at(&pixels, 48, 16), [0, 0, 0, 255], "遮罩外不画红");
    assert_eq!(at(&pixels, 16, 48), [0, 0, 0, 255], "遮罩里不画绿");
    assert_eq!(at(&pixels, 48, 48), [0, 255, 0, 255], "遮罩外的绿");
}

/// 透明的地方不算遮罩：用 circle 着色器挖成圆。
#[test]
fn mask_shapes_follow_alpha() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        world.insert_resource(ClearColor(wgpu::Color::BLACK));
        world.spawn((Sprite::color(Vec2::splat(64.0), Vec4::ONE), Transform2d::default(), Masking::Write, SpriteShader { name: "circle".into(), params: Vec4::ZERO }));
        world.spawn((Sprite::color(Vec2::splat(64.0), RED), Transform2d::default(), Masking::Inside));
    });
    assert_eq!(at(&pixels, 32, 32), [255, 0, 0, 255]);
    assert_eq!(at(&pixels, 2, 2), [0, 0, 0, 255], "圆外的角");
}

#[test]
fn lighting_darkens_lit_sprites_and_lights_brighten_them() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render_app(&gfx, |app| {
        app.add_systems(Startup, |world: &mut World| {
            world.insert_resource(ClearColor(wgpu::Color::BLACK));
            world.insert_resource(Lighting2d {
                enabled: true,
                ambient: Vec3::splat(0.25),
                lights: vec![Light2d { position: Vec2::new(16.0, 16.0), radius: 20.0, color: Vec3::ONE, intensity: 0.75, ..Default::default() }],
            });
            // 整屏白色、受光
            square(world, 0.0, 0.0, 64.0, Vec4::ONE);
            // 左下一块不受光
            world.spawn((Sprite::color(Vec2::splat(16.0), Vec4::ONE), Transform2d::from_xy(-24.0, -24.0).with_z(1.0), Unlit));
        });
        // 粒子之类的额外实例（每帧 PreUpdate 清空，Update 里推）不受光
        app.add_systems(Update, |mut extra: ResMut<SpriteInstances>| {
            extra.push(
                SpriteImage::Texture(TextureId::WHITE),
                SpriteInstance { position: Vec2::new(24.0, -24.0), size: Vec2::splat(8.0), z: 1.0, ..Default::default() },
            );
        });
    });
    // 线性 0.25 → sRGB 137
    assert!(near(at(&pixels, 2, 32), gray(137), 2), "只有环境光 {:?}", at(&pixels, 2, 32));
    // 像素中心离灯心 0.7 point：0.25 + 0.75 × (1 − 0.7 / 20)² ≈ 0.948，sRGB 249
    assert!(near(at(&pixels, 48, 16), gray(249), 2), "灯中心 {:?}", at(&pixels, 48, 16));
    assert_eq!(at(&pixels, 8, 56), gray(255), "不受光的");
    assert_eq!(at(&pixels, 56, 56), gray(255), "额外实例不受光");
}

#[test]
fn lighting_off_draws_as_is() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        world.insert_resource(Lighting2d { enabled: false, ambient: Vec3::ZERO, lights: vec![] });
        square(world, 0.0, 0.0, 64.0, Vec4::ONE);
    });
    assert_eq!(at(&pixels, 32, 32), gray(255));
}

#[test]
fn post_effects_run_in_order() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        world.insert_resource(ClearColor(wgpu::Color::BLACK));
        square(world, -16.0, 0.0, 32.0, RED);
        square(world, 16.0, 0.0, 32.0, Vec4::new(0.0, 1.0, 0.0, 1.0));
        let mut shaders = world.resource_mut::<Shaders>();
        // 自己写的：只留红色通道
        shaders.set([("red".to_owned(), "fn post(p: Post) -> vec4f {\n    return vec4f(p.color.r, 0.0, 0.0, 1.0);\n}".to_owned())].into());
        world.insert_resource(PostEffects(vec![
            // 用不了的跳过，不打乱后面的
            PostEffect { shader: "nope".into(), params: Vec4::ZERO, ..Default::default() },
            PostEffect { shader: "grayscale".into(), params: Vec4::new(1.0, 0.0, 0.0, 0.0), ..Default::default() },
            PostEffect { shader: "red".into(), params: Vec4::ZERO, ..Default::default() },
            // 第三遍（什么都不改）：中间纹理要来回换着用，不能读写同一张
            PostEffect { shader: "grayscale".into(), params: Vec4::ZERO, ..Default::default() },
        ]));
    });
    // 去色：红 → 0.2126，绿 → 0.7152（线性），再只留红色通道
    assert!(near(at(&pixels, 16, 32), [127, 0, 0, 255], 3), "{:?}", at(&pixels, 16, 32));
    assert!(near(at(&pixels, 48, 32), [221, 0, 0, 255], 3), "{:?}", at(&pixels, 48, 32));
}

#[test]
fn unknown_or_broken_post_effects_are_skipped() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        square(world, 0.0, 0.0, 64.0, RED);
        world.resource_mut::<Shaders>().set([("bad".to_owned(), "fn post(p: Post) -> vec4f { return 1; }".to_owned())].into());
        world.insert_resource(PostEffects(vec![
            PostEffect { shader: "nope".into(), params: Vec4::ZERO, ..Default::default() },
            PostEffect { shader: "bad".into(), params: Vec4::ZERO, ..Default::default() },
            // 精灵着色器不能当后处理
            PostEffect { shader: "flash".into(), params: Vec4::ZERO, ..Default::default() },
        ]));
    });
    assert_eq!(at(&pixels, 32, 32), [255, 0, 0, 255]);
}

#[test]
fn vignette_darkens_the_corners_only() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        square(world, 0.0, 0.0, 64.0, Vec4::ONE);
        world.insert_resource(PostEffects(vec![PostEffect { shader: "vignette".into(), params: Shaders::default_params("vignette"), ..Default::default() }]));
    });
    assert_eq!(at(&pixels, 32, 32), gray(255));
    assert!(at(&pixels, 0, 0)[0] < 200, "{:?}", at(&pixels, 0, 0));
}

/// 自己声明绑定的着色器 naga 能过、建管线会失败：要按原样画，别的精灵照常画（不能把整帧弄坏）。
#[test]
fn shaders_declaring_their_own_bindings_fall_back() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        let code = "@group(2) @binding(0) var<uniform> u: vec4f;\nfn effect(e: Effect) -> vec4f { return e.color * u; }";
        world.resource_mut::<Shaders>().set([("bound".to_owned(), code.to_owned())].into());
        let a = square(world, -16.0, 0.0, 32.0, RED);
        world.entity_mut(a).insert(SpriteShader { name: "bound".into(), params: Vec4::ZERO });
        square(world, 16.0, 0.0, 32.0, Vec4::new(0.0, 0.0, 1.0, 1.0));
    });
    assert_eq!(at(&pixels, 16, 32), [255, 0, 0, 255]);
    assert_eq!(at(&pixels, 48, 32), [0, 0, 255, 255]);
}

/// 背景层（z < 0）有自己的遮罩。
#[test]
fn background_layer_masks() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        world.insert_resource(ClearColor(wgpu::Color::BLACK));
        world.spawn((Sprite::color(Vec2::new(32.0, 64.0), Vec4::ONE), Transform2d::from_xy(-16.0, 0.0).with_z(-1.0), Masking::Write));
        world.spawn((Sprite::color(Vec2::splat(64.0), RED), Transform2d::default().with_z(-2.0), Masking::Inside));
    });
    assert_eq!(at(&pixels, 16, 32), [255, 0, 0, 255]);
    assert_eq!(at(&pixels, 48, 32), [0, 0, 0, 255]);
}

/// 着色器库换了代码：下一帧用新的（编过的管线和名字都丢掉重来）。
#[test]
fn changed_shaders_take_effect_next_frame() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render_frames(&gfx, 2, |app| {
        app.add_systems(Startup, |world: &mut World| {
            let e = square(world, 0.0, 0.0, 64.0, RED);
            world.entity_mut(e).insert(SpriteShader { name: "tint".into(), params: Vec4::ZERO });
        });
        // 第一帧是绿，第二帧换成蓝，名字不变
        app.add_systems(Update, |mut shaders: ResMut<Shaders>, mut frames: Local<u32>| {
            *frames += 1;
            let color = if *frames == 1 { "0.0, 1.0, 0.0" } else { "0.0, 0.0, 1.0" };
            shaders.set([("tint".to_owned(), format!("fn effect(e: Effect) -> vec4f {{ return vec4f({color}, 1.0); }}"))].into());
        });
    });
    assert_eq!(at(&pixels, 32, 32), [0, 0, 255, 255]);
}

#[test]
fn post_process_is_warmed_every_frame_even_when_inactive() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting(Arc<AtomicUsize>);
    impl nge_ecs::PostProcess for Counting {
        fn active(&self, _: &World) -> bool {
            false
        }
        fn warm(&mut self, _: &World, _: &Gfx) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        fn process(&mut self, _: &mut World, _: &Gfx, _: &mut wgpu::CommandEncoder, _: &wgpu::TextureView, _: &wgpu::TextureView) {
            unreachable!("没开后处理");
        }
    }

    let Some(gfx) = gfx() else { return };
    let warmed = Arc::new(AtomicUsize::new(0));
    let counter = warmed.clone();
    render_frames(&gfx, 3, move |app| {
        app.set_post_process(Counting(counter.clone()));
    });
    assert_eq!(warmed.load(Ordering::SeqCst), 3);
}

/// 排在精灵渲染器后面，记下这一帧到它为止的（draw 次数，实例数）。
struct CountProbe(std::sync::Arc<std::sync::Mutex<(u32, u32)>>);

impl nge_ecs::Renderer for CountProbe {
    fn render(&mut self, world: &mut World, _: &Gfx, _: &mut wgpu::CommandEncoder, _: &wgpu::TextureView) {
        *self.0.lock().unwrap() = world.resource::<nge_ecs::RenderFrame>().counts();
    }
}

#[test]
fn sprites_count_one_draw_per_batch_and_every_instance() {
    let Some(gfx) = gfx() else { return };
    let counts = std::sync::Arc::new(std::sync::Mutex::new((0, 0)));
    let probe = counts.clone();
    render_app(&gfx, move |app| {
        app.add_renderer(CountProbe(probe.clone()));
        app.add_systems(Startup, |world: &mut World| {
            // 两个同样的合一批；带着色器的另一批；z < 0 的在背景层
            square(world, -16.0, 0.0, 8.0, RED);
            square(world, 0.0, 0.0, 8.0, RED);
            let e = square(world, 16.0, 0.0, 8.0, RED);
            world.entity_mut(e).insert(SpriteShader { name: "flash".into(), params: Vec4::ONE });
            let back = square(world, 0.0, 16.0, 8.0, RED);
            world.entity_mut(back).insert(Transform2d { z: -1.0, ..Transform2d::from_xy(0.0, 16.0) });
        });
    });
    assert_eq!(*counts.lock().unwrap(), (3, 4));
}

#[test]
fn post_passes_count_as_draws() {
    let Some(gfx) = gfx() else { return };
    let draws = std::sync::Arc::new(std::sync::Mutex::new(None));
    let seen = draws.clone();
    render_frames(&gfx, 2, move |app| {
        let seen = seen.clone();
        app.add_systems(Startup, |world: &mut World| {
            square(world, 0.0, 0.0, 8.0, RED);
            world.insert_resource(PostEffects(vec![
                PostEffect { shader: "grayscale".into(), params: Vec4::ONE, ..Default::default() },
                PostEffect { shader: "vignette".into(), params: Vec4::ONE, ..Default::default() },
            ]));
        });
        app.add_systems(Update, move |perf: Res<nge_ecs::Perf>| {
            if let Some(r) = perf.report() {
                *seen.lock().unwrap() = Some(r.draws);
            }
        });
    });
    assert_eq!(*draws.lock().unwrap(), Some(3), "一个精灵加两遍后处理");
}

/// 一张 1×1 的法线图（RGB 是存着的原值，按线性数据读）。
fn normal_map(world: &mut World, n: Vec3) -> TextureId {
    let c = ((n.normalize() * 0.5 + 0.5) * 255.0 + 0.5).to_array().map(|v| v as u8);
    world.resource_mut::<Assets<Image>>().add_rgba(glam::UVec2::ONE, vec![c[0], c[1], c[2], 255]).unwrap()
}

fn lit(world: &mut World, lights: Vec<Light2d>) {
    world.insert_resource(ClearColor(wgpu::Color::BLACK));
    world.insert_resource(Lighting2d { enabled: true, ambient: Vec3::ZERO, lights });
}

fn srgb(linear: f32) -> u8 {
    let c = if linear <= 0.003_130_8 { linear * 12.92 } else { 1.055 * linear.powf(1.0 / 2.4) - 0.055 };
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// 灯在画面中间、高度 0：法线朝着灯的一侧亮、背着的黑；镜像、转 180° 的法线跟着翻；朝外的法线按灯的高度算。
#[test]
fn normal_maps_light_the_side_facing_the_light() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    // 左右各一块，`left` / `right` 是（宽，转角，法线）
    let scene = |left: (f32, f32, Vec3), right: (f32, f32, Vec3), height: f32| {
        move |world: &mut World| {
            lit(world, vec![Light2d { position: Vec2::ZERO, radius: 100.0, height, ..Default::default() }]);
            for (x, (w, rotation, n)) in [(-20.0, left), (20.0, right)] {
                let n = normal_map(world, n);
                let mut t = Transform2d::from_xy(x, 0.0);
                t.rotation = rotation;
                world.spawn((Sprite::color(Vec2::new(w, 16.0), Vec4::ONE), t, nge_render2d::NormalMap(n)));
            }
        }
    };
    // 像素 (12, 31) 的中心是世界 (−19.5, 0.5)，(51, 31) 是 (19.5, 0.5)
    let p = Vec2::new(19.5, 0.5);
    let fall = (1.0 - p.length() / 100.0).powi(2);
    let facing = srgb(fall * p.x / p.length());
    let pixels = render(&gfx, scene((16.0, 0.0, Vec3::X), (16.0, 0.0, Vec3::X), 0.0));
    assert!(near(at(&pixels, 12, 31), gray(facing), 2), "朝着灯 {:?} ≈ {facing}", at(&pixels, 12, 31));
    assert_eq!(at(&pixels, 51, 31), gray(0), "背着灯");
    let pixels = render(&gfx, scene((16.0, std::f32::consts::PI, Vec3::X), (-16.0, 0.0, Vec3::X), 0.0));
    assert_eq!(at(&pixels, 12, 31), gray(0), "转 180° 后朝左，背着灯");
    assert!(near(at(&pixels, 51, 31), gray(facing), 2), "镜像后朝左，朝着灯 {:?}", at(&pixels, 51, 31));
    let height = 20.0;
    let pixels = render(&gfx, scene((16.0, 0.0, Vec3::Z), (16.0, 0.0, Vec3::Z), height));
    let up = srgb(fall * height / p.extend(height).length());
    assert!(near(at(&pixels, 12, 31), gray(up), 2), "朝外的法线 {:?} ≈ {up}", at(&pixels, 12, 31));
}

/// 方块挡住开了 shadows 的灯：后面黑，前面、旁边照常；挡光的方块自己照常亮；没开 shadows 的灯穿过去。
#[test]
fn occluders_cast_shadows_from_lights_that_want_them() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    // `plain`：同一个位置再放一盏没开 shadows 的灯
    let scene = |shadows: bool, plain: bool| {
        move |world: &mut World| {
            let light = |shadows| Light2d { position: Vec2::new(-24.0, 0.0), radius: 200.0, shadows, ..Default::default() };
            lit(world, if plain { vec![light(true), light(false)] } else { vec![light(shadows)] });
            square(world, 0.0, 0.0, 64.0, Vec4::ONE);
            // 灯右边一根竖着的挡光块：2 × 12 放大两倍，画出来 4 × 24（多边形的写法在下面那块）
            let mut bar = Transform2d::from_xy(-8.0, 0.0).with_z(1.0);
            bar.scale = Vec2::splat(2.0);
            world.spawn((Sprite::color(Vec2::new(2.0, 12.0), Vec4::ONE), bar, nge_render2d::Occluder2d::default()));
            // 只有两个点的挡光线（没有精灵），在灯下方
            world.spawn((Transform2d::from_xy(-24.0, -16.0), nge_render2d::Occluder2d::polygon([Vec2::new(-8.0, 0.0), Vec2::new(8.0, 0.0)])));
        }
    };
    let pixels = render(&gfx, scene(true, false));
    assert_eq!(at(&pixels, 40, 31), gray(0), "挡光块后面");
    assert_eq!(at(&pixels, 40, 12), gray(0), "挡光块按放大后的大小挡");
    assert!(at(&pixels, 40, 1)[0] > 0, "斜上方擦过挡光块上沿，没被挡");
    assert!(at(&pixels, 2, 31)[0] > 0, "灯左边");
    assert!(at(&pixels, 24, 31)[0] > 0, "挡光块自己照常亮");
    assert_eq!(at(&pixels, 8, 60), gray(0), "线段下面");
    let pixels = render(&gfx, scene(false, false));
    assert!(at(&pixels, 40, 31)[0] > 0 && at(&pixels, 8, 60)[0] > 0, "没开 shadows 的灯穿过去");
    let pixels = render(&gfx, scene(true, true));
    assert!(at(&pixels, 40, 31)[0] > 0, "开了 shadows 的灯被挡，旁边没开的照常照过来");
}

/// 法线图还没加载好时当没有法线图画，不会因为白色的「法线」暗一下。
#[test]
fn normal_maps_not_loaded_yet_are_ignored() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        lit(world, vec![Light2d { position: Vec2::ZERO, radius: 100.0, ..Default::default() }]);
        world.spawn((Sprite::color(Vec2::new(16.0, 16.0), Vec4::ONE), Transform2d::from_xy(-20.0, 0.0)));
        world.spawn((Sprite::color(Vec2::new(16.0, 16.0), Vec4::ONE), Transform2d::from_xy(20.0, 0.0), nge_render2d::NormalMap(TextureId::new(999))));
    });
    assert_eq!(at(&pixels, 51, 31), at(&pixels, 12, 31), "和没有法线图的一样亮");
}

/// bloom：亮的方块往外晕开，远处和方块本身不变；低于阈值的不晕。
#[test]
fn bloom_spreads_bright_pixels_only() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let scene = |brightness: f32, bloom: bool| {
        move |world: &mut World| {
            world.insert_resource(ClearColor(wgpu::Color::BLACK));
            square(world, 0.0, 0.0, 8.0, Vec4::new(brightness, brightness, brightness, 1.0));
            if bloom {
                world.insert_resource(PostEffects(vec![PostEffect { shader: "bloom".into(), params: Vec4::new(0.5, 1.0, 4.0, 0.0), ..Default::default() }]));
            }
        }
    };
    let plain = render(&gfx, scene(1.0, false));
    let glow = render(&gfx, scene(1.0, true));
    assert_eq!(at(&plain, 40, 32), gray(0));
    assert!(at(&glow, 40, 32)[0] > 20, "方块旁边晕开 {:?}", at(&glow, 40, 32));
    assert_eq!(at(&glow, 32, 32), gray(255), "方块本身");
    assert!(at(&glow, 2, 2)[0] < at(&glow, 40, 32)[0] / 3, "远处的角暗得多 {:?}", at(&glow, 2, 2));
    let dim = render(&gfx, scene(0.3, true));
    assert_eq!(at(&dim, 40, 32), gray(0), "低于阈值不晕");
}

/// 背景层的后处理只改 z < 0 的精灵，前面的不变；只有背景层的后处理时不开整屏后处理。
#[test]
fn back_layer_post_leaves_the_front_alone() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        world.insert_resource(ClearColor(wgpu::Color::BLACK));
        world.spawn((Sprite::color(Vec2::new(32.0, 64.0), RED), Transform2d::from_xy(-16.0, 0.0).with_z(-1.0)));
        world.spawn((Sprite::color(Vec2::new(32.0, 64.0), RED), Transform2d::from_xy(16.0, 0.0)));
        world.insert_resource(PostEffects(vec![PostEffect { shader: "grayscale".into(), params: Vec4::X, layer: PostLayer::Back }]));
    });
    // 线性红 (1, 0, 0) 去色是 0.2126，sRGB ≈ 127
    assert!(near(at(&pixels, 16, 32), gray(127), 2), "背景去色 {:?}", at(&pixels, 16, 32));
    assert_eq!(at(&pixels, 48, 32), [255, 0, 0, 255], "前面的不变");
}

/// 和 3D 混排（Interleave）时，有背景层的后处理照样把 z < 0 的精灵分出来先画、过后处理。
#[test]
fn back_layer_post_works_when_interleaved() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        world.insert_resource(nge_ecs::Interleave(true));
        world.insert_resource(ClearColor(wgpu::Color::BLACK));
        world.spawn((Sprite::color(Vec2::new(32.0, 64.0), RED), Transform2d::from_xy(-16.0, 0.0).with_z(-1.0)));
        world.spawn((Sprite::color(Vec2::new(32.0, 64.0), RED), Transform2d::from_xy(16.0, 0.0)));
        world.insert_resource(PostEffects(vec![PostEffect { shader: "grayscale".into(), params: Vec4::X, layer: PostLayer::Back }]));
    });
    assert!(near(at(&pixels, 16, 32), gray(127), 2), "背景去色 {:?}", at(&pixels, 16, 32));
    assert_eq!(at(&pixels, 48, 32), [255, 0, 0, 255], "前面的不变");
}

/// 级数只改晕开的范围，不把亮度叠上去：6 级在方块旁边不比 2 级亮。
#[test]
fn bloom_levels_change_the_reach_not_the_brightness() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let glow = |levels: f32| {
        render(&gfx, move |world| {
            world.insert_resource(ClearColor(wgpu::Color::BLACK));
            square(world, 0.0, 0.0, 8.0, Vec4::ONE);
            world.insert_resource(PostEffects(vec![PostEffect { shader: "bloom".into(), params: Vec4::new(0.5, 1.0, levels, 0.0), ..Default::default() }]));
        })
    };
    let (few, many) = (at(&glow(2.0), 32, 32)[0], at(&glow(6.0), 32, 32)[0]);
    let (a, b) = (at(&glow(2.0), 38, 32)[0] as f32, at(&glow(6.0), 38, 32)[0] as f32);
    assert_eq!((few, many), (255, 255));
    // 不归一时 6 级比 2 级亮得多（各级加起来）；归一后同样的亮度摊到更大的范围，旁边不会更亮
    assert!(a > 0.0 && b > 0.0 && b <= a, "旁边的亮度 2 级 {a}，6 级 {b}");
}

/// 背景层和整屏都有后处理、整屏那串里 bloom 和普通的混着：每遍的参数各是各的（uniform 不互相覆盖）。
#[test]
fn back_and_screen_post_chains_with_bloom_keep_their_own_params() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        world.insert_resource(ClearColor(wgpu::Color::BLACK));
        world.resource_mut::<Shaders>().set([("scale".to_owned(), "fn post(p: Post) -> vec4f {\n    return vec4f(p.color.rgb * p.params.x, 1.0);\n}".to_owned())].into());
        world.spawn((Sprite::color(Vec2::new(32.0, 64.0), RED), Transform2d::from_xy(-16.0, 0.0).with_z(-1.0)));
        world.spawn((Sprite::color(Vec2::new(32.0, 64.0), RED), Transform2d::from_xy(16.0, 0.0)));
        world.insert_resource(PostEffects(vec![
            PostEffect { shader: "grayscale".into(), params: Vec4::X, layer: PostLayer::Back },
            // 阈值 2：什么都不晕，原样过
            PostEffect { shader: "bloom".into(), params: Vec4::new(2.0, 1.0, 3.0, 0.0), ..Default::default() },
            PostEffect { shader: "scale".into(), params: Vec4::new(0.5, 0.0, 0.0, 0.0), ..Default::default() },
        ]));
    });
    // 线性 0.2126 × 0.5 → sRGB ≈ 92；线性红 0.5 → sRGB 188
    assert!(near(at(&pixels, 16, 32), gray(92), 2), "背景去色再减半 {:?}", at(&pixels, 16, 32));
    assert!(near(at(&pixels, 48, 32), [188, 0, 0, 255], 2), "前面只减半 {:?}", at(&pixels, 48, 32));
}

/// 灯在方块的 45° 方向：对角线正好擦过两个角，也挡住（光不从角缝漏过去）。
#[test]
fn light_does_not_leak_through_occluder_corners() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        lit(world, vec![Light2d { position: Vec2::splat(-20.0), radius: 200.0, shadows: true, ..Default::default() }]);
        square(world, 0.0, 0.0, 64.0, Vec4::ONE);
        world.spawn((Transform2d::default(), nge_render2d::Occluder2d::polygon(nge_render2d_square(8.0))));
    });
    // 像素 (51, 12) 的中心是世界 (19.5, 19.5)，和灯的连线正好过 (−4, −4)、(4, 4) 两个角
    assert_eq!(at(&pixels, 51, 12), gray(0));
}

fn nge_render2d_square(side: f32) -> Vec<Vec2> {
    let h = side / 2.0;
    vec![Vec2::new(-h, -h), Vec2::new(h, -h), Vec2::new(h, h), Vec2::new(-h, h)]
}

/// 灯高 0、像素正好在灯心：带法线图的也照亮，不出 NaN。
#[test]
fn normal_mapped_pixel_right_under_a_flat_light_is_lit() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        // 像素 (32, 31) 的中心是世界 (0.5, 0.5)
        lit(world, vec![Light2d { position: Vec2::splat(0.5), radius: 100.0, height: 0.0, ..Default::default() }]);
        let n = normal_map(world, Vec3::Z);
        world.spawn((Sprite::color(Vec2::splat(16.0), Vec4::ONE), Transform2d::default(), nge_render2d::NormalMap(n)));
    });
    assert_eq!(at(&pixels, 32, 31), gray(255));
}

/// 没写大小、贴图还没加载好的挡光精灵先不挡（不当成 1 × 1 的小块）。
#[test]
fn occluders_without_a_size_yet_do_not_block() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        lit(world, vec![Light2d { position: Vec2::new(-24.0, 0.0), radius: 200.0, shadows: true, ..Default::default() }]);
        square(world, 0.0, 0.0, 64.0, Vec4::ONE);
        // 读不到的文件：句柄在（加载中 / 失败），一直没有尺寸
        let pending = world.resource_scope(|world, mut server: Mut<AssetServer>| server.load(&mut world.resource_mut::<Assets<Image>>(), "no-such-file.png"));
        world.spawn((Sprite::from_texture(pending), Transform2d::from_xy(-8.0, 0.0).with_z(1.0), nge_render2d::Occluder2d::default()));
    });
    assert!(at(&pixels, 40, 31)[0] > 0);
}
