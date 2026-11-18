//! 离屏渲染到纹理再读回像素，验证贴图方向、深度遮挡、光照、alpha 裁剪、2D 叠加和 glTF。没有 GPU 时跳过。
//!
//! 设了 `NGE_DUMP_DIR` 时，把 glTF 示例场景另存成 PNG，方便人工核对。

use glam::{Quat, UVec2, Vec2, Vec3, Vec4};
use nge_core::{Ctx, Emit, Gfx, Viewport, wgpu};
use nge_ecs::prelude::*;
use nge_render3d::prelude::*;
use nge_render3d::{AssetPlugin, AssetSource, Gizmos3d, ImageId, MemorySource, Shadows};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIZE: u32 = 64;

fn gfx(size: u32) -> Option<Gfx> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    let viewport = Viewport { size: [size as f32; 2], pixels: [size; 2] };
    Some(Gfx { screen: None, pipeline_cache: None, device, queue, format: FORMAT, viewport })
}

struct Pixels {
    size: u32,
    pixels: Vec<[u8; 4]>,
}

impl Pixels {
    /// (x, y)：左上角原点。
    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        self.pixels[(y * self.size + x) as usize]
    }

    fn center(&self) -> [u8; 4] {
        self.at(self.size / 2, self.size / 2)
    }
}

fn close(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 3)
}

/// 跑一帧并读回 RGBA 像素。`build` 加插件和系统；没有 GPU 时返回 None。
fn render(size: u32, build: impl Fn(&mut App) + Send + Sync + 'static) -> Option<Pixels> {
    render_frames(size, 1, build)
}

/// 连续跑 `frames` 帧，画到同一张纹理上，读回最后一帧。
fn render_frames(size: u32, frames: u32, build: impl Fn(&mut App) + Send + Sync + 'static) -> Option<Pixels> {
    let mut offscreen = Offscreen::new(size, build)?;
    (1..frames).for_each(|_| offscreen.frame(false).map_or((), drop));
    offscreen.frame(true)
}

/// 一个跑在离屏纹理上的游戏，可以一帧一帧地推进。
struct Offscreen {
    gfx: Gfx,
    game: Box<dyn nge_core::Game>,
    texture: wgpu::Texture,
    size: u32,
}

impl Offscreen {
    fn new(size: u32, build: impl Fn(&mut App) + Send + Sync + 'static) -> Option<Self> {
        let gfx = gfx(size)?;
        let build = std::sync::Arc::new(build);
        let factory = nge_ecs::factory(move || {
            let mut app = App::new();
            build(&mut app);
            app
        });
        let game = factory(&gfx);
        let texture = gfx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Some(Self { gfx, game, texture, size })
    }

    /// 推进一帧并画出来；`read` 为真时读回像素。
    fn frame(&mut self, read: bool) -> Option<Pixels> {
        let (gfx, size) = (&self.gfx, self.size);
        let emit: Emit = Box::new(|_, _| {});
        self.game.update(&mut Ctx::new(gfx.viewport, &emit), 0.016);
        let mut encoder = gfx.device.create_command_encoder(&Default::default());
        self.game.render(gfx, &mut encoder, &self.texture.create_view(&Default::default()));
        if !read {
            gfx.queue.submit([encoder.finish()]);
            return None;
        }
        let readback = gfx.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(size * size * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * 4), rows_per_image: None },
            },
            self.texture.size(),
        );
        gfx.queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |r| r.expect("map"));
        gfx.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let pixels = readback.slice(..).get_mapped_range().expect("读回").chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect();
        Some(Pixels { size, pixels })
    }
}

/// 只开亮度 1 的环境光：画出来的颜色就是材质 × 贴图的颜色。
fn unlit(app: &mut App) {
    app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 1.0 });
    app.insert_resource(ClearColor(wgpu::Color::BLACK));
}

fn solid(world: &mut World, color: Vec4, mesh: Mesh, transform: Transform3d) {
    let mut assets = world.resource_mut::<Assets3d>();
    let mesh = assets.add_mesh(mesh).unwrap();
    let material = assets.add_material(Material { base_color: color, ..Default::default() });
    world.spawn((Mesh3d { mesh, material }, transform));
}

/// 朝 +Z（对着默认相机）的平面。
fn facing_camera() -> Transform3d {
    Transform3d { rotation: Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), ..Default::default() }
}

#[test]
fn texture_keeps_orientation() {
    let Some(img) = render(SIZE, |app| {
        unlit(app);
        app.add_systems(Startup, |world: &mut World| {
            // 8×8 贴图，四个 4×4 的色块：左上红、右上绿、左下蓝、右下白
            let rgba = (0..64)
                .flat_map(|i| match (i % 8 < 4, i / 8 < 4) {
                    (true, true) => [255, 0, 0, 255],
                    (false, true) => [0, 255, 0, 255],
                    (true, false) => [0, 0, 255, 255],
                    (false, false) => [255, 255, 255, 255],
                })
                .collect();
            let image = world.resource_mut::<Assets<Image>>().add_rgba(UVec2::splat(8), rgba).unwrap();
            let mut assets = world.resource_mut::<Assets3d>();
            let mesh = assets.add_mesh(Mesh::plane(2.0)).unwrap();
            let material = assets.add_material(Material { base_color_texture: Some(image), ..Default::default() });
            world.spawn((Mesh3d { mesh, material }, facing_camera()));
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let c = SIZE / 2;
    assert!(close(img.at(c - 5, c - 5), [255, 0, 0, 255]), "左上 {:?}", img.at(c - 5, c - 5));
    assert!(close(img.at(c + 5, c - 5), [0, 255, 0, 255]), "右上 {:?}", img.at(c + 5, c - 5));
    assert!(close(img.at(c - 5, c + 5), [0, 0, 255, 255]), "左下 {:?}", img.at(c - 5, c + 5));
    assert!(close(img.at(c + 5, c + 5), [255, 255, 255, 255]), "右下 {:?}", img.at(c + 5, c + 5));
    assert!(close(img.at(1, 1), [0, 0, 0, 255]), "背景是清屏色");
}

#[test]
fn depth_hides_far_objects_in_any_draw_order() {
    for near_first in [true, false] {
        let Some(img) = render(SIZE, move |app| {
            unlit(app);
            app.add_systems(Startup, move |world: &mut World| {
                let near = |w: &mut World| solid(w, Vec4::new(1.0, 0.0, 0.0, 1.0), Mesh::cube(1.0), Transform3d::from_xyz(0.0, 0.0, 1.0));
                let far = |w: &mut World| solid(w, Vec4::new(0.0, 1.0, 0.0, 1.0), Mesh::cube(3.0), Transform3d::from_xyz(0.0, 0.0, -2.0));
                if near_first {
                    near(world);
                    far(world);
                } else {
                    far(world);
                    near(world);
                }
            });
        }) else {
            return eprintln!("没有 GPU，跳过");
        };
        assert!(close(img.center(), [255, 0, 0, 255]), "near_first={near_first}: {:?}", img.center());
        // 近处方块覆盖中心 ±8px，远处的覆盖 ±15px
        assert!(close(img.at(SIZE / 2, 20), [0, 255, 0, 255]), "远处的大方块露出来的部分 {:?}", img.at(SIZE / 2, 20));
    }
}

#[test]
fn directional_light_shades_the_lit_side() {
    let Some(img) = render(SIZE, |app| {
        app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.0 });
        app.insert_resource(ClearColor(wgpu::Color::BLUE));
        app.add_systems(Startup, |world: &mut World| {
            solid(world, Vec4::ONE, Mesh::sphere(1.5, 32), Transform3d::default());
            // 光从 +X 照过来
            world.spawn(DirectionalLight { direction: Vec3::NEG_X, color: Vec3::ONE, illuminance: 3.0, shadows: false });
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let c = SIZE / 2;
    let (lit, dark) = (img.at(c + 8, c), img.at(c - 8, c));
    assert!(lit[0] > 150, "向光面 {lit:?}");
    assert!(dark[0] < 10 && dark[2] < 10, "背光面应该是黑的（不是背景蓝）{dark:?}");
}

#[test]
fn point_light_falls_off_with_distance() {
    let Some(img) = render(SIZE, |app| {
        app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.0 });
        app.add_systems(Startup, |world: &mut World| {
            solid(world, Vec4::ONE, Mesh::plane(4.0), facing_camera());
            world.spawn((PointLight { intensity: 2.0, range: 10.0, ..Default::default() }, Transform3d::from_xyz(-1.0, 0.0, 0.5)));
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let c = SIZE / 2;
    let (near, far) = (img.at(c - 6, c), img.at(c + 10, c));
    assert!(near[0] > far[0] + 30, "靠近灯 {near:?}，远离灯 {far:?}");
}

#[test]
fn alpha_mask_discards_transparent_texels() {
    let Some(img) = render(SIZE, |app| {
        unlit(app);
        app.add_systems(Startup, |world: &mut World| {
            // 左半透明、右半不透明的白色贴图
            let rgba = (0..16).flat_map(|i| [255, 255, 255, if i % 4 < 2 { 0 } else { 255 }]).collect();
            let image = world.resource_mut::<Assets<Image>>().add_rgba(UVec2::splat(4), rgba).unwrap();
            let mut assets = world.resource_mut::<Assets3d>();
            let mesh = assets.add_mesh(Mesh::plane(2.0)).unwrap();
            let material = assets.add_material(Material { base_color_texture: Some(image), alpha_cutoff: Some(0.5), ..Default::default() });
            world.spawn((Mesh3d { mesh, material }, facing_camera()));
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let c = SIZE / 2;
    assert!(close(img.at(c - 6, c), [0, 0, 0, 255]), "透明部分露出背景 {:?}", img.at(c - 6, c));
    assert!(close(img.at(c + 6, c), [255, 255, 255, 255]), "{:?}", img.at(c + 6, c));
}

#[test]
fn back_faces_are_culled_unless_double_sided() {
    for double_sided in [false, true] {
        let Some(img) = render(SIZE, move |app| {
            unlit(app);
            app.add_systems(Startup, move |world: &mut World| {
                let mut assets = world.resource_mut::<Assets3d>();
                let mesh = assets.add_mesh(Mesh::plane(2.0)).unwrap();
                let material = assets.add_material(Material { double_sided, ..Default::default() });
                // 背面对着相机
                let back = Transform3d { rotation: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2), ..Default::default() };
                world.spawn((Mesh3d { mesh, material }, back));
            });
        }) else {
            return eprintln!("没有 GPU，跳过");
        };
        let expected = if double_sided { [255, 255, 255, 255] } else { [0, 0, 0, 255] };
        assert!(close(img.center(), expected), "double_sided={double_sided}: {:?}", img.center());
    }
}

#[test]
fn sprites_draw_over_3d_without_clearing_it() {
    use nge_render2d::{Render2dPlugin, Sprite, Transform2d};
    let Some(img) = render(SIZE, |app| {
        unlit(app);
        app.add_plugins(Render2dPlugin);
        app.add_systems(Startup, |world: &mut World| {
            solid(world, Vec4::new(1.0, 0.0, 0.0, 1.0), Mesh::cube(4.0), Transform3d::default());
            world.spawn((Sprite::color(Vec2::splat(10.0), Vec4::new(0.0, 1.0, 0.0, 1.0)), Transform2d::default()));
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(close(img.center(), [0, 255, 0, 255]), "精灵在上面 {:?}", img.center());
    assert!(close(img.at(SIZE / 2, 16), [255, 0, 0, 255]), "3D 画面没被 2D 清掉 {:?}", img.at(SIZE / 2, 16));
}

#[test]
fn background_sprites_draw_under_3d() {
    use nge_render2d::{Render2dPlugin, Sprite, Transform2d};
    let Some(img) = render(SIZE, |app| {
        unlit(app);
        app.add_plugins(Render2dPlugin);
        app.add_systems(Startup, |world: &mut World| {
            solid(world, Vec4::new(1.0, 0.0, 0.0, 1.0), Mesh::cube(1.0), Transform3d::default());
            // 铺满画面的背景（z < 0）和一个在上面的小方块
            world.spawn((Sprite::color(Vec2::splat(1000.0), Vec4::new(0.0, 0.0, 1.0, 1.0)), Transform2d::default().with_z(-1.0)));
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(close(img.center(), [255, 0, 0, 255]), "模型盖住背景 {:?}", img.center());
    assert!(close(img.at(2, 2), [0, 0, 255, 255]), "模型外面是背景 {:?}", img.at(2, 2));
}

#[test]
fn gltf_model_renders_with_texture() {
    let size = 256;
    let Some(img) = render(size, |app| {
        app.add_plugins(Render3dPlugin).insert_resource(ClearColor(wgpu::Color { r: 0.02, g: 0.03, b: 0.06, a: 1.0 }));
        app.add_systems(Startup, |world: &mut World| {
            let model = world.resource_scope(|world, mut assets: Mut<Assets3d>| {
                assets.load_gltf(&mut world.resource_mut::<Assets<Image>>(), include_bytes!("../../../examples/model/assets/crate.glb")).unwrap()
            });
            let mut assets = world.resource_mut::<Assets3d>();
            let ground = assets.add_mesh(Mesh::plane(6.0)).unwrap();
            let grass = assets.add_material(Material { base_color: Vec4::new(0.2, 0.35, 0.2, 1.0), roughness: 0.9, ..Default::default() });
            world.spawn((Model3d(model), Transform3d { rotation: Quat::from_rotation_y(0.5), ..Transform3d::from_xyz(0.0, 0.5, 0.0) }));
            world.spawn((Mesh3d { mesh: ground, material: grass }, Transform3d::default()));
            world.spawn((Camera3d::default(), Transform3d::from_xyz(2.5, 2.2, 3.5).looking_at(Vec3::new(0.0, 0.6, 0.0), Vec3::Y)));
            world.spawn(DirectionalLight { direction: Vec3::new(-1.0, -2.0, -1.5), ..Default::default() });
            world.spawn((PointLight { color: Vec3::new(1.0, 0.8, 0.5), intensity: 6.0, range: 8.0, shadows: false }, Transform3d::from_xyz(-1.5, 1.5, 1.5)));
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    if let Ok(dir) = std::env::var("NGE_DUMP_DIR") {
        let rgba: Vec<u8> = img.pixels.iter().flatten().copied().collect();
        let path = std::path::Path::new(&dir).join("gltf_crate.png");
        image::save_buffer(&path, &rgba, size, size, image::ExtendedColorType::Rgba8).expect("写 PNG");
        eprintln!("已写出 {}", path.display());
    }
    // 箱子在画面中间：既不是背景色，也不是纯色（有砖纹）
    let c = size / 2;
    let samples: Vec<[u8; 4]> = (0..20).map(|i| img.at(c - 20 + i * 2, c - 10)).collect();
    assert!(samples.iter().all(|p| !close(*p, img.at(2, 2))), "中间应该是模型 {samples:?}");
    let distinct = samples.iter().map(|p| p[0] / 16).collect::<std::collections::HashSet<_>>().len();
    assert!(distinct >= 3, "应该能看到贴图纹理 {samples:?}");
}

#[test]
fn missing_texture_and_material_fall_back() {
    let Some(img) = render(SIZE, |app| {
        unlit(app);
        app.add_systems(Startup, |world: &mut World| {
            // 从另一份资源库拿编号：在这个世界的资源库里都不存在
            let mut other = Assets3d::default();
            let red = Material { base_color: Vec4::new(1.0, 0.0, 0.0, 1.0), ..Default::default() };
            let ghost_image = Assets::<Image>::with_white().add_rgba(UVec2::ONE, vec![255, 0, 0, 255]).unwrap();
            let ghost_material = (0..4).map(|_| other.add_material(red)).last().unwrap();

            let mut assets = world.resource_mut::<Assets3d>();
            let mesh = assets.add_mesh(Mesh::cube(2.0)).unwrap();
            let material = assets.add_material(Material { base_color_texture: Some(ghost_image), ..Default::default() });
            world.spawn((Mesh3d { mesh, material }, Transform3d::from_xyz(0.6, 0.0, 0.0)));
            world.spawn((Mesh3d { mesh, material: ghost_material }, Transform3d::from_xyz(-0.6, 0.0, 0.0)));
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    // 两边都退回白色：不存在的贴图用白贴图，不存在的材质用默认材质
    let c = SIZE / 2;
    for x in [c - 12, c + 12] {
        assert!(close(img.at(x, c), [255, 255, 255, 255]), "x={x}: {:?}", img.at(x, c));
    }
}

#[test]
fn every_frame_clears_before_drawing() {
    let Some(img) = render_frames(SIZE, 2, |app| {
        unlit(app);
        app.add_systems(Startup, |world: &mut World| solid(world, Vec4::ONE, Mesh::cube(2.0), Transform3d::default()));
        // 第二帧把立方体删掉：不清屏的话还会留着上一帧的白色
        app.add_systems(Update, |mut commands: Commands, q: Query<Entity, With<Mesh3d>>, mut frame: Local<u32>| {
            *frame += 1;
            if *frame == 2 {
                q.iter().for_each(|e| commands.entity(e).despawn());
            }
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(close(img.center(), [0, 0, 0, 255]), "{:?}", img.center());
}

#[test]
fn tiny_objects_still_draw() {
    let Some(img) = render(SIZE, |app| {
        unlit(app);
        app.add_systems(Startup, |world: &mut World| {
            // 缩放 0.002，行列式约 1e-8；相机贴得很近
            let tiny = Transform3d { scale: Vec3::splat(0.002), ..facing_camera() };
            solid(world, Vec4::ONE, Mesh::plane(2.0), tiny);
            world.spawn((Camera3d { near: 1e-4, ..Default::default() }, Transform3d::from_xyz(0.0, 0.0, 0.005)));
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(close(img.center(), [255, 255, 255, 255]), "{:?}", img.center());
}

#[test]
fn mirrored_objects_keep_their_outside() {
    for double_sided in [false, true] {
        let Some(img) = render(SIZE, move |app| {
            app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.0 });
            app.insert_resource(ClearColor(wgpu::Color::BLUE));
            app.add_systems(Startup, move |world: &mut World| {
                let mut assets = world.resource_mut::<Assets3d>();
                let mesh = assets.add_mesh(Mesh::cube(2.0)).unwrap();
                let material = assets.add_material(Material { double_sided, ..Default::default() });
                world.spawn((Mesh3d { mesh, material }, Transform3d { scale: Vec3::new(-1.0, 1.0, 1.0), ..Default::default() }));
                // 光从相机这边照过去：外表面亮，看到的若是内壁就是黑的
                world.spawn(DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, illuminance: 3.0, shadows: false });
            });
        }) else {
            return eprintln!("没有 GPU，跳过");
        };
        let c = img.center();
        assert!(c[0] > 150 && c[2] > 150, "double_sided={double_sided}: {c:?}");
    }
}

/// 读文件前先等测试放行，用来稳定地看到「加载中」的那一帧。
struct Gate {
    open: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    inner: MemorySource,
}

impl AssetSource for Gate {
    fn read(&self, uri: &str) -> Result<Vec<u8>, String> {
        let (open, cv) = &*self.open;
        let _open = cv.wait_while(open.lock().unwrap(), |open| !*open).unwrap();
        self.inner.read(uri)
    }
}

fn red_png() -> Vec<u8> {
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    png
}

#[test]
fn loaded_images_are_shared_by_2d_and_3d() {
    let gate = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let source = Gate { open: gate.clone(), inner: MemorySource::default().with("asset:/@t/red.png", red_png()) };
    let source = std::sync::Mutex::new(Some(std::sync::Arc::new(source) as std::sync::Arc<dyn AssetSource>));
    let Some(mut offscreen) = Offscreen::new(SIZE, move |app| {
        app.add_plugins(AssetPlugin::new().source(source.lock().unwrap().take().expect("只建一次")));
        unlit(app);
        app.add_plugins(nge_render2d::Render2dPlugin);
        app.add_systems(Startup, |world: &mut World| {
            let red = world.resource_scope(|world, mut server: Mut<AssetServer>| server.load(&mut world.resource_mut::<Assets<Image>>(), "asset:/@t/red.png"));
            // 左边一个 3D 平面、右边一个精灵，用同一张图
            let mut assets = world.resource_mut::<Assets3d>();
            let mesh = assets.add_mesh(Mesh::plane(1.0)).unwrap();
            let material = assets.add_material(Material { base_color_texture: Some(red), ..Default::default() });
            world.spawn((Mesh3d { mesh, material }, Transform3d { translation: Vec3::new(-1.0, 0.0, 0.0), ..facing_camera() }));
            world.spawn((nge_render2d::Sprite::from_texture(red).with_size(Vec2::splat(16.0)), nge_render2d::Transform2d::from_xy(16.0, 0.0)));
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let c = SIZE / 2;
    let (mesh_at, sprite_at) = ((c - 14, c), (c + 16, c));
    let loading = offscreen.frame(true).unwrap();
    assert!(close(loading.at(mesh_at.0, mesh_at.1), [255, 255, 255, 255]), "加载中的 3D 贴图是白色 {:?}", loading.at(mesh_at.0, mesh_at.1));
    assert!(close(loading.at(sprite_at.0, sprite_at.1), [255, 255, 255, 255]), "加载中的精灵是白色 {:?}", loading.at(sprite_at.0, sprite_at.1));

    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let mut loaded = None;
    // 按墙钟等后台线程读完：机器忙时多画几帧也等得到
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        let img = offscreen.frame(true).unwrap();
        if close(img.at(sprite_at.0, sprite_at.1), [255, 0, 0, 255]) {
            loaded = Some(img);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let loaded = loaded.expect("贴图一直没加载出来");
    assert!(close(loaded.at(mesh_at.0, mesh_at.1), [255, 0, 0, 255]), "3D 也换成了红色 {:?}", loaded.at(mesh_at.0, mesh_at.1));
}

#[test]
fn ortho_camera_shows_the_same_size_at_any_distance() {
    for distance in [3.0, 300.0] {
        let Some(img) = render(SIZE, move |app| {
            unlit(app);
            app.add_systems(Startup, move |world: &mut World| {
                // 竖直方向看到 2 个单位：边长 1 的平面占画面中间一半
                world.spawn((Camera3d { ortho: Some(2.0), ..Default::default() }, Transform3d::from_xyz(0.0, 0.0, distance)));
                solid(world, Vec4::ONE, Mesh::plane(1.0), facing_camera());
            });
        }) else {
            return eprintln!("没有 GPU，跳过");
        };
        let q = SIZE / 4;
        assert!(close(img.at(q + 2, SIZE / 2), [255; 4]), "距离 {distance}：平面里面 {:?}", img.at(q + 2, SIZE / 2));
        assert!(close(img.at(q - 2, SIZE / 2), [0, 0, 0, 255]), "距离 {distance}：平面外面 {:?}", img.at(q - 2, SIZE / 2));
    }
}

#[test]
fn ortho_highlight_is_the_same_everywhere_on_a_flat_surface() {
    let Some(img) = render(SIZE, |app| {
        app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.0 });
        app.add_systems(Startup, |world: &mut World| {
            world.spawn((Camera3d { ortho: Some(2.0), ..Default::default() }, Transform3d::from_xyz(0.0, 0.0, 1.0)));
            let mut assets = world.resource_mut::<Assets3d>();
            let mesh = assets.add_mesh(Mesh::plane(4.0)).unwrap();
            let material = assets.add_material(Material { base_color: Vec4::ONE, metallic: 1.0, roughness: 0.1, ..Default::default() });
            world.spawn((Mesh3d { mesh, material }, facing_camera()));
            // 光从相机那边正对着平面照：正交相机的视线处处一样，高光也处处一样
            world.spawn(DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, illuminance: 1.0, shadows: false });
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let (center, corner) = (img.center(), img.at(4, 4));
    assert!(center[0] > 100, "正对着有高光 {center:?}");
    assert!(close(center, corner), "中间 {center:?} 和角上 {corner:?} 一样亮");
}

/// 和 2D 对齐的正交相机（同 `camera3d.match2d`）：1 个单位是 1 个像素，z 在 [-1000, 1000] 都看得到。
fn match_2d(world: &mut World) {
    world.spawn((Camera3d { ortho: Some(SIZE as f32), near: 0.0, far: 2000.0, ..Default::default() }, Transform3d::from_xyz(0.0, 0.0, 1000.0)));
}

/// 左边：红方块正面在 z = 10，前面 z = 5 的绿精灵（下半）被挡、z = 5000 的白精灵（上半）不被裁掉；
/// 右边：蓝方块正面在 z = -40，z = -10 的黄精灵在它前面。
fn mixed_scene(app: &mut App, interleave: bool) {
    use nge_render2d::{Render2dPlugin, Sprite, Transform2d};
    unlit(app);
    app.add_plugins(Render2dPlugin).insert_resource(nge_ecs::Interleave(interleave));
    app.add_systems(Startup, |world: &mut World| {
        match_2d(world);
        solid(world, Vec4::new(1.0, 0.0, 0.0, 1.0), Mesh::cube(20.0), Transform3d::from_xyz(-16.0, 0.0, 0.0));
        solid(world, Vec4::new(0.0, 0.0, 1.0, 1.0), Mesh::cube(20.0), Transform3d::from_xyz(16.0, 0.0, -50.0));
        let sprite = |world: &mut World, x: f32, y: f32, z: f32, color: Vec4| {
            world.spawn((Sprite::color(Vec2::splat(6.0), color), Transform2d::from_xy(x, y).with_z(z)));
        };
        sprite(world, -16.0, -6.0, 5.0, Vec4::new(0.0, 1.0, 0.0, 1.0));
        sprite(world, -16.0, 6.0, 5000.0, Vec4::ONE);
        sprite(world, 16.0, 0.0, -10.0, Vec4::new(1.0, 1.0, 0.0, 1.0));
    });
}

#[test]
fn interleaved_sprites_and_models_sort_by_depth() {
    let Some(img) = render(SIZE, |app| mixed_scene(app, true)) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(close(img.at(16, 38), [255, 0, 0, 255]), "z = 5 的精灵在模型后面 {:?}", img.at(16, 38));
    assert!(close(img.at(16, 26), [255; 4]), "z 很大的精灵在最前面、不被裁掉 {:?}", img.at(16, 26));
    assert!(close(img.at(48, 32), [255, 255, 0, 255]), "z < 0 的精灵在更远的模型前面 {:?}", img.at(48, 32));
    assert!(close(img.at(48, 25), [0, 0, 255, 255]), "精灵外面是模型 {:?}", img.at(48, 25));
}

#[test]
fn without_interleave_layers_stay_as_before() {
    let Some(img) = render(SIZE, |app| mixed_scene(app, false)) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(close(img.at(16, 38), [0, 255, 0, 255]), "z ≥ 0 的精灵画在 3D 上面 {:?}", img.at(16, 38));
    assert!(close(img.at(48, 32), [0, 0, 255, 255]), "z < 0 的精灵画在 3D 下面 {:?}", img.at(48, 32));
}

/// 地面上方一个方块，光从左上斜着照：方块的影子落在它右边的地上。
fn shadow_scene(app: &mut App, shadows: bool) {
    app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.1 });
    app.insert_resource(ClearColor(wgpu::Color::BLACK));
    app.add_systems(Startup, move |world: &mut World| {
        let (camera, eye) = shadow_camera();
        world.spawn((camera, eye));
        solid(world, Vec4::ONE, Mesh::plane(6.0), Transform3d::default());
        solid(world, Vec4::ONE, Mesh::cube(0.5), Transform3d::from_xyz(0.0, 1.0, 0.0));
        world.spawn(DirectionalLight { direction: Vec3::new(1.0, -1.0, 0.0), shadows, ..Default::default() });
    });
}

fn shadow_camera() -> (Camera3d, Transform3d) {
    (Camera3d::default(), Transform3d::from_xyz(0.0, 4.0, 4.0).looking_at(Vec3::new(0.5, 0.0, 0.0), Vec3::Y))
}

/// 世界上的点画在哪个像素。
fn pixel_of(point: Vec3) -> (u32, u32) {
    let (camera, eye) = shadow_camera();
    pixel_in(camera, eye, point)
}

fn pixel_in(camera: Camera3d, eye: Transform3d, point: Vec3) -> (u32, u32) {
    pixel_sized(camera, eye, SIZE, point)
}

#[test]
fn shadowed_light_darkens_what_is_behind_the_caster() {
    let Some(img) = render(SIZE, |app| shadow_scene(app, true)) else {
        return eprintln!("没有 GPU，跳过");
    };
    // 方块中心在 y = 1，光沿 (1, -1) 走：影子中心落在 x = 1
    let (sx, sy) = pixel_of(Vec3::new(1.0, 0.0, 0.0));
    let (lx, ly) = pixel_of(Vec3::new(-1.5, 0.0, 0.0));
    let (shadow, lit) = (img.at(sx, sy), img.at(lx, ly));
    assert!(lit[0] > 120, "没挡住的地方照亮了 {lit:?}");
    // 环境光 0.1（线性）是 sRGB 89
    assert!(close(shadow, [89, 89, 89, 255]), "影子里只有环境光 {shadow:?}，外面 {lit:?}");
    // 方块顶面朝光，没有自己挡自己
    let (tx, ty) = pixel_of(Vec3::new(0.0, 1.25, 0.0));
    assert!(img.at(tx, ty)[0] > 120, "方块顶面没有阴影痘 {:?}", img.at(tx, ty));
}

#[test]
fn without_shadows_the_ground_is_evenly_lit() {
    let Some(img) = render(SIZE, |app| shadow_scene(app, false)) else {
        return eprintln!("没有 GPU，跳过");
    };
    let (sx, sy) = pixel_of(Vec3::new(1.0, 0.0, 0.0));
    let (lx, ly) = pixel_of(Vec3::new(-1.5, 0.0, 0.0));
    assert!(close(img.at(sx, sy), img.at(lx, ly)), "{:?} vs {:?}", img.at(sx, sy), img.at(lx, ly));
}

/// 两截竖条：下半截（y 0–1）跟关节 0、上半截（y 1–2）跟关节 1；关节 1 挂在关节 0 上方 1 处。
/// 动画 bend 把关节 1 绕 z 转 90°，上半截倒向左边。模型整体下移 1，下半截在画面中心下方。
fn bending_bar(world: &mut World, animate: bool) {
    use nge_render3d::{Animation, Channel, Influence, Interpolation, Model, Node, Part, Pose, Property, Skeleton, Skin, Trs, Vertex};

    let mut mesh = Mesh::default();
    for (joint, y) in [(0u16, 0.0), (1, 1.0)] {
        let base = mesh.vertices.len() as u32;
        for (x, dy) in [(-0.25, 0.0), (0.25, 0.0), (0.25, 1.0), (-0.25, 1.0)] {
            mesh.vertices.push(Vertex::new(Vec3::new(x, y + dy, 0.0), Vec3::Z, Vec2::ZERO));
            mesh.skin.push(Influence { joints: [joint, 0, 0, 0], weights: [1.0, 0.0, 0.0, 0.0] });
        }
        mesh.indices.extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
    }
    let quarter = Vec4::from(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
    let mut assets = world.resource_mut::<Assets3d>();
    let mesh = assets.add_mesh(mesh).unwrap();
    let material = assets.add_material(Material { base_color: Vec4::new(1.0, 0.0, 0.0, 1.0), ..Default::default() });
    let model = assets
        .add_model(Model {
            parts: vec![Part { skin: Some(0), ..Part::new(mesh, material, glam::Mat4::IDENTITY) }],
            skeleton: Skeleton {
                nodes: vec![
                    Node { parent: None, rest: Trs::default() },
                    Node { parent: Some(0), rest: Trs { translation: Vec3::Y, ..Default::default() } },
                ],
                skins: vec![Skin { joints: vec![0, 1], inverse_bind: vec![glam::Mat4::IDENTITY, glam::Mat4::from_translation(Vec3::NEG_Y)] }],
                names: vec![],
            },
            animations: vec![Animation {
                name: "bend".into(),
                duration: 1.0,
                channels: vec![Channel { node: 1, property: Property::Rotation, interpolation: Interpolation::Step, times: vec![0.0, 1.0], values: vec![quarter; 2] }],
            }],
        })
        .unwrap();
    // 右边再放一根不播动画的：两份关节矩阵在同一个缓冲里，各自从自己的起点读
    world.spawn((Model3d(model), Transform3d::from_xyz(1.5, -1.0, 0.0)));
    let mut entity = world.spawn((Model3d(model), Transform3d::from_xyz(0.0, -1.0, 0.0)));
    if animate {
        entity.insert(Animator::new("bend"));
    } else {
        // 去掉 Animator 后留下的姿势（弯着的）不该再起作用
        let bent = glam::Mat4::from_translation(Vec3::Y) * glam::Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let mut pose = Pose::default();
        pose.nodes = vec![glam::Mat4::IDENTITY, bent];
        entity.insert(pose);
    }
    // 开着投影子的光，蒙皮的阴影管线也要跑一遍
    world.spawn(DirectionalLight { direction: Vec3::NEG_Z, shadows: true, ..Default::default() });
}

fn bar_pixels(animate: bool) -> Option<(bool, bool, bool)> {
    let img = render(SIZE, move |app| {
        unlit(app);
        app.add_systems(Startup, move |world: &mut World| bending_bar(world, animate));
    })?;
    let eye = Transform3d::from_xyz(0.0, 0.0, 5.0);
    let lit = |p: Vec3| {
        let (x, y) = pixel_in(Camera3d::default(), eye, p);
        img.at(x, y)[0] > 100
    };
    // 上半截竖着时在 (0, 0.5)，倒向左边后在 (-0.5, 0)
    // 右边那根一直竖着
    Some((lit(Vec3::new(0.0, 0.5, 0.0)), lit(Vec3::new(-0.6, 0.0, 0.0)), lit(Vec3::new(1.5, 0.5, 0.0))))
}

#[test]
fn skinned_models_follow_their_joints() {
    let Some(rest) = bar_pixels(false) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert_eq!(rest, (true, false, true), "不播动画（只剩 Pose）：竖着");
    assert_eq!(bar_pixels(true).unwrap(), (false, true, true), "关节 1 转了 90°：上半截倒向左边");
}

/// 不蒙皮的部件挂在节点上：动画把节点移到左边 1.5 处，方块跟着走。
fn sliding_cube(animate: bool) -> Option<Pixels> {
    use nge_render3d::{Animation, Channel, Interpolation, MaterialId, Model, Node, Part, Property, Skeleton, Trs};

    render(SIZE, move |app| {
        unlit(app);
        app.add_systems(Startup, move |world: &mut World| {
            let left = Vec4::new(-1.5, 0.0, 0.0, 0.0);
            let mut assets = world.resource_mut::<Assets3d>();
            let mesh = assets.add_mesh(Mesh::cube(1.0)).unwrap();
            let model = assets
                .add_model(Model {
                    parts: vec![Part { node: Some(0), ..Part::new(mesh, MaterialId::DEFAULT, glam::Mat4::IDENTITY) }],
                    skeleton: Skeleton { nodes: vec![Node { parent: None, rest: Trs::default() }], skins: vec![], names: vec![] },
                    animations: vec![Animation {
                        name: "slide".into(),
                        duration: 1.0,
                        channels: vec![Channel { node: 0, property: Property::Translation, interpolation: Interpolation::Step, times: vec![0.0], values: vec![left] }],
                    }],
                })
                .unwrap();
            let mut entity = world.spawn((Model3d(model), Transform3d::default()));
            if animate {
                entity.insert(Animator::new("slide"));
            }
        });
    })
}

#[test]
fn animated_nodes_move_their_parts() {
    let Some(rest) = sliding_cube(false) else {
        return eprintln!("没有 GPU，跳过");
    };
    let eye = Transform3d::from_xyz(0.0, 0.0, 5.0);
    let (lx, ly) = pixel_in(Camera3d::default(), eye, Vec3::new(-1.5, 0.0, 0.5));
    assert!(rest.center()[0] > 200 && rest.at(lx, ly)[0] < 10);
    let moved = sliding_cube(true).unwrap();
    assert!(moved.center()[0] < 10 && moved.at(lx, ly)[0] > 200, "{:?}", moved.at(lx, ly));
}

/// 排在 3D 渲染器后面，记下这一帧到它为止的（draw 次数，实例数）。
struct CountProbe(std::sync::Arc<std::sync::Mutex<(u32, u32)>>);

impl nge_ecs::Renderer for CountProbe {
    fn render(&mut self, world: &mut World, _: &Gfx, _: &mut wgpu::CommandEncoder, _: &wgpu::TextureView) {
        *self.0.lock().unwrap() = world.resource::<nge_ecs::RenderFrame>().counts();
    }
}

#[test]
fn models_count_their_draws_and_the_shadow_pass_counts_again() {
    for (shadows, expected) in [(false, (2, 2)), (true, (4, 2))] {
        let counts = std::sync::Arc::new(std::sync::Mutex::new((0, 0)));
        let probe = counts.clone();
        if render(SIZE, move |app| {
            shadow_scene(app, shadows);
            app.add_renderer(CountProbe(probe.clone()));
        })
        .is_none()
        {
            return;
        }
        assert_eq!(*counts.lock().unwrap(), expected, "阴影 {shadows}");
    }
}

/// 每帧往 `Billboards` 里放东西（它在 PreUpdate 清空）。
fn billboards(app: &mut App, push: impl Fn(&mut nge_render3d::Billboards) + Send + Sync + 'static) {
    app.add_systems(Update, move |mut b: ResMut<nge_render3d::Billboards>| push(&mut b));
}

fn quad(z: f32, size: f32, color: Vec4) -> nge_render3d::Billboard {
    nge_render3d::Billboard { position: Vec3::new(0.0, 0.0, z), size, rotation: 0.0, color }
}

#[test]
fn billboards_are_hidden_by_models_in_front_of_them() {
    use nge_render3d::Blend;
    let Some(img) = render(SIZE, |app| {
        unlit(app);
        // 小方块在前面，后面一张大的红色方片
        app.add_systems(Startup, |world: &mut World| solid(world, Vec4::new(0.0, 1.0, 0.0, 1.0), Mesh::cube(1.0), Transform3d::from_xyz(0.0, 0.0, 1.0)));
        billboards(app, |b| b.push(ImageId::WHITE, Blend::Alpha, quad(-1.0, 4.0, Vec4::new(1.0, 0.0, 0.0, 1.0))));
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(close(img.center(), [0, 255, 0, 255]), "被方块挡住 {:?}", img.center());
    assert!(close(img.at(SIZE / 2, 18), [255, 0, 0, 255]), "方块外面露出方片 {:?}", img.at(SIZE / 2, 18));
    assert!(close(img.at(1, 1), [0, 0, 0, 255]));
}

#[test]
fn additive_billboards_add_up_and_do_not_write_depth() {
    use nge_render3d::Blend;
    let Some(img) = render(SIZE, |app| {
        unlit(app);
        // 近的先放：要是写了深度，远的那张就被挡掉；颜色按透明度乘：0.5 × 0.5 两张加起来是 0.5
        billboards(app, |b| {
            b.push(ImageId::WHITE, Blend::Add, quad(1.0, 2.0, Vec4::new(0.5, 0.0, 0.0, 0.5)));
            b.push(ImageId::WHITE, Blend::Add, quad(-1.0, 2.0, Vec4::new(0.5, 0.0, 0.0, 0.5)));
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let c = img.center();
    assert!(c[0].abs_diff(188) <= 4 && c[1] == 0, "线性 0.5 写进 sRGB 是 188 左右：{c:?}");
}

#[test]
fn alpha_billboards_blend_over_what_is_behind() {
    use nge_render3d::Blend;
    let Some(img) = render(SIZE, |app| {
        unlit(app);
        app.add_systems(Startup, |world: &mut World| solid(world, Vec4::new(0.0, 0.0, 1.0, 1.0), Mesh::plane(4.0), facing_camera()));
        billboards(app, |b| b.push(ImageId::WHITE, Blend::Alpha, quad(1.0, 1.0, Vec4::new(1.0, 0.0, 0.0, 0.5))));
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let c = img.center();
    // 线性空间里各一半，写进 sRGB 是 188 左右
    assert!(c[0].abs_diff(188) <= 4 && c[2].abs_diff(188) <= 4 && c[1] == 0, "{c:?}");
}

#[test]
fn ribbons_draw_a_band_facing_the_camera() {
    use nge_render3d::{Blend, RibbonPoint};
    let Some(img) = render(SIZE, |app| {
        unlit(app);
        billboards(app, |b| {
            let point = |x: f32| RibbonPoint { position: Vec3::new(x, 0.0, 0.0), width: 0.5, color: Vec4::new(1.0, 1.0, 0.0, 1.0) };
            b.ribbon(ImageId::WHITE, Blend::Alpha, [point(-1.0), point(0.0), point(1.0)]);
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(close(img.center(), [255, 255, 0, 255]), "{:?}", img.center());
    assert!(close(img.at(SIZE / 2 - 10, SIZE / 2), [255, 255, 0, 255]), "沿 x 方向");
    assert!(close(img.at(SIZE / 2, SIZE / 2 - 10), [0, 0, 0, 255]), "竖直方向只有带子那么宽");
}

/// 半透明材质的物体。
fn glass(world: &mut World, color: Vec4, mesh: Mesh, transform: Transform3d) {
    let mut assets = world.resource_mut::<Assets3d>();
    let mesh = assets.add_mesh(mesh).unwrap();
    let material = assets.add_material(Material { base_color: color, blend: true, ..Default::default() });
    world.spawn((Mesh3d { mesh, material }, transform));
}

fn at_z(z: f32) -> Transform3d {
    Transform3d { translation: Vec3::new(0.0, 0.0, z), ..facing_camera() }
}

#[test]
fn blended_objects_show_what_is_behind() {
    for glass_first in [true, false] {
        let Some(img) = render(SIZE, move |app| {
            unlit(app);
            app.add_systems(Startup, move |world: &mut World| {
                let red = |w: &mut World| glass(w, Vec4::new(1.0, 0.0, 0.0, 0.5), Mesh::plane(1.0), at_z(1.0));
                let blue = |w: &mut World| solid(w, Vec4::new(0.0, 0.0, 1.0, 1.0), Mesh::plane(4.0), facing_camera());
                if glass_first {
                    red(world);
                    blue(world);
                } else {
                    blue(world);
                    red(world);
                }
            });
        }) else {
            return eprintln!("没有 GPU，跳过");
        };
        let c = img.center();
        // 线性空间里红蓝各一半，写进 sRGB 是 188 左右
        assert!(c[0].abs_diff(188) <= 4 && c[2].abs_diff(188) <= 4 && c[1] == 0, "glass_first={glass_first}: {c:?}");
        assert!(close(img.at(SIZE / 2, 20), [0, 0, 255, 255]), "半透明方片外面是蓝色 {:?}", img.at(SIZE / 2, 20));
    }
}

#[test]
fn opaque_objects_hide_blended_ones_behind_them() {
    let Some(img) = render(SIZE, |app| {
        unlit(app);
        app.add_systems(Startup, |world: &mut World| {
            glass(world, Vec4::new(1.0, 0.0, 0.0, 0.5), Mesh::plane(4.0), facing_camera());
            solid(world, Vec4::new(0.0, 1.0, 0.0, 1.0), Mesh::cube(1.0), Transform3d::from_xyz(0.0, 0.0, 1.0));
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(close(img.center(), [0, 255, 0, 255]), "前面的不透明方块挡住半透明的 {:?}", img.center());
}

#[test]
fn blended_objects_mix_far_to_near_in_any_spawn_order() {
    let mut results = Vec::new();
    for red_first in [true, false] {
        let Some(img) = render(SIZE, move |app| {
            unlit(app);
            app.add_systems(Startup, move |world: &mut World| {
                let red = |w: &mut World| glass(w, Vec4::new(1.0, 0.0, 0.0, 0.5), Mesh::plane(2.0), at_z(0.0));
                let green = |w: &mut World| glass(w, Vec4::new(0.0, 1.0, 0.0, 0.5), Mesh::plane(2.0), at_z(1.0));
                if red_first {
                    red(world);
                    green(world);
                } else {
                    green(world);
                    red(world);
                }
            });
        }) else {
            return eprintln!("没有 GPU，跳过");
        };
        results.push(img.center());
    }
    assert!(close(results[0], results[1]), "{:?} vs {:?}", results[0], results[1]);
    // 先画远的红再画近的绿：红 0.25、绿 0.5（线性），sRGB 是 137、188 左右
    let c = results[0];
    assert!(c[0].abs_diff(137) <= 4 && c[1].abs_diff(188) <= 4 && c[2] == 0, "{c:?}");
}

#[test]
fn blended_objects_cast_no_shadow() {
    let Some(img) = render(SIZE, |app| {
        app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.1 });
        app.insert_resource(ClearColor(wgpu::Color::BLACK));
        app.add_systems(Startup, |world: &mut World| {
            let (camera, eye) = shadow_camera();
            world.spawn((camera, eye));
            solid(world, Vec4::ONE, Mesh::plane(6.0), Transform3d::default());
            glass(world, Vec4::new(1.0, 1.0, 1.0, 0.5), Mesh::cube(0.5), Transform3d::from_xyz(0.0, 1.0, 0.0));
            world.spawn(DirectionalLight { direction: Vec3::new(1.0, -1.0, 0.0), shadows: true, ..Default::default() });
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let (sx, sy) = pixel_of(Vec3::new(1.0, 0.0, 0.0));
    let (lx, ly) = pixel_of(Vec3::new(-1.5, 0.0, 0.0));
    assert!(img.at(lx, ly)[0] > 120, "{:?}", img.at(lx, ly));
    assert!(close(img.at(sx, sy), img.at(lx, ly)), "半透明方块后面没有影子 {:?} vs {:?}", img.at(sx, sy), img.at(lx, ly));
}

/// 世界上的点在边长 `size` 的图上画在哪个像素。
fn pixel_sized(camera: Camera3d, eye: Transform3d, size: u32, point: Vec3) -> (u32, u32) {
    let c = camera.view_proj(&eye, 1.0) * point.extend(1.0);
    let ndc = c.truncate().truncate() / c.w;
    (((ndc.x * 0.5 + 0.5) * size as f32) as u32, ((0.5 - ndc.y * 0.5) * size as f32) as u32)
}

/// 朝下照的聚光灯。
fn spot_down(outer: f32, shadows: bool) -> (SpotLight, Transform3d) {
    let light = SpotLight { intensity: 20.0, range: 10.0, inner_angle: outer * 0.8, outer_angle: outer, shadows, ..Default::default() };
    (light, Transform3d { translation: Vec3::new(0.0, 3.0, 0.0), rotation: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2), ..Default::default() })
}

#[test]
fn spot_light_lights_its_cone_only() {
    let Some(img) = render(SIZE, |app| {
        app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.0 });
        app.add_systems(Startup, |world: &mut World| {
            solid(world, Vec4::ONE, Mesh::plane(4.0), facing_camera());
            // 在 (0, 0, 3) 朝 -Z 照向平面：外锥 10° 在平面上半径约 0.53
            let spot = SpotLight { intensity: 10.0, range: 10.0, inner_angle: 5f32.to_radians(), outer_angle: 10f32.to_radians(), ..Default::default() };
            world.spawn((spot, Transform3d::from_xyz(0.0, 0.0, 3.0)));
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let c = SIZE / 2;
    let (center, inner, outside) = (img.center(), img.at(c + 2, c), img.at(c + 13, c));
    assert!(center[0] > 100, "锥里亮 {center:?}");
    assert!(close(center, inner), "内锥里一样亮 {center:?} vs {inner:?}");
    assert!(outside[0] < 5, "锥外暗 {outside:?}");
    let (top, left) = (img.at(c, c - 13), img.at(c - 13, c));
    assert!(top[0] < 5 && left[0] < 5, "四周都是暗的 {top:?} {left:?}");
}

/// 地面上方一个方块，聚光灯从正上方照下来：影子落在方块正下方。
fn spot_scene(app: &mut App, shadows: bool) {
    app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.1 });
    app.insert_resource(ClearColor(wgpu::Color::BLACK));
    app.add_systems(Startup, move |world: &mut World| {
        let (camera, eye) = shadow_camera();
        world.spawn((camera, eye));
        solid(world, Vec4::ONE, Mesh::plane(6.0), Transform3d::default());
        solid(world, Vec4::ONE, Mesh::cube(0.5), Transform3d::from_xyz(0.0, 1.0, 0.0));
        world.spawn(spot_down(40f32.to_radians(), shadows));
    });
}

#[test]
fn spot_light_shadow_falls_behind_the_caster() {
    let (sx, sy) = pixel_of(Vec3::ZERO);
    let (lx, ly) = pixel_of(Vec3::new(-1.2, 0.0, 0.0));
    let Some(img) = render(SIZE, |app| spot_scene(app, true)) else {
        return eprintln!("没有 GPU，跳过");
    };
    let (shadow, lit) = (img.at(sx, sy), img.at(lx, ly));
    assert!(lit[0] > 120, "锥里没挡住的地方亮 {lit:?}");
    assert!(close(shadow, [89, 89, 89, 255]), "方块正下方只有环境光 {shadow:?}，外面 {lit:?}");
    let (tx, ty) = pixel_of(Vec3::new(0.0, 1.25, 0.0));
    assert!(img.at(tx, ty)[0] > 120, "方块顶面没有阴影痘 {:?}", img.at(tx, ty));

    let Some(img) = render(SIZE, |app| spot_scene(app, false)) else { return };
    assert!(img.at(sx, sy)[0] > 120, "不开阴影时方块下面也亮 {:?}", img.at(sx, sy));
}

#[test]
fn spot_shadow_pass_skips_what_is_outside_the_cone() {
    let counts = std::sync::Arc::new(std::sync::Mutex::new((0, 0)));
    let probe = counts.clone();
    if render(SIZE, move |app| {
        app.add_plugins(Render3dPlugin);
        app.add_systems(Startup, |world: &mut World| {
            solid(world, Vec4::ONE, Mesh::plane(6.0), Transform3d::default());
            // 外锥 20°：方块在锥外
            solid(world, Vec4::ONE, Mesh::cube(0.5), Transform3d::from_xyz(2.5, 1.0, 0.0));
            world.spawn(spot_down(20f32.to_radians(), true));
        });
        app.add_renderer(CountProbe(probe.clone()));
    })
    .is_none()
    {
        return;
    }
    assert_eq!(*counts.lock().unwrap(), (3, 2), "主 pass 2 次，阴影 pass 只画地面");
}

#[test]
fn point_light_casts_shadows_all_around() {
    let camera = Camera3d::default();
    let eye = Transform3d::from_xyz(0.0, 7.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z);
    let scene = move |shadows: bool| {
        render(SIZE, move |app| {
            app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.1 });
            app.insert_resource(ClearColor(wgpu::Color::BLACK));
            app.add_systems(Startup, move |world: &mut World| {
                world.spawn((camera, eye));
                solid(world, Vec4::ONE, Mesh::plane(10.0), Transform3d::default());
                // 灯在 (0, 1.5, 0)：四周各一块，影子落在 2 远的地上；正下方一块，影子比它大一圈
                for p in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z, Vec3::ZERO] {
                    solid(world, Vec4::ONE, Mesh::cube(0.5), Transform3d { translation: p + Vec3::Y * 0.75, ..Default::default() });
                }
                world.spawn((PointLight { intensity: 20.0, range: 10.0, shadows, ..Default::default() }, Transform3d::from_xyz(0.0, 1.5, 0.0)));
            });
        })
    };
    let Some(img) = scene(true) else {
        return eprintln!("没有 GPU，跳过");
    };
    let at = |img: &Pixels, p: Vec3| {
        let (x, y) = pixel_sized(camera, eye, SIZE, p);
        img.at(x, y)
    };
    let lit = at(&img, Vec3::new(1.6, 0.0, 1.4));
    assert!(lit[0] > 120, "斜对角没挡住 {lit:?}");
    let shadows = [Vec3::new(2.0, 0.0, 0.0), Vec3::new(-2.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.0, 0.0, -2.0), Vec3::new(0.4, 0.0, 0.0)];
    for p in shadows {
        let c = at(&img, p);
        assert!(c[0] <= 95, "{p} 在影子里 {c:?}，外面 {lit:?}");
    }
    let Some(img) = scene(false) else { return };
    for p in shadows {
        assert!(at(&img, p)[0] > 120, "不开阴影时 {p} 是亮的 {:?}", at(&img, p));
    }
}

/// 点光源在高 3 处，挡板挡住朝 +X 斜 45° 往下的那一片地。地上离灯水平 2.86–3 的一圈，往外推之前离灯最远的轴是 y、
/// 推之后是 x：得按推出去之后的点挑面，不然拿 -Y 面去采一个在它视野外的点，算成照亮（对角线接缝漏光）。
#[test]
fn point_shadow_has_no_gap_on_the_diagonal_between_faces() {
    const SIZE: u32 = 128;
    let camera = Camera3d::default();
    let eye = Transform3d::from_xyz(0.0, 8.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z);
    let Some(img) = render(SIZE, move |app| {
        app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.1 });
        // 阴影图小一点，推出去的量大，接缝那一圈有好几个像素宽
        app.insert_resource(ClearColor(wgpu::Color::BLACK)).insert_resource(Shadows { local_size: 64, ..Default::default() });
        app.add_systems(Startup, move |world: &mut World| {
            world.spawn((camera, eye));
            solid(world, Vec4::ONE, Mesh::plane(12.0), Transform3d::default());
            solid(world, Vec4::ONE, Mesh::cube(0.8), Transform3d::from_xyz(1.465, 1.5, 0.0));
            world.spawn((PointLight { intensity: 20.0, range: 10.0, shadows: true, ..Default::default() }, Transform3d::from_xyz(0.0, 3.0, 0.0)));
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let at = |p: Vec3| {
        let (x, y) = pixel_sized(camera, eye, SIZE, p);
        img.at(x, y)
    };
    let lit = at(Vec3::new(-2.93, 0.0, 0.0));
    assert!(lit[0] > 120, "对面没挡住 {lit:?}");
    for x in [2.5, 2.93, 3.5] {
        let c = at(Vec3::new(x, 0.0, 0.0));
        assert!(c[0] <= 95, "x = {x} 在挡板的影子里 {c:?}，外面 {lit:?}");
    }
}

/// 第一帧画完后把阴影图换个尺寸、挪一下投影物：主 pass 得换用新的阴影图，影子跟着挪。
fn resized_shadows(spot: bool) -> Option<Pixels> {
    render_frames(SIZE, 2, move |app| {
        app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.1 });
        app.insert_resource(ClearColor(wgpu::Color::BLACK)).insert_resource(Shadows { size: 1024, local_size: 512, ..Default::default() });
        app.add_systems(Startup, move |world: &mut World| {
            world.spawn(shadow_camera());
            solid(world, Vec4::ONE, Mesh::plane(6.0), Transform3d::default());
            solid(world, Vec4::ONE, Mesh::cube(0.5), Transform3d::from_xyz(0.0, 1.0, 0.0));
            if spot {
                world.spawn(spot_down(40f32.to_radians(), true));
            } else {
                world.spawn(DirectionalLight { direction: Vec3::new(1.0, -1.0, 0.0), shadows: true, ..Default::default() });
            }
        });
        app.add_systems(Update, |mut frame: Local<u32>, mut shadows: ResMut<Shadows>, mut q: Query<&mut Transform3d, With<Mesh3d>>| {
            *frame += 1;
            if *frame == 2 {
                (shadows.size, shadows.local_size) = (2048, 256);
                q.iter_mut().filter(|t| t.translation.y == 1.0).for_each(|mut t| t.translation.z = -1.2);
            }
        });
    })
}

#[test]
fn changing_shadow_map_sizes_at_runtime_keeps_shadows_right() {
    let Some(img) = resized_shadows(false) else {
        return eprintln!("没有 GPU，跳过");
    };
    // 方块挪到 z = -1.2：方向光的影子从 (1, 0, 0) 挪到 (1, 0, -1.2)
    let at = |img: &Pixels, p: Vec3| {
        let (x, y) = pixel_of(p);
        img.at(x, y)
    };
    let (now, before) = (at(&img, Vec3::new(1.0, 0.0, -1.2)), at(&img, Vec3::X));
    assert!(now[0] <= 95 && before[0] > 120, "方向光：影子在新位置 {now:?}，旧位置亮 {before:?}");

    // 聚光灯在正上方 3 处：影子按透视放大 1.5 倍，落在 (0, 0, -1.8)
    let img = resized_shadows(true).unwrap();
    let (now, before) = (at(&img, Vec3::new(0.0, 0.0, -1.8)), at(&img, Vec3::ZERO));
    assert!(now[0] <= 95 && before[0] > 120, "聚光灯：影子在新位置 {now:?}，旧位置亮 {before:?}");
}

/// 相机沿地面往远处看，近、中、远三个大方块；光从左上斜着照，影子落在各自右边。
fn cascade_scene(app: &mut App, shadows: Shadows) {
    app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.1 });
    app.insert_resource(ClearColor(wgpu::Color::BLACK)).insert_resource(shadows);
    app.add_systems(Startup, move |world: &mut World| {
        world.spawn(cascade_camera());
        solid(world, Vec4::ONE, Mesh::plane(200.0), Transform3d::default());
        for z in CASCADE_CUBES {
            solid(world, Vec4::ONE, Mesh::cube(2.0), Transform3d::from_xyz(0.0, 1.0, z));
        }
        world.spawn(DirectionalLight { direction: Vec3::new(1.0, -1.0, 0.0), shadows: true, ..Default::default() });
    });
}

const CASCADE_CUBES: [f32; 3] = [0.0, -15.0, -45.0];
const CASCADE_SIZE: u32 = 256;

fn cascade_camera() -> (Camera3d, Transform3d) {
    (Camera3d::default(), Transform3d::from_xyz(0.0, 4.0, 8.0).looking_at(Vec3::new(0.0, 0.0, -20.0), Vec3::Y))
}

/// 每个方块（影子里的点, 旁边亮处的点）的颜色。
fn cascade_pixels(shadows: Shadows) -> Option<Vec<([u8; 4], [u8; 4])>> {
    let img = render(CASCADE_SIZE, move |app| cascade_scene(app, shadows))?;
    let (camera, eye) = cascade_camera();
    let at = |p: Vec3| {
        let (x, y) = pixel_sized(camera, eye, CASCADE_SIZE, p);
        img.at(x, y)
    };
    Some(CASCADE_CUBES.iter().map(|&z| (at(Vec3::new(2.0, 0.0, z)), at(Vec3::new(-2.5, 0.0, z)))).collect())
}

#[test]
fn cascades_shadow_near_and_far_casters_but_not_past_the_distance() {
    let cascades = Shadows { cascades: 3, distance: 30.0, ..Default::default() };
    let Some(px) = cascade_pixels(cascades) else {
        return eprintln!("没有 GPU，跳过");
    };
    // 近的在相机前约 8，中间的约 22，远的约 51
    for (i, (shadow, lit)) in px[..2].iter().enumerate() {
        assert!(lit[0] > 120, "第 {i} 个旁边亮 {lit:?}");
        assert!(shadow[0] <= 95, "第 {i} 个有影子 {shadow:?}");
    }
    let (shadow, lit) = px[2];
    assert!(close(shadow, lit), "distance 外的没有影子 {shadow:?} vs {lit:?}");

    // 不分段时远处的也有影子：上面没影子是 distance 截掉的
    let Some(px) = cascade_pixels(Shadows::default()) else { return };
    for (i, (shadow, lit)) in px.iter().enumerate() {
        assert!(lit[0] > 120 && shadow[0] <= 95, "一段时第 {i} 个有影子 {shadow:?} / {lit:?}");
    }
}

#[path = "offscreen/morph.rs"]
mod morph;

/// 一条穿过画面中心的白线（z = -1），前面挡着一个红色方块；`on_top` 时画在最前面。
fn line_behind_cube(on_top: bool) -> Option<Pixels> {
    render(SIZE, move |app| {
        unlit(app);
        app.add_systems(Startup, |world: &mut World| solid(world, Vec4::new(1.0, 0.0, 0.0, 1.0), Mesh::cube(1.0), Transform3d::from_xyz(0.0, 0.0, 1.0)));
        app.add_systems(Update, move |mut gizmos: ResMut<Gizmos3d>| {
            let (a, b, white) = (Vec3::new(-4.0, 0.0, -1.0), Vec3::new(4.0, 0.0, -1.0), Vec4::ONE);
            if on_top {
                gizmos.on_top().line(a, b, white);
            } else {
                gizmos.line(a, b, white);
            }
        });
    })
}

/// 中间那两行（1 像素的线落在其中一行）里，第 `x` 列有没有白色。
fn white_in_middle(img: &Pixels, x: u32) -> bool {
    [SIZE / 2 - 1, SIZE / 2].iter().any(|&y| close(img.at(x, y), [255, 255, 255, 255]))
}

#[test]
fn debug_lines_are_hidden_behind_models_unless_on_top() {
    let Some(img) = line_behind_cube(false) else { return eprintln!("没有 GPU，跳过") };
    assert!(white_in_middle(&img, 4), "方块外面看得见线");
    assert!(!white_in_middle(&img, SIZE / 2) && close(img.center(), [255, 0, 0, 255]), "被前面的方块挡住 {:?}", img.center());
    let Some(img) = line_behind_cube(true) else { return };
    assert!(white_in_middle(&img, 4) && white_in_middle(&img, SIZE / 2), "画在最前面的不被挡");
}
