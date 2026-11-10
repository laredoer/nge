//! 离屏渲染文字再读回像素：字体加载完才画、世界坐标跟着相机、屏幕坐标不跟。没有 GPU 时跳过。

use std::sync::{Arc, Condvar, Mutex};

use glam::{Vec2, Vec4};
use nge_assets::{AssetPlugin, AssetServer, AssetSource, Assets, MemorySource};
use nge_core::{Ctx, Emit, Gfx, Viewport, wgpu};
use nge_ecs::prelude::*;
use nge_render2d::{Camera2d, Render2dPlugin, Transform2d};
use nge_text::prelude::*;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIZE: u32 = 128;
const INTER: &[u8] = include_bytes!("../fonts/Inter-Bold.ttf");
const FONT_URI: &str = "asset:/@t/inter.ttf";

struct Offscreen {
    gfx: Gfx,
    game: Box<dyn nge_core::Game>,
    texture: wgpu::Texture,
}

impl Offscreen {
    fn new(build: impl Fn(&mut App) + Send + Sync + 'static) -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
        let gfx = Gfx { screen: None, pipeline_cache: None, device, queue, format: FORMAT, viewport: Viewport { size: [SIZE as f32; 2], pixels: [SIZE; 2] } };
        let build = Arc::new(build);
        let game = nge_ecs::factory(move || {
            let mut app = App::new();
            build(&mut app);
            app
        })(&gfx);
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
        Some(Self { gfx, game, texture })
    }

    /// 推进一帧，读回亮像素（红色通道 > 128）的坐标。
    fn frame(&mut self) -> Vec<Vec2> {
        let gfx = &self.gfx;
        let emit: Emit = Box::new(|_, _| {});
        self.game.update(&mut Ctx::new(gfx.viewport, &emit), 0.016);
        let mut encoder = gfx.device.create_command_encoder(&Default::default());
        self.game.render(gfx, &mut encoder, &self.texture.create_view(&Default::default()));
        let readback = gfx.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(SIZE * SIZE * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE * 4), rows_per_image: None },
            },
            self.texture.size(),
        );
        gfx.queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |r| r.expect("map"));
        gfx.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let data = readback.slice(..).get_mapped_range().expect("读回");
        data.chunks_exact(4)
            .enumerate()
            .filter(|(_, p)| p[0] > 128)
            .map(|(i, _)| Vec2::new((i as u32 % SIZE) as f32, (i as u32 / SIZE) as f32))
            .collect()
    }

    /// 一直推进到画出东西为止（字体在后台加载）。
    fn frame_until_drawn(&mut self) -> Vec<Vec2> {
        for _ in 0..200 {
            let lit = self.frame();
            if !lit.is_empty() {
                return lit;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("文字一直没画出来");
    }
}

fn centroid(points: &[Vec2]) -> Vec2 {
    points.iter().copied().sum::<Vec2>() / points.len() as f32
}

/// 读文件前先等测试放行。
struct Gate {
    open: Arc<(Mutex<bool>, Condvar)>,
    inner: MemorySource,
}

impl AssetSource for Gate {
    fn read(&self, uri: &str) -> Result<Vec<u8>, String> {
        let (open, cv) = &*self.open;
        let _open = cv.wait_while(open.lock().unwrap(), |open| !*open).unwrap();
        self.inner.read(uri)
    }
}

/// 黑底，2D + 文字，加载 Inter 字体；`spawn` 摆文字和相机。
fn scene(source: Arc<dyn AssetSource>, spawn: impl Fn(&mut World, FontId) + Send + Sync + 'static) -> impl Fn(&mut App) + Send + Sync + 'static {
    let source = Mutex::new(Some(source));
    let spawn = Arc::new(spawn);
    move |app| {
        app.add_plugins(AssetPlugin::new().source(source.lock().unwrap().take().expect("只建一次")))
            .add_plugins(Render2dPlugin)
            .add_plugins(TextPlugin)
            .insert_resource(ClearColor(wgpu::Color::BLACK));
        let spawn = spawn.clone();
        app.add_systems(Startup, move |world: &mut World| {
            let font = world.resource_scope(|world, mut server: Mut<AssetServer>| server.load(&mut world.resource_mut::<Assets<Font>>(), FONT_URI));
            spawn(world, font);
        });
    }
}

fn memory() -> Arc<dyn AssetSource> {
    Arc::new(MemorySource::default().with(FONT_URI, INTER))
}

fn label(font: FontId) -> Text2d {
    Text2d::new("HH").with_font(font).with_size(24.0).with_color(Vec4::ONE)
}

#[test]
fn text_waits_for_its_font() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let source = Arc::new(Gate { open: gate.clone(), inner: MemorySource::default().with(FONT_URI, INTER) });
    let Some(mut offscreen) = Offscreen::new(scene(source, |world, font| {
        world.spawn((label(font), Transform2d::default()));
    })) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(offscreen.frame().is_empty(), "字体没加载完时不画");
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let lit = offscreen.frame_until_drawn();
    let c = centroid(&lit);
    assert!((c - Vec2::splat(SIZE as f32 / 2.0)).length() < 4.0, "居中锚点，文字中心在视图中心 {c}");
    assert!(lit.iter().all(|p| (p.x - 64.0).abs() < 30.0 && (p.y - 64.0).abs() < 20.0), "只在中间一小块");
}

#[test]
fn world_text_follows_the_camera_and_screen_text_does_not() {
    let Some(mut fixed) = Offscreen::new(scene(memory(), |world, font| {
        world.spawn((label(font), Transform2d::default()));
    })) else {
        return eprintln!("没有 GPU，跳过");
    };
    let base = centroid(&fixed.frame_until_drawn());

    let mut panned = Offscreen::new(scene(memory(), |world, font| {
        world.spawn((label(font), Transform2d::default()));
        world.spawn(Camera2d { position: Vec2::new(20.0, 10.0), ..Default::default() });
    }))
    .unwrap();
    let moved = centroid(&panned.frame_until_drawn());
    assert!((moved - base - Vec2::new(-20.0, 10.0)).length() < 1.5, "相机右移、上移，文字往左、往下 {base} → {moved}");

    let mut screen = Offscreen::new(scene(memory(), |world, font| {
        world.spawn((label(font).with_anchor(Anchor::TopLeft), Transform2d::from_xy(10.0, 10.0), ScreenText));
        world.spawn(Camera2d { position: Vec2::new(40.0, 40.0), zoom: 3.0, ..Default::default() });
    }))
    .unwrap();
    let lit = screen.frame_until_drawn();
    let min = lit.iter().fold(Vec2::splat(f32::MAX), |m, p| m.min(*p));
    assert!(min.x >= 10.0 && min.x < 16.0 && min.y >= 10.0 && min.y < 22.0, "左上角锚在 (10, 10) 附近 {min}");
}

#[test]
fn zoom_scales_world_text() {
    let Some(mut normal) = Offscreen::new(scene(memory(), |world, font| {
        world.spawn((label(font), Transform2d::default()));
    })) else {
        return eprintln!("没有 GPU，跳过");
    };
    let small = normal.frame_until_drawn().len();
    let mut zoomed = Offscreen::new(scene(memory(), |world, font| {
        world.spawn((label(font), Transform2d::default()));
        world.spawn(Camera2d { zoom: 2.0, ..Default::default() });
    }))
    .unwrap();
    let big = zoomed.frame_until_drawn().len();
    assert!(big as f32 > small as f32 * 3.0, "放大两倍，亮像素约四倍：{small} → {big}");
}

/// 排在文字渲染器后面，记下这一帧到它为止的（draw 次数，实例数）。
struct CountProbe(Arc<Mutex<(u32, u32)>>);

impl nge_ecs::Renderer for CountProbe {
    fn render(&mut self, world: &mut World, _: &Gfx, _: &mut wgpu::CommandEncoder, _: &wgpu::TextureView) {
        *self.0.lock().unwrap() = world.resource::<nge_ecs::RenderFrame>().counts();
    }
}

#[test]
fn all_text_is_one_draw() {
    let counts = Arc::new(Mutex::new((0, 0)));
    let probe = counts.clone();
    let build = scene(memory(), |world, font| {
        world.spawn((label(font), Transform2d::default()));
        world.spawn((label(font), Transform2d::from_xy(0.0, 30.0)));
    });
    let Some(mut offscreen) = Offscreen::new(move |app| {
        build(app);
        app.add_renderer(CountProbe(probe.clone()));
    }) else {
        return;
    };
    offscreen.frame_until_drawn();
    assert_eq!(*counts.lock().unwrap(), (1, 2), "两段字一次 draw");
}

/// 排在文字渲染器后面，记下所有文字框的大小。
struct BoxProbe(Arc<Mutex<Vec<Vec2>>>);

impl nge_ecs::Renderer for BoxProbe {
    fn render(&mut self, world: &mut World, _: &Gfx, _: &mut wgpu::CommandEncoder, _: &wgpu::TextureView) {
        *self.0.lock().unwrap() = world.query::<&TextBox>().iter(world).map(|b| b.0).collect();
    }
}

/// 摆一段 `HH`（只显示 `visible` 个字），相机缩放 `zoom`；返回亮像素和文字框。
fn typed(visible: Option<usize>, zoom: f32) -> Option<(Vec<Vec2>, Vec<Vec2>)> {
    let boxes = Arc::new(Mutex::new(Vec::new()));
    let probe = boxes.clone();
    let build = scene(memory(), move |world, font| {
        world.spawn((Text2d { visible, ..label(font) }, Transform2d::default()));
        world.spawn(Camera2d { zoom, ..Default::default() });
    });
    let mut offscreen = Offscreen::new(move |app| {
        build(app);
        app.add_renderer(BoxProbe(probe.clone()));
    })?;
    let lit = offscreen.frame_until_drawn();
    let boxes = boxes.lock().unwrap().clone();
    Some((lit, boxes))
}

#[test]
fn typing_hides_the_rest_and_the_box_is_written_back() {
    let Some((all, full_box)) = typed(None, 1.0) else {
        return eprintln!("没有 GPU，跳过");
    };
    let (half, half_box) = typed(Some(1), 1.0).unwrap();
    let right = |lit: &[Vec2]| lit.iter().fold(f32::MIN, |m, p| m.max(p.x));
    let left = |lit: &[Vec2]| lit.iter().fold(f32::MAX, |m, p| m.min(p.x));
    assert!((left(&half) - left(&all)).abs() < 1.0, "第一个字在原来的位置上 {} / {}", left(&half), left(&all));
    assert!(right(&half) < right(&all) - 5.0, "第二个字没画 {} / {}", right(&half), right(&all));
    assert_eq!(full_box.len(), 1);
    assert_eq!(half_box, full_box, "没显示的字照样占地方");
    let b = full_box[0];
    assert!(b.x > 20.0 && b.x < 60.0 && b.y > 20.0 && b.y < 40.0, "24 号的 HH 大约这么大 {b}");
    let (_, zoomed) = typed(None, 2.0).unwrap();
    assert!((zoomed[0] - b).length() < 0.5, "世界文字的框按世界单位，不随缩放变 {b} / {}", zoomed[0]);
}
