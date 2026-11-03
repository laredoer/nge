//! 离屏渲染到纹理再读回像素，验证坐标系、uv 方向、z 顺序和纯色精灵。没有 GPU 时跳过。

use glam::{UVec2, Vec2, Vec4};
use nge_core::{Ctx, Emit, Gfx, Viewport, wgpu};
use nge_ecs::prelude::*;
use nge_render2d::prelude::*;

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
    let factory = nge_ecs::factory(move || {
        let setup = setup.clone();
        let mut app = App::new();
        app.add_plugins(Render2dPlugin).add_systems(Startup, move |world: &mut World| setup(world));
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
    bytemuck::cast_slice::<u8, [u8; 4]>(&data).to_vec()
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
        let texture = world.resource_mut::<Assets>().add_rgba(UVec2::splat(2), rgba).unwrap();
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
