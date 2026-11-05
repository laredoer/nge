//! 离屏渲染到纹理再读回像素，验证贴图方向、深度遮挡、光照、alpha 裁剪、2D 叠加和 glTF。没有 GPU 时跳过。
//!
//! 设了 `NGE_DUMP_DIR` 时，把 glTF 示例场景另存成 PNG，方便人工核对。

use glam::{Quat, UVec2, Vec2, Vec3, Vec4};
use nge_core::{Ctx, Emit, Gfx, Viewport, wgpu};
use nge_ecs::prelude::*;
use nge_render3d::prelude::*;
use nge_render3d::{AssetPlugin, AssetSource, MemorySource};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIZE: u32 = 64;

fn gfx(size: u32) -> Option<Gfx> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    let viewport = Viewport { size: [size as f32; 2], pixels: [size; 2] };
    Some(Gfx { device, queue, format: FORMAT, viewport })
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
            world.spawn(DirectionalLight { direction: Vec3::NEG_X, color: Vec3::ONE, illuminance: 3.0 });
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
            world.spawn((PointLight { color: Vec3::new(1.0, 0.8, 0.5), intensity: 6.0, range: 8.0 }, Transform3d::from_xyz(-1.5, 1.5, 1.5)));
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
                world.spawn(DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, illuminance: 3.0 });
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
    for _ in 0..200 {
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
            world.spawn(DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, illuminance: 1.0 });
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let (center, corner) = (img.center(), img.at(4, 4));
    assert!(center[0] > 100, "正对着有高光 {center:?}");
    assert!(close(center, corner), "中间 {center:?} 和角上 {corner:?} 一样亮");
}
