//! 大地图的性能回归（见 docs/big-scene-perf.md）：2000 × 200 格的地图，镜头每帧往右挪，
//! 量一整帧在 CPU 上花多久（更新 + 精灵渲染器录制、提交命令；不含 GPU 画的时间），1280 × 720。
//! 跑一串帧取中位数，阈值放得很宽，只拦住退回整张遍历这种量级的退化。没有 GPU 时跳过。

use std::time::{Duration, Instant};

use bevy_ecs::prelude::*;
use nge_core::{Ctx, Emit, Gfx, Viewport, wgpu};
use nge_ecs::App;
use serde_json::json;

const COLS: usize = 2000;
const ROWS: usize = 200;
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const WARMUP: usize = 30;
const FRAMES: usize = 120;
/// 镜头移动时一帧（更新 + 渲染的 CPU）的中位数。
const MOVE_LIMIT: Duration = Duration::from_millis(5);

fn big_map() -> Vec<i32> {
    let mut d = vec![-1; COLS * ROWS];
    for c in 0..COLS {
        let ground = 120 + (c * 7 % 23);
        for r in ground..ROWS {
            d[r * COLS + c] = 0;
        }
        if c % 37 < 6 {
            d[(ground - 8) * COLS + c] = 1;
        }
    }
    d
}

fn gfx() -> Option<Gfx> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    let viewport = Viewport { size: [WIDTH as f32, HEIGHT as f32], pixels: [WIDTH, HEIGHT] };
    Some(Gfx { screen: None, pipeline_cache: None, device, queue, format: wgpu::TextureFormat::Bgra8UnormSrgb, viewport })
}

/// 镜头每帧往右挪 8，停在地面附近。
fn move_camera(mut frame: Local<u32>, mut cameras: Query<&mut nge_render2d::Camera2d>) {
    *frame += 1;
    for mut c in &mut cameras {
        c.position = glam::Vec2::new(700.0 + *frame as f32 * 8.0, -2000.0);
    }
}

fn app() -> App {
    let mut png = Vec::new();
    image::RgbaImage::new(16, 16).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let mut app = App::new();
    app.add_plugins(nge_assets::AssetPlugin::new())
        .add_plugins(nge_audio::AudioPlugin::mock())
        .add_plugins(nge_render3d::Render3dPlugin)
        .add_plugins(nge_render2d::Render2dPlugin)
        .add_plugins(nge_scene::ScenePlugin::new().texture("plain", &png))
        .add_systems(nge_ecs::Update, move_camera);
    app
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

#[test]
fn moving_the_camera_over_a_huge_map_stays_cheap() {
    let Some(gfx) = gfx() else { return eprintln!("没有 GPU，跳过") };
    let mut game = nge_ecs::factory(app)(&gfx);
    let emit: Emit = Box::new(|_, _| {});
    let scene = json!({ "tilemaps": [{ "id": "big", "texture": "plain", "tile": 16, "cols": COLS, "x": 0, "y": 0, "data": big_map(), "solid": [0], "oneWay": [1] }] });
    game.prop(&mut Ctx::new(gfx.viewport, &emit), "scene", Some(&scene));
    let texture = gfx.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: gfx.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let (mut update, mut render, mut whole) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..WARMUP + FRAMES {
        let t0 = Instant::now();
        game.update(&mut Ctx::new(gfx.viewport, &emit), 1.0 / 60.0);
        let t1 = Instant::now();
        let mut encoder = gfx.device.create_command_encoder(&Default::default());
        game.render(&gfx, &mut encoder, &view);
        gfx.queue.submit([encoder.finish()]);
        let t2 = Instant::now();
        // 等 GPU 画完，不让命令越积越多
        gfx.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        if i >= WARMUP {
            update.push(t1 - t0);
            render.push(t2 - t1);
            whole.push(t2 - t0);
        }
    }
    let (u, r, w) = (median(update), median(render), median(whole));
    eprintln!("镜头移动的一帧（中位数）：更新 {u:?}，渲染 {r:?}，合计 {w:?}");
    assert!(w < MOVE_LIMIT, "镜头移动的一帧 {w:?}");
}
