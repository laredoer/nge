//! 离屏渲染到纹理再读回像素，验证坐标系、uv 方向、z 顺序和纯色精灵。没有 GPU 时跳过。

use glam::{UVec2, Vec2, Vec4};
use nge_core::{Ctx, Emit, Gfx, Viewport, wgpu};
use nge_ecs::prelude::*;
use nge_render2d::prelude::*;
use nge_render2d::{Parallax, Repeat, SpriteImage, SpriteInstance, SpriteInstances, TextureId, TileLayer};

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

/// 同 [`render`]，再跑一帧更新读回这一帧的裁剪统计。
fn render_culled(gfx: &Gfx, setup: impl Fn(&mut World) + Send + Sync + 'static) -> (Vec<[u8; 4]>, nge_ecs::CullStats) {
    let setup = std::sync::Arc::new(setup);
    let stats = std::sync::Arc::new(std::sync::Mutex::new(None));
    let seen = stats.clone();
    let pixels = render_app(gfx, move |app| {
        let setup = setup.clone();
        app.add_systems(Startup, move |world: &mut World| setup(world));
        let seen = seen.clone();
        app.add_systems(Update, move |frame: Res<nge_ecs::RenderFrame>| *seen.lock().unwrap() = Some(frame.culled()));
    });
    let culled = stats.lock().unwrap().expect("渲染后又更新了一帧");
    (pixels, culled)
}

/// 同 [`render`]，但可以往 App 里加系统。
fn render_app(gfx: &Gfx, build: impl Fn(&mut App) + Send + Sync + 'static) -> Vec<[u8; 4]> {
    let factory = nge_ecs::factory(move || {
        let mut app = App::new();
        app.add_plugins(Render2dPlugin);
        build(&mut app);
        app
    });
    let mut game = factory(gfx);
    let emit: Emit = Box::new(|_, _| {});
    game.update(&mut Ctx::new(gfx.viewport, &emit), 0.016);

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
    let mut encoder = gfx.device.create_command_encoder(&Default::default());
    game.render(gfx, &mut encoder, &texture.create_view(&Default::default()));
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
    let pixels = bytemuck::cast_slice::<u8, [u8; 4]>(&data).to_vec();
    game.update(&mut Ctx::new(gfx.viewport, &emit), 0.016);
    pixels
}

fn at(pixels: &[[u8; 4]], x: u32, y: u32) -> [u8; 4] {
    pixels[(y * SIZE + x) as usize]
}

#[test]
fn quadrant_texture_keeps_orientation() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        // 2×2：左上红、右上绿、左下蓝、右下白
        let rgba = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255]].concat();
        let texture = world.resource_mut::<Assets<Image>>().add_rgba(UVec2::splat(2), rgba).unwrap();
        world.spawn((Sprite::from_texture(texture).with_size(Vec2::splat(SIZE as f32)), Transform2d::default()));
    });
    // 线性过滤在色块交界处会混色，取靠外的点（夹边后是纯色）
    assert_eq!(at(&pixels, 4, 4), [255, 0, 0, 255]);
    assert_eq!(at(&pixels, 60, 4), [0, 255, 0, 255]);
    assert_eq!(at(&pixels, 4, 60), [0, 0, 255, 255]);
    assert_eq!(at(&pixels, 60, 60), [255, 255, 255, 255]);
}

#[test]
fn world_y_up_and_z_order() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        world.insert_resource(ClearColor(wgpu::Color::BLACK));
        // 世界 (-16, 16) 是屏幕左上象限
        world.spawn((Sprite::color(Vec2::splat(32.0), Vec4::new(1.0, 0.0, 0.0, 1.0)), Transform2d::from_xy(-16.0, 16.0)));
        // 两个重叠在右下象限，z 大的（绿）在上面，先生成它以验证排序
        world.spawn((Sprite::color(Vec2::splat(32.0), Vec4::new(0.0, 1.0, 0.0, 1.0)), Transform2d::from_xy(16.0, -16.0).with_z(1.0)));
        world.spawn((Sprite::color(Vec2::splat(32.0), Vec4::new(0.0, 0.0, 1.0, 1.0)), Transform2d::from_xy(16.0, -16.0)));
    });
    assert_eq!(at(&pixels, 16, 16), [255, 0, 0, 255]);
    assert_eq!(at(&pixels, 48, 48), [0, 255, 0, 255]);
    assert_eq!(at(&pixels, 48, 16), [0, 0, 0, 255]);
}

#[test]
fn sprite_instances_are_drawn_with_entity_sprites() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render_app(&gfx, |app| {
        app.add_systems(Startup, |mut commands: Commands| {
            // 实体精灵：左半边绿色，z = 0
            commands.spawn((Sprite::color(Vec2::new(32.0, 64.0), Vec4::new(0.0, 1.0, 0.0, 1.0)), Transform2d::from_xy(-16.0, 0.0)));
        });
        app.add_systems(Update, |mut instances: ResMut<SpriteInstances>| {
            // 额外实例：中间一块红色，盖在绿色上面；右下角一块蓝色在下面（z 小）
            let red = SpriteInstance { position: Vec2::ZERO, size: Vec2::splat(16.0), z: 1.0, color: Vec4::new(1.0, 0.0, 0.0, 1.0), ..Default::default() };
            instances.push(SpriteImage::Texture(TextureId::WHITE), red);
            let blue = SpriteInstance { position: Vec2::new(16.0, -16.0), size: Vec2::splat(8.0), z: -1.0, color: Vec4::new(0.0, 0.0, 1.0, 1.0), ..Default::default() };
            instances.push(SpriteImage::Texture(TextureId::WHITE), blue);
        });
    });
    assert_eq!(at(&pixels, 30, 32), [255, 0, 0, 255], "红色额外实例在绿色精灵上面");
    assert_eq!(at(&pixels, 4, 32), [0, 255, 0, 255]);
    assert_eq!(at(&pixels, 48, 48), [0, 0, 255, 255]);
}

#[test]
fn parallax_sprites_are_drawn_where_drawn_says() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        world.spawn(Camera2d { position: Vec2::new(1000.0, 0.0), ..Default::default() });
        // 钉在屏幕上的（parallax 0）画在中间；同样位置的普通精灵跟着世界走，在视口外
        world.spawn((Sprite::color(Vec2::splat(16.0), Vec4::new(1.0, 0.0, 0.0, 1.0)), Transform2d::default(), Parallax(Vec2::ZERO)));
        world.spawn((Sprite::color(Vec2::splat(64.0), Vec4::new(0.0, 1.0, 0.0, 1.0)), Transform2d::default().with_z(-1.0)));
    });
    assert_eq!(at(&pixels, 32, 32), [255, 0, 0, 255]);
    assert_ne!(at(&pixels, 4, 4), [0, 255, 0, 255], "普通精灵不在画面里");
}

#[test]
fn repeated_sprites_fill_the_view_along_their_axis() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        // 相机挪到很远的地方，16 × 8 的条带横着铺满，竖着只有一行
        world.spawn(Camera2d { position: Vec2::new(5000.0, 0.0), ..Default::default() });
        world.spawn((Sprite::color(Vec2::new(16.0, 8.0), Vec4::new(1.0, 0.0, 0.0, 1.0)), Transform2d::default(), Repeat { x: true, y: false }));
    });
    for x in [1, 20, 32, 45, 62] {
        assert_eq!(at(&pixels, x, 32), [255, 0, 0, 255], "x = {x}");
    }
    assert_ne!(at(&pixels, 32, 10), [255, 0, 0, 255], "竖着不重复");
}

#[test]
fn repeated_sprites_tile_from_where_parallax_draws_them() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        // 竖直视差 0：钉在屏幕中间那一行；不按视差铺的话条带在 y = 0，远在视野下面
        world.spawn(Camera2d { position: Vec2::new(5000.0, 1000.0), ..Default::default() });
        world.spawn((
            Sprite::color(Vec2::new(16.0, 8.0), Vec4::new(1.0, 0.0, 0.0, 1.0)),
            Transform2d::default(),
            Parallax(Vec2::new(1.0, 0.0)),
            Repeat { x: true, y: false },
        ));
    });
    for x in [1, 32, 62] {
        assert_eq!(at(&pixels, x, 32), [255, 0, 0, 255], "x = {x}");
    }
}

#[test]
fn sprite_instances_are_cleared_every_frame() {
    let mut app = App::new();
    app.add_plugins(Render2dPlugin).add_systems(Update, |time: Res<Time>, mut instances: ResMut<SpriteInstances>| {
        if time.frame == 1 {
            instances.push(SpriteImage::Texture(TextureId::WHITE), SpriteInstance::default());
        }
    });
    let viewport = Viewport { size: [SIZE as f32; 2], pixels: [SIZE; 2] };
    app.tick(viewport, 0.016);
    assert_eq!(app.world().resource::<SpriteInstances>().len(), 1);
    app.tick(viewport, 0.016);
    assert!(app.world().resource::<SpriteInstances>().is_empty(), "下一帧开头清空");
}

/// 8×8 的 BC3 贴图：四个 4×4 块是红、绿、蓝、白，带三级 mipmap（红色）。
fn bc3_quadrants() -> nge_assets::Image {
    bc3_image(true, [0xF800, 0x07E0, 0x001F, 0xFFFF])
}

fn bc3_image(srgb: bool, quadrants: [u16; 4]) -> nge_assets::Image {
    use nge_assets::{BlockFormat, ktx2};
    let block = |rgb565: u16| [[0xFF, 0xFF, 0, 0, 0, 0, 0, 0], [rgb565.to_le_bytes(), rgb565.to_le_bytes(), [0; 2], [0; 2]].concat().try_into().unwrap()].concat();
    let level0 = quadrants.map(block).concat();
    let red = block(0xF800);
    let bytes = ktx2::write(Some(BlockFormat::Bc3), srgb, UVec2::splat(8), &[level0, red.clone(), red.clone(), red]);
    nge_assets::Image::decode(&bytes).unwrap()
}

fn gfx_with(features: wgpu::Features) -> Option<Gfx> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let descriptor = wgpu::DeviceDescriptor { required_features: adapter.features() & features, ..Default::default() };
    let (device, queue) = pollster::block_on(adapter.request_device(&descriptor)).ok()?;
    let viewport = Viewport { size: [SIZE as f32; 2], pixels: [SIZE; 2] };
    Some(Gfx { screen: None, pipeline_cache: None, device, queue, format: FORMAT, viewport })
}

#[test]
fn compressed_textures_draw_the_same_with_or_without_the_device_feature() {
    let Some(direct) = gfx_with(wgpu::Features::TEXTURE_COMPRESSION_BC) else { return eprintln!("没有可用 GPU，跳过") };
    let fallback = gfx_with(wgpu::Features::empty()).expect("同一个适配器");
    if !direct.device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC) {
        eprintln!("适配器不支持 BC，只测回退");
    }
    let draw = |gfx: &Gfx| {
        render(gfx, |world| {
            let texture = world.resource_mut::<Assets<Image>>().add(bc3_quadrants());
            world.spawn((Sprite::from_texture(texture).with_size(Vec2::splat(SIZE as f32)), Transform2d::default()));
        })
    };
    let (a, b) = (draw(&direct), draw(&fallback));
    for pixels in [&a, &b] {
        assert_eq!(at(pixels, 4, 4), [255, 0, 0, 255]);
        assert_eq!(at(pixels, 60, 4), [0, 255, 0, 255]);
        assert_eq!(at(pixels, 4, 60), [0, 0, 255, 255]);
        assert_eq!(at(pixels, 60, 60), [255, 255, 255, 255]);
    }
    assert!(a == b, "直接上传和回退解码画出来一样");
}

#[test]
fn unorm_compressed_textures_stay_unorm_on_the_fallback() {
    let Some(direct) = gfx_with(wgpu::Features::TEXTURE_COMPRESSION_BC) else { return eprintln!("没有可用 GPU，跳过") };
    if !direct.device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC) {
        return eprintln!("适配器不支持 BC，跳过");
    }
    let fallback = gfx_with(wgpu::Features::empty()).expect("同一个适配器");
    let draw = |gfx: &Gfx| {
        render(gfx, |world| {
            // 中灰（RGB565 0x8410）：按 UNORM 和按 sRGB 采样差很多
            let texture = world.resource_mut::<Assets<Image>>().add(bc3_image(false, [0x8410; 4]));
            world.spawn((Sprite::from_texture(texture).with_size(Vec2::splat(SIZE as f32)), Transform2d::default()));
        })
    };
    let (a, b) = (draw(&direct), draw(&fallback));
    assert!(at(&a, 32, 32)[0] > 160, "UNORM 的 0.5 写到 sRGB 目标上是 188 左右：{:?}", at(&a, 32, 32));
    assert_eq!(at(&a, 32, 32), at(&b, 32, 32), "回退解码也按 UNORM");
}

#[test]
fn the_fallback_keeps_the_mipmaps() {
    let Some(direct) = gfx_with(wgpu::Features::TEXTURE_COMPRESSION_BC) else { return eprintln!("没有可用 GPU，跳过") };
    let fallback = gfx_with(wgpu::Features::empty()).expect("同一个适配器");
    let draw = |gfx: &Gfx| {
        render(gfx, |world| {
            world.insert_resource(ClearColor(wgpu::Color::BLACK));
            // 8×8 缩成 2×2：用第 2 级（红色）；没有 mipmap 时是第 0 级四色混出来的灰
            let texture = world.resource_mut::<Assets<Image>>().add(bc3_quadrants());
            world.spawn((Sprite::from_texture(texture).with_size(Vec2::splat(2.0)), Transform2d::default()));
        })
    };
    for pixels in [draw(&direct), draw(&fallback)] {
        let p = at(&pixels, 32, 32);
        assert_eq!(p, [255, 0, 0, 255], "缩小后用的是 mipmap");
    }
}

#[test]
fn broken_blocks_on_the_fallback_draw_white_instead_of_crashing() {
    let Some(gfx) = gfx_with(wgpu::Features::empty()) else { return eprintln!("没有可用 GPU，跳过") };
    let pixels = render(&gfx, |world| {
        // texture2ddecoder 解这个 ASTC 块会 panic
        let bad = [0xb4, 0xf9, 0x74, 0x9e, 0x5d, 0xe5, 0x7e, 0x26, 0xe4, 0x3d, 0x3d, 0xc1, 0xf6, 0x19, 0x87, 0x0e];
        let bytes = nge_assets::ktx2::write(Some(nge_assets::BlockFormat::Astc4x4), true, UVec2::splat(4), &[bad.to_vec()]);
        let texture = world.resource_mut::<Assets<Image>>().add(nge_assets::Image::decode(&bytes).unwrap());
        world.spawn((Sprite::from_texture(texture).with_size(Vec2::splat(SIZE as f32)), Transform2d::default()));
    });
    assert_eq!(at(&pixels, 32, 32), [255, 255, 255, 255]);
}

/// 一行 `cols` 格、`rows` 行的瓦片层，帧号都是 0，左上角在 `origin`。
fn white_tiles(origin: Vec2, tile: f32, cols: usize, rows: usize, chunk: usize, z: f32) -> TileLayer {
    TileLayer::new(SpriteImage::Texture(TextureId::WHITE), origin, tile, cols, vec![0; cols * rows], chunk, Vec::new(), z)
}

#[test]
fn tile_layers_draw_only_the_chunks_in_view() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let (pixels, culled) = render_culled(&gfx, |world| {
        world.insert_resource(ClearColor(wgpu::Color::BLACK));
        // 1000 × 4 格、每格 8：左上角在屏幕左边，只有下半屏有瓦片
        world.spawn(white_tiles(Vec2::new(-32.0, 0.0), 8.0, 1000, 4, 32, 0.0));
    });
    assert_eq!(at(&pixels, 32, 48), [255, 255, 255, 255], "下半屏是瓦片");
    assert_eq!(at(&pixels, 32, 16), [0, 0, 0, 255], "上半屏没有");
    assert_eq!(culled.chunks, [1, 32], "视野只碰到第一块");
    // 视野 64 宽外扩一格：列 0..9，4 行
    assert_eq!(culled.tiles, 9 * 4);
}

#[test]
fn back_and_front_tile_layers_add_up() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let (pixels, culled) = render_culled(&gfx, |world| {
        world.insert_resource(ClearColor(wgpu::Color::BLACK));
        world.spawn(white_tiles(Vec2::new(-32.0, 32.0), 8.0, 8, 4, 4, -1.0));
        let mut front = white_tiles(Vec2::new(-32.0, 0.0), 8.0, 8, 4, 4, 1.0);
        // 视差 0：钉在屏幕上，相机挪开也在
        front.parallax = Some(Vec2::ZERO);
        world.spawn(front);
    });
    assert_eq!(at(&pixels, 32, 16), [255, 255, 255, 255]);
    assert_eq!(at(&pixels, 32, 48), [255, 255, 255, 255]);
    assert_eq!(culled.chunks, [4, 4]);
}

#[test]
fn sprites_out_of_view_are_culled_but_counted() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let (pixels, culled) = render_culled(&gfx, |world| {
        world.insert_resource(ClearColor(wgpu::Color::BLACK));
        world.spawn((Sprite::color(Vec2::splat(16.0), Vec4::new(1.0, 0.0, 0.0, 1.0)), Transform2d::default()));
        for i in 0..10 {
            world.spawn((Sprite::color(Vec2::splat(16.0), Vec4::ONE), Transform2d::from_xy(500.0 + i as f32 * 20.0, 0.0)));
        }
        // 转了 45° 的长条：中心在视野外，一角伸进来
        world.spawn((Sprite::color(Vec2::new(120.0, 4.0), Vec4::new(0.0, 1.0, 0.0, 1.0)), Transform2d { rotation: std::f32::consts::FRAC_PI_4, ..Transform2d::from_xy(70.0, 0.0) }));
    });
    assert_eq!(at(&pixels, 32, 32), [255, 0, 0, 255]);
    assert_eq!(culled.sprites, [2, 12]);
}

/// 一直画，直到 GPU 时间读回来（最多 15 秒）；返回最后看到的（支不支持、最近一帧）。
fn gpu_times(gfx: &Gfx) -> (bool, Option<nge_ecs::GpuTimes>) {
    gpu_times_with(gfx, |_| {})
}

/// 同 [`gpu_times`]，先对 app 做点什么（加后处理、加渲染器）。
fn gpu_times_with(gfx: &Gfx, setup: impl Fn(&mut App) + Send + Sync + 'static) -> (bool, Option<nge_ecs::GpuTimes>) {
    let seen = std::sync::Arc::new(std::sync::Mutex::new((false, None)));
    let out = seen.clone();
    let factory = nge_ecs::factory(move || {
        let mut app = App::new();
        app.add_plugins(Render2dPlugin);
        setup(&mut app);
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn((Sprite::color(Vec2::splat(16.0), Vec4::ONE), Transform2d::default()));
        });
        let out = out.clone();
        app.add_systems(Update, move |clock: Res<nge_ecs::GpuClock>| *out.lock().unwrap() = (clock.supported(), clock.last().cloned()));
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    for frame in 0.. {
        game.update(&mut Ctx::new(gfx.viewport, &emit), 0.016);
        let got = seen.lock().unwrap().clone();
        // 不支持的画几帧就够了
        if got.1.is_some() || (!got.0 && frame > 3) || std::time::Instant::now() > deadline {
            return got;
        }
        let mut encoder = gfx.device.create_command_encoder(&Default::default());
        game.render(gfx, &mut encoder, &view);
        gfx.queue.submit([encoder.finish()]);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    unreachable!()
}

#[test]
fn gpu_time_is_read_back_without_waiting_when_timestamps_are_supported() {
    let Some(gfx) = gfx_with(wgpu::Features::TIMESTAMP_QUERY) else { return eprintln!("没有可用 GPU，跳过") };
    if !gfx.device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        return eprintln!("GPU 不支持时间戳，跳过");
    }
    let (supported, times) = gpu_times(&gfx);
    assert!(supported);
    let times = times.expect("15 秒内读回来了");
    assert_eq!(times.passes.iter().map(|p| p.0).collect::<Vec<_>>(), ["精灵"]);
    assert!(times.total_ms >= 0.0 && times.total_ms < 1000.0, "{times:?}");
}

/// 两个后处理里一个没有这个着色器（跳过）：只量真的画了的那一遍，没有没写过的查询。
#[test]
fn only_post_passes_that_ran_are_timed() {
    let Some(gfx) = gfx_with(wgpu::Features::TIMESTAMP_QUERY) else { return eprintln!("没有可用 GPU，跳过") };
    if !gfx.device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        return eprintln!("GPU 不支持时间戳，跳过");
    }
    let (_, times) = gpu_times_with(&gfx, |app| {
        let effect = |shader: &str| nge_render2d::PostEffect { shader: shader.into(), params: Vec4::ZERO, layer: nge_render2d::PostLayer::Screen };
        app.insert_resource(nge_render2d::PostEffects(vec![effect("no-such-shader"), effect("vignette")]));
    });
    let times = times.expect("15 秒内读回来了");
    assert_eq!(times.passes.iter().map(|p| p.0).collect::<Vec<_>>(), ["精灵", "后处理"], "{times:?}");
    assert!(!times.truncated);
}

/// 一帧开的 pass 比上限多：多出的不量，记成不全。
struct ManyPasses;

impl nge_ecs::Renderer for ManyPasses {
    fn render(&mut self, world: &mut World, _gfx: &Gfx, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        for _ in 0..nge_ecs::MAX_PASSES + 6 {
            let timer = nge_ecs::pass_timer(world, "空");
            let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("空"),
                timestamp_writes: timer.as_ref().and_then(nge_ecs::PassTimer::writes),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                ..Default::default()
            });
        }
    }
}

#[test]
fn passes_over_the_limit_are_dropped_and_flagged() {
    let Some(gfx) = gfx_with(wgpu::Features::TIMESTAMP_QUERY) else { return eprintln!("没有可用 GPU，跳过") };
    if !gfx.device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        return eprintln!("GPU 不支持时间戳，跳过");
    }
    let (_, times) = gpu_times_with(&gfx, |app| {
        app.add_renderer(ManyPasses);
    });
    let times = times.expect("15 秒内读回来了");
    assert_eq!(times.passes.len(), nge_ecs::MAX_PASSES as usize);
    assert!(times.truncated);
}

#[test]
fn without_timestamps_gpu_time_is_unsupported_and_drawing_still_works() {
    let Some(gfx) = gfx() else { return eprintln!("没有可用 GPU，跳过") };
    let (supported, times) = gpu_times(&gfx);
    assert!(!supported && times.is_none());
}
