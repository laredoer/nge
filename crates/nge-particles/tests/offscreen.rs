//! 离屏渲染粒子再读回像素。没有 GPU 时跳过。

use glam::{Vec2, Vec4};
use nge_core::{Ctx, Emit, Gfx, Viewport, wgpu};
use nge_ecs::prelude::*;
use nge_particles::prelude::*;
use nge_render2d::prelude::*;

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn render(setup: impl Fn(&mut World) + Send + Sync + 'static) -> Option<Vec<[u8; 4]>> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    let gfx = Gfx { device, queue, format: FORMAT, viewport: Viewport { size: [SIZE as f32; 2], pixels: [SIZE; 2] } };
    let setup = std::sync::Arc::new(setup);
    let mut game = nge_ecs::factory(move || {
        let setup = setup.clone();
        let mut app = App::new();
        app.add_plugins(Render2dPlugin)
            .add_plugins(ParticlePlugin)
            .insert_resource(ClearColor(wgpu::Color::BLACK))
            .add_systems(Startup, move |world: &mut World| setup(world));
        app
    })(&gfx);
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
    game.render(&gfx, &mut encoder, &texture.create_view(&Default::default()));
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE * 4), rows_per_image: None },
        },
        texture.size(),
    );
    gfx.queue.submit([encoder.finish()]);
    readback.map_async(wgpu::MapMode::Read, .., |r| r.expect("map"));
    gfx.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let data = readback.get_mapped_range(..).expect("mapped");
    Some(bytemuck::cast_slice::<u8, [u8; 4]>(&data).to_vec())
}

#[test]
fn burst_particles_are_drawn_at_the_emitter() {
    let Some(pixels) = render(|world| {
        // 不动、不变色的粒子：在 (16, 16) 处画一个 8×8 的红块
        let emitter = ParticleEmitter {
            speed: (0.0, 0.0),
            size: (8.0, 8.0),
            color: (Vec4::new(1.0, 0.0, 0.0, 1.0), Vec4::new(1.0, 0.0, 0.0, 1.0)),
            ..ParticleEmitter::burst(5)
        };
        world.spawn((emitter, Transform2d::from_xy(16.0, 16.0)));
    }) else {
        return eprintln!("没有可用 GPU，跳过");
    };
    let at = |x: u32, y: u32| pixels[(y * SIZE + x) as usize];
    let center = Vec2::new(32.0 + 16.0, 32.0 - 16.0);
    assert_eq!(at(center.x as u32, center.y as u32), [255, 0, 0, 255]);
    assert_eq!(at(center.x as u32 + 6, center.y as u32), [0, 0, 0, 255], "只有 8×8 大");
    assert_eq!(at(16, 48), [0, 0, 0, 255]);
}
