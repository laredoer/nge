//! 一帧在 CPU 上花多久：一个像平台跳跃那样的场景（瓦片地图、几百个刚体、带着色器的精灵、文字、2D 光照、后处理），
//! 跑几百帧，量更新和录制、提交渲染命令（CPU，不含 GPU 画的时间）各花多少。刚体热身时大多已经落地静止。
//! 没有 GPU 时跳过；默认不跑：
//! `cargo test -p nge-scene --test frame_bench -- --ignored --nocapture`（见 docs/fast-dev.md）。

use std::time::Instant;

use nge_core::{Ctx, Emit, Gfx, Viewport, wgpu};
use nge_ecs::App;
use serde_json::{Value, json};

const SIZE: u32 = 800;
const WARMUP: usize = 60;
const FRAMES: usize = 300;

fn scene() -> Value {
    let data: Vec<i32> = (0..400).map(|i| if i % 7 == 0 { -1 } else { 1 }).collect();
    let boxes = (0..300).map(|i| json!({ "id": format!("box{i}"), "x": (i % 30) * 20 - 300, "y": 40 + (i / 30) * 20, "w": 12, "body": {} }));
    let shiny = (0..100).map(|i| json!({ "id": format!("coin{i}"), "x": i * 8 - 400, "y": 300, "w": 10, "shader": "flash", "params": [1, 1, 1, 0.3] }));
    let texts: Vec<Value> = (0..10).map(|i| json!({ "id": format!("t{i}"), "text": format!("分数 {i}"), "x": 0, "y": i * 20 })).collect();
    json!({
        "physics": { "gravity": [0, -1800] },
        "tilemaps": [{ "id": "level", "texture": "plain", "tile": 16, "cols": 200, "x": -1600, "y": 0, "data": data, "solid": [1] }],
        "sprites": boxes.chain(shiny).collect::<Vec<_>>(),
        "texts": texts,
        "lighting": { "ambient": "#404060", "lights": [{ "x": 0, "y": 100, "radius": 300 }] },
        "post": [{ "shader": "vignette" }],
    })
}

fn gfx() -> Option<Gfx> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    println!("GPU：{:?}", adapter.get_info());
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    Some(Gfx { device, queue, format: wgpu::TextureFormat::Bgra8UnormSrgb, viewport: Viewport { size: [SIZE as f32; 2], pixels: [SIZE; 2] } })
}

/// 每帧记下有多少个精灵，确认场景真的加载出来了（不是在量一个空场景）。
#[derive(bevy_ecs::prelude::Resource, Default)]
struct Loaded(std::sync::Arc<std::sync::atomic::AtomicUsize>);

fn count_sprites(loaded: bevy_ecs::prelude::Res<Loaded>, sprites: bevy_ecs::prelude::Query<&nge_render2d::Sprite>) {
    loaded.0.store(sprites.iter().count(), std::sync::atomic::Ordering::Relaxed);
}

fn app(loaded: std::sync::Arc<std::sync::atomic::AtomicUsize>) -> App {
    let mut png = Vec::new();
    image::RgbaImage::new(16, 16).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let mut app = App::new();
    app.add_plugins(nge_assets::AssetPlugin::new())
        .add_plugins(nge_audio::AudioPlugin::mock())
        .add_plugins(nge_render3d::Render3dPlugin)
        .add_plugins(nge_render2d::Render2dPlugin)
        .add_plugins(nge_scene::ScenePlugin::new().texture("plain", &png))
        .insert_resource(Loaded(loaded))
        .add_systems(nge_ecs::Update, count_sprites);
    app
}

fn summary(name: &str, mut ms: Vec<f64>) {
    ms.sort_by(f64::total_cmp);
    let avg = ms.iter().sum::<f64>() / ms.len() as f64;
    println!("{name}: 平均 {avg:.2} ms，中位 {:.2}，p95 {:.2}，最长 {:.2}", ms[ms.len() / 2], ms[ms.len() * 95 / 100], ms[ms.len() - 1]);
}

#[test]
#[ignore = "量帧时间，手动跑"]
fn frame_cost() {
    let Some(gfx) = gfx() else { return eprintln!("没有 GPU，跳过") };
    let loaded = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sprites = loaded.clone();
    let mut game = nge_ecs::factory(move || app(sprites.clone()))(&gfx);
    let emit: Emit = Box::new(|_, _| {});
    game.prop(&mut Ctx::new(gfx.viewport, &emit), "scene", Some(&scene()));
    let texture = gfx.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: gfx.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let (mut update, mut render) = (Vec::new(), Vec::new());
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
            update.push((t1 - t0).as_secs_f64() * 1000.0);
            render.push((t2 - t1).as_secs_f64() * 1000.0);
        }
    }
    assert!(loaded.load(std::sync::atomic::Ordering::Relaxed) >= 400, "场景没加载出来");
    summary("更新", update);
    summary("渲染（CPU 录制 + 提交）", render);
}
