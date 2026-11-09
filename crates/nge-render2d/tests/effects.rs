//! 2D 特效的离屏渲染：着色器、遮罩、2D 光照、后处理（见 docs/effects2d.md）。没有 GPU 时跳过。

use glam::{Vec2, Vec4};
use nge_core::{Ctx, Emit, Gfx, Viewport, wgpu};
use nge_ecs::prelude::*;
use nge_render2d::prelude::*;
use nge_render2d::{Light2d, Lighting2d, Masking, PostEffect, PostEffects, Shaders, SpriteImage, SpriteInstance, SpriteInstances, SpriteShader, TextureId, Unlit};
use glam::Vec3;

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn gfx() -> Option<Gfx> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    let viewport = Viewport { size: [SIZE as f32; 2], pixels: [SIZE; 2] };
    Some(Gfx { device, queue, format: FORMAT, viewport })
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
                lights: vec![Light2d { position: Vec2::new(16.0, 16.0), radius: 20.0, color: Vec3::ONE, intensity: 0.75 }],
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
            PostEffect { shader: "nope".into(), params: Vec4::ZERO },
            PostEffect { shader: "grayscale".into(), params: Vec4::new(1.0, 0.0, 0.0, 0.0) },
            PostEffect { shader: "red".into(), params: Vec4::ZERO },
            // 第三遍（什么都不改）：中间纹理要来回换着用，不能读写同一张
            PostEffect { shader: "grayscale".into(), params: Vec4::ZERO },
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
            PostEffect { shader: "nope".into(), params: Vec4::ZERO },
            PostEffect { shader: "bad".into(), params: Vec4::ZERO },
            // 精灵着色器不能当后处理
            PostEffect { shader: "flash".into(), params: Vec4::ZERO },
        ]));
    });
    assert_eq!(at(&pixels, 32, 32), [255, 0, 0, 255]);
}

#[test]
fn vignette_darkens_the_corners_only() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        square(world, 0.0, 0.0, 64.0, Vec4::ONE);
        world.insert_resource(PostEffects(vec![PostEffect { shader: "vignette".into(), params: Shaders::default_params("vignette") }]));
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
