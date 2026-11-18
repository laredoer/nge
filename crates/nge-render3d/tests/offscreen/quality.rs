//! 3D 画质（见 docs/render3d-quality.md）：天空、环境光照、雾、软阴影、多灯投影、级联过渡。

use nge_render3d::{Assets, Billboard, Billboards, Blend, SkyBake, CubeImage, Fog, FogMode, HeightFog, ShadowFilter, ShadowPriority, Sky, SkyGradient, SkySource};

use std::sync::{Arc, Mutex};

use super::*;

fn gradient(top: Vec3, horizon: Vec3, ground: Vec3) -> SkyGradient {
    SkyGradient { top, horizon, ground, sun: Vec3::ZERO, ..Default::default() }
}

fn sky(source: SkySource) -> Sky {
    Sky { source, ..Default::default() }
}

/// 天空在后台烘焙：一直跑帧（每帧让一下烘焙线程）直到烘焙好，再读回像素；按墙钟最多等 15 秒。
fn render_baked(size: u32, build: impl Fn(&mut App) + Send + Sync + 'static) -> Option<Pixels> {
    let bake = Arc::new(Mutex::new(SkyBake::default()));
    let mut offscreen = Offscreen::new(size, watch_bake(build, bake.clone()))?;
    wait_baked(&mut offscreen, &bake);
    offscreen.frame(true)
}

/// 加一个系统把每帧的 `SkyBake` 记到 `into`。
fn watch_bake(build: impl Fn(&mut App) + Send + Sync + 'static, into: Arc<Mutex<SkyBake>>) -> impl Fn(&mut App) + Send + Sync + 'static {
    move |app| {
        build(app);
        let into = into.clone();
        app.add_systems(Update, move |bake: Res<SkyBake>| *into.lock().unwrap() = *bake);
    }
}

fn wait_baked(offscreen: &mut Offscreen, bake: &Mutex<SkyBake>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    offscreen.frame(false);
    while !bake.lock().unwrap().ready && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
        offscreen.frame(false);
    }
    assert!(bake.lock().unwrap().ready, "15 秒内天空没烘焙好");
}

#[test]
fn turning_the_sun_does_not_rebake_the_sky() {
    let bake = Arc::new(Mutex::new(SkyBake::default()));
    // 太阳的角度由测试控制（等烘焙的帧数不固定，不能按帧累加，不然可能转出画面）
    let turn = Arc::new(Mutex::new(0.0f32));
    let angle = turn.clone();
    let build = move |app: &mut App| {
        dark(app);
        app.insert_resource(sky(SkySource::Gradient(SkyGradient { sun: Vec3::new(0.0, -0.2, 1.0), sun_size: 0.2, ..SkyGradient::default() })));
        let angle = angle.clone();
        app.add_systems(Update, move |mut sky: ResMut<Sky>| {
            let t = *angle.lock().unwrap();
            if let SkySource::Gradient(g) = &mut sky.source {
                g.sun = Vec3::new(t.sin(), -0.2, t.cos());
            }
        });
    };
    let Some(mut offscreen) = Offscreen::new(SIZE, watch_bake(build, bake.clone())) else {
        return eprintln!("没有 GPU，跳过");
    };
    wait_baked(&mut offscreen, &bake);
    let before = offscreen.frame(true).unwrap();
    // 太阳每帧转一点，20 帧共 0.3 弧度，还在画面里
    for _ in 0..20 {
        *turn.lock().unwrap() += 0.015;
        offscreen.frame(false);
    }
    let after = offscreen.frame(true).unwrap();
    let b = *bake.lock().unwrap();
    assert_eq!((b.count, b.ready), (1, true), "太阳转了 20 多帧，只烘焙过一次");
    assert!(before.pixels != after.pixels, "太阳确实在动（每帧按新方向画）");
}

fn dark(app: &mut App) {
    app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.0 });
    app.insert_resource(ClearColor(wgpu::Color::BLACK));
}

#[test]
fn gradient_sky_fills_the_background_behind_objects() {
    let Some(img) = render_baked(SIZE, |app| {
        dark(app);
        let g = gradient(Vec3::X, Vec3::Y, Vec3::Z);
        app.insert_resource(Sky { diffuse: 0.0, specular: 0.0, ..sky(SkySource::Gradient(g)) });
        app.add_systems(Startup, |world: &mut World| solid(world, Vec4::ONE, Mesh::cube(0.5), Transform3d::default()));
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let (top, middle, bottom) = (img.at(SIZE / 2, 1), img.at(4, SIZE / 2), img.at(SIZE / 2, SIZE - 2));
    assert!(top[0] > 150 && top[0] > top[1], "上方偏 top（红）{top:?}");
    assert!(middle[1] > 240 && middle[0] < 10 && middle[2] < 100, "地平线附近是 horizon（绿），往下一点点就混进 ground {middle:?}");
    assert!(bottom[2] > 150 && bottom[2] > bottom[1], "下方偏 ground（蓝）{bottom:?}");
    // 物体挡住天空；没有灯、环境光照强度 0：是黑的
    assert!(close(img.center(), [0, 0, 0, 255]), "方块不被天空盖住 {:?}", img.center());
}

#[test]
fn the_sun_is_drawn_against_its_light_direction() {
    let Some(img) = render_baked(SIZE, |app| {
        dark(app);
        // 光往 +Z 走：太阳在 -Z，正前方
        let g = SkyGradient { sun: Vec3::Z, sun_color: Vec3::ONE, sun_size: 5f32.to_radians(), ..gradient(Vec3::splat(0.1), Vec3::splat(0.1), Vec3::splat(0.1)) };
        app.insert_resource(sky(SkySource::Gradient(g)));
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(img.center()[0] > 240, "太阳盘 {:?}", img.center());
    assert!(img.at(2, 2)[0] < 100, "别处是天空 {:?}", img.at(2, 2));
}

/// 6 个面各一个颜色；-Z 面左半红、右半蓝（看左右镜像没有）。
fn faces() -> [Vec<u8>; 6] {
    let solid = |c: [u8; 4]| c.repeat(16);
    let front: Vec<u8> = (0..16).flat_map(|i| if i % 4 < 2 { [255, 0, 0, 255] } else { [0, 0, 255, 255] }).collect();
    [solid([255, 255, 0, 255]), solid([0, 255, 255, 255]), solid([255, 255, 255, 255]), solid([60, 60, 60, 255]), solid([255, 0, 255, 255]), front]
}

/// 边长 8 的立方体，每格按 `dir(面, s, t)` 给的方向的正负着色：x > 0 红、y > 0 绿、z > 0 蓝。
fn octant_faces(dir: fn(usize, f32, f32) -> Vec3) -> [Vec<u8>; 6] {
    std::array::from_fn(|face| {
        (0..64)
            .flat_map(|i| {
                let d = dir(face, ((i % 8) as f32 + 0.5) / 8.0, ((i / 8) as f32 + 0.5) / 8.0);
                [if d.x > 0.0 { 255 } else { 0 }, if d.y > 0.0 { 255 } else { 0 }, if d.z > 0.0 { 255 } else { 0 }, 255]
            })
            .collect()
    })
}

/// KTX2 / Vulkan 规范的立方体面：主轴 ma、(sc, tc) 按规范的表，s、t 从图的左上角起。
fn spec_dir(face: usize, s: f32, t: f32) -> Vec3 {
    let (sc, tc) = (2.0 * s - 1.0, 2.0 * t - 1.0);
    match face {
        0 => Vec3::new(1.0, -tc, -sc),
        1 => Vec3::new(-1.0, -tc, sc),
        2 => Vec3::new(sc, 1.0, tc),
        3 => Vec3::new(sc, -1.0, -tc),
        4 => Vec3::new(sc, -tc, 1.0),
        _ => Vec3::new(-sc, -tc, -1.0),
    }
}

/// 6 张图的约定（docs/render3d-quality.md）：从里面朝那个方向看到的画面，侧面上方 +Y，抬头那张上方 +Z，低头那张上方 -Z。
fn user_dir(face: usize, s: f32, t: f32) -> Vec3 {
    let (forward, up) = [(Vec3::X, Vec3::Y), (Vec3::NEG_X, Vec3::Y), (Vec3::Y, Vec3::Z), (Vec3::NEG_Y, Vec3::NEG_Z), (Vec3::Z, Vec3::Y), (Vec3::NEG_Z, Vec3::Y)][face];
    forward + forward.cross(up) * (2.0 * s - 1.0) + up * (1.0 - 2.0 * t)
}

/// 从原点朝 `look` 看（视角 90°，正好看满一个面）。
fn cube_sky(faces: [Vec<u8>; 6], side: u32, from_ktx2: bool, look: Vec3) -> Option<(Pixels, Transform3d)> {
    let up = if look.y.abs() > 0.9 { Vec3::NEG_Z } else { Vec3::Y };
    let eye = Transform3d::default().looking_at(look, up);
    let img = render_baked(SIZE, move |app| {
        dark(app);
        let faces = faces.clone();
        app.add_systems(Startup, move |world: &mut World| {
            let source = if from_ktx2 {
                let cube = CubeImage { size: side, faces: faces.clone() };
                SkySource::Cube(world.resource_mut::<Assets<CubeImage>>().add(cube))
            } else {
                let mut images = world.resource_mut::<Assets<Image>>();
                SkySource::Faces(faces.clone().map(|f| images.add_rgba(UVec2::splat(side), f).unwrap()))
            };
            world.insert_resource(sky(source));
            world.spawn((Camera3d { fov_y: 90f32.to_radians(), ..Default::default() }, eye));
        });
    })?;
    Some((img, eye))
}

/// 朝 6 个面各看一次，每个面看 4 个斜方向：画出来的颜色是那个方向的正负。
fn octants_match(faces: [Vec<u8>; 6], from_ktx2: bool) -> Option<()> {
    for look in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z] {
        let (img, eye) = cube_sky(faces.clone(), 8, from_ktx2, look)?;
        let (a, b) = look.any_orthonormal_pair();
        for (i, j) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
            let d = look + (a * i + b * j) * 0.45;
            let (x, y) = pixel_sized(Camera3d { fov_y: 90f32.to_radians(), ..Default::default() }, eye, SIZE, d);
            let want = [d.x > 0.0, d.y > 0.0, d.z > 0.0].map(|p| if p { 255 } else { 0 });
            let got = img.at(x, y);
            assert!(close(got, [want[0], want[1], want[2], 255]), "ktx2={from_ktx2} 朝 {look} 看 {d}：{got:?}");
        }
    }
    Some(())
}

#[test]
fn png_faces_show_what_you_see_looking_out_without_mirroring() {
    let Some((img, _)) = cube_sky(faces(), 4, false, Vec3::NEG_Z) else {
        return eprintln!("没有 GPU，跳过");
    };
    let (left, right) = (img.at(10, SIZE / 2), img.at(SIZE - 10, SIZE / 2));
    assert!(close(left, [255, 0, 0, 255]) && close(right, [0, 0, 255, 255]), "朝 -Z 看左红右蓝 {left:?} {right:?}");
    // 六个面（含抬头、低头）都按文档的约定
    octants_match(octant_faces(user_dir), false);
}

#[test]
fn ktx2_cubes_follow_the_spec_layout() {
    // 外部工具按 KTX2 / Vulkan 规范存的立方体：直接按 GPU 的约定用，不重排
    if octants_match(octant_faces(spec_dir), true).is_none() {
        eprintln!("没有 GPU，跳过");
    }
}

/// 没有灯、常量环境光 0，只有一片灰色的天空照着白球。
fn sky_lit_sphere(diffuse: f32, specular: f32, metallic: f32, roughness: f32, top: Vec3) -> Option<Pixels> {
    render_baked(SIZE, move |app| {
        dark(app);
        app.insert_resource(Sky { diffuse, specular, background: false, ..sky(SkySource::Gradient(gradient(top, Vec3::splat(0.5), Vec3::splat(0.5)))) });
        app.add_systems(Startup, move |world: &mut World| {
            let mut assets = world.resource_mut::<Assets3d>();
            let mesh = assets.add_mesh(Mesh::sphere(1.5, 32)).unwrap();
            let material = assets.add_material(Material { base_color: Vec4::ONE, metallic, roughness, ..Default::default() });
            world.spawn((Mesh3d { mesh, material }, Transform3d::default()));
        });
    })
}

#[test]
fn the_sky_lights_objects_by_its_irradiance() {
    let Some(lit) = sky_lit_sphere(1.0, 0.0, 0.0, 1.0, Vec3::splat(0.5)) else {
        return eprintln!("没有 GPU，跳过");
    };
    // 0.5 灰的天空，白色漫反射：亮度约 0.5 × (1 - F) ≈ 0.48，sRGB 约 185
    let c = lit.center();
    assert!(c[0].abs_diff(185) <= 12 && close(c, [c[0], c[0], c[0], 255]), "{c:?}");
    assert!(close(lit.at(1, 1), [0, 0, 0, 255]), "不画天空（background 关着）{:?}", lit.at(1, 1));
    let off = sky_lit_sphere(0.0, 0.0, 0.0, 1.0, Vec3::splat(0.5)).unwrap();
    assert!(close(off.center(), [0, 0, 0, 255]), "强度 0 是黑的 {:?}", off.center());
    // 上亮下暗的天空：球的上半边亮
    let sky = sky_lit_sphere(1.0, 0.0, 0.0, 1.0, Vec3::ONE).unwrap();
    assert!(sky.at(SIZE / 2, SIZE / 2 - 12)[0] > sky.at(SIZE / 2, SIZE / 2 + 12)[0] + 10, "{:?} {:?}", sky.at(SIZE / 2, 20), sky.at(SIZE / 2, 44));
}

#[test]
fn smooth_metal_reflects_the_sky() {
    let Some(on) = sky_lit_sphere(1.0, 1.0, 1.0, 0.1, Vec3::ONE) else {
        return eprintln!("没有 GPU，跳过");
    };
    let off = sky_lit_sphere(1.0, 0.0, 1.0, 0.1, Vec3::ONE).unwrap();
    // 金属没有漫反射：关掉镜面是黑的
    assert!(close(off.center(), [0, 0, 0, 255]), "{:?}", off.center());
    assert!(on.center()[0] > 120, "镜面反射了天空 {:?}", on.center());
    // 上半边映出亮的天顶
    assert!(on.at(SIZE / 2, SIZE / 2 - 14)[0] >= on.at(SIZE / 2, SIZE / 2 + 14)[0], "{:?} {:?}", on.at(SIZE / 2, 18), on.at(SIZE / 2, 46));
}

/// 沿地面往远处看，红色的雾。
fn foggy(fog: Fog, extra: fn(&mut App)) -> Option<Pixels> {
    render(SIZE, move |app| {
        unlit(app);
        app.insert_resource(fog);
        app.add_systems(Startup, |world: &mut World| {
            world.spawn((Camera3d::default(), fog_eye()));
            solid(world, Vec4::ONE, Mesh::plane(400.0), Transform3d::default());
        });
        extra(app);
    })
}

fn fog_eye() -> Transform3d {
    Transform3d::from_xyz(0.0, 1.0, 0.0).looking_at(Vec3::new(0.0, 0.5, -10.0), Vec3::Y)
}

#[test]
fn fog_tints_far_things_toward_its_color() {
    let red = Fog { mode: FogMode::Linear { start: 3.0, end: 12.0 }, color: Vec3::X, ..Default::default() };
    let Some(img) = foggy(red, |_| {}) else {
        return eprintln!("没有 GPU，跳过");
    };
    let (near, far) = (img.at(SIZE / 2, SIZE - 1), img.at(SIZE / 2, SIZE / 2 + 1));
    assert!(near[1] > 230, "近处没什么雾 {near:?}");
    assert!(far[0] > 240 && far[1] < near[1] - 60, "远处趋向红色 {far:?}");
    let none = foggy(Fog::default(), |_| {}).unwrap();
    assert!(close(none.at(SIZE / 2, SIZE / 2 + 1), [255, 255, 255, 255]), "没雾时远处也是白的");
    // 高度雾：只开高度雾，低处的地面也被盖住
    let height = Fog { color: Vec3::X, height: Some(HeightFog { base: 0.0, falloff: 1.0, density: 0.3 }), ..Default::default() };
    let img = foggy(height, |_| {}).unwrap();
    assert!(img.at(SIZE / 2, SIZE / 2 + 1)[1] < 120, "高度雾 {:?}", img.at(SIZE / 2, SIZE / 2 + 1));
}

#[test]
fn fog_covers_blended_meshes_and_billboards_too() {
    let thick = Fog { mode: FogMode::Exp { density: 0.6 }, color: Vec3::X, ..Default::default() };
    let Some(img) = foggy(thick, |app| {
        // 远处：左边半透明的绿玻璃、右边绿的方片、中间一个叠加的白方片（盖在地面上）
        app.add_systems(Startup, |world: &mut World| glass(world, Vec4::new(0.0, 1.0, 0.0, 0.8), Mesh::cube(2.0), Transform3d::from_xyz(-2.0, 1.0, -12.0)));
        app.add_systems(Update, |mut b: ResMut<Billboards>| {
            let quad = |position, color| Billboard { position, size: 2.0, color, ..Default::default() };
            b.push(ImageId::WHITE, Blend::Alpha, quad(Vec3::new(2.0, 1.0, -12.0), Vec4::new(0.0, 1.0, 0.0, 1.0)));
            b.push(ImageId::WHITE, Blend::Add, quad(Vec3::new(0.0, 0.0, -12.0), Vec4::ONE));
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let at = |p: Vec3| {
        let (x, y) = pixel_sized(Camera3d::default(), fog_eye(), SIZE, p);
        img.at(x, y)
    };
    for (what, p) in [("半透明", Vec3::new(-2.0, 1.0, -11.0)), ("方片", Vec3::new(2.0, 1.0, -12.0)), ("叠加方片", Vec3::new(0.0, 0.0, -12.0))] {
        let c = at(p);
        // 半透明的后面是清屏的黑：0.8 × 红
        assert!(c[0] > 200 && c[1] < 30 && c[2] < 30, "{what}被浓雾盖成红色 {c:?}");
    }
}

/// 一堵 3 高的薄墙立在 x = 0（z 从 -1.5 到 1.5），阳光往 +X 斜照，影子铺在 x > 0 的地上；从正上方看。
fn wall_shadow(shadows: Shadows) -> Option<(Pixels, Transform3d)> {
    let eye = Transform3d::from_xyz(1.5, 6.0, 0.0).looking_at(Vec3::new(1.5, 0.0, 0.0), Vec3::NEG_Z);
    let img = render(128, move |app| {
        app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.1 });
        app.insert_resource(ClearColor(wgpu::Color::BLACK)).insert_resource(shadows);
        app.add_systems(Startup, move |world: &mut World| {
            world.spawn((Camera3d::default(), eye));
            solid(world, Vec4::ONE, Mesh::plane(12.0), Transform3d::default());
            let wall = Transform3d { translation: Vec3::new(0.0, 1.5, 0.0), scale: Vec3::new(0.1, 3.0, 3.0), ..Default::default() };
            solid(world, Vec4::ONE, Mesh::cube(1.0), wall);
            world.spawn(DirectionalLight { direction: Vec3::new(1.0, -1.0, 0.0), shadows: true, ..Default::default() });
        });
    })?;
    Some((img, eye))
}

/// 影子在 z = 1.5 的那条边上，x 处（挡住它的墙边高度也是 x）沿 z 的过渡像素有几个：影子（≤ 95）和照亮（≥ 亮处 − 8）之间。
fn penumbra((img, eye): &(Pixels, Transform3d), x: f32) -> usize {
    let (_, from) = pixel_sized(Camera3d::default(), *eye, 128, Vec3::new(x, 0.0, 0.7));
    let (column, to) = pixel_sized(Camera3d::default(), *eye, 128, Vec3::new(x, 0.0, 2.6));
    let row: Vec<u8> = (from.min(to)..=from.max(to)).map(|y| img.at(column, y)[0]).collect();
    let lit = *row.iter().max().unwrap();
    assert!(row.iter().any(|&v| v <= 95) && lit > 150, "x = {x}：一头在影子里、一头亮 {row:?}");
    row.iter().filter(|&&v| v > 95 && v < lit - 8).count()
}

fn sharp() -> Shadows {
    Shadows { size: 256, distance: 10.0, ..Default::default() }
}

#[test]
fn soft_filters_widen_the_shadow_edge() {
    let Some(pcf) = wall_shadow(sharp()) else {
        return eprintln!("没有 GPU，跳过");
    };
    let poisson = wall_shadow(Shadows { filter: ShadowFilter::Poisson, softness: 6.0, ..sharp() }).unwrap();
    let (a, b) = (penumbra(&pcf, 1.5), penumbra(&poisson, 1.5));
    assert!(b > a + 2, "泊松盘半径 6 的边缘比 3×3 PCF 宽：{b} vs {a}");
}

/// PCSS：离挡住它的墙边越远，影子边缘越虚；泊松盘处处一样宽。
#[test]
fn pcss_penumbra_grows_with_the_distance_to_the_blocker() {
    let Some(pcss) = wall_shadow(Shadows { filter: ShadowFilter::Pcss, sun_size: 8.0, ..sharp() }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let (near, far) = (penumbra(&pcss, 0.4), penumbra(&pcss, 2.5));
    assert!(far > near + 3, "贴着墙根的边 {near}，远处的 {far}");
    // 墙边高 2.5，沿光线到地面约 3.5，太阳角直径 8°：半影全宽约 3.5 × tan 8° ≈ 0.5，画面上约 9 个像素；
    // 把直径当半径用会宽出一倍（约 19）
    assert!(far <= 12, "半影宽和太阳角直径对得上 {far}");
    let poisson = wall_shadow(Shadows { filter: ShadowFilter::Poisson, softness: 4.0, ..sharp() }).unwrap();
    let (near, far) = (penumbra(&poisson, 0.4), penumbra(&poisson, 2.5));
    assert!(near.abs_diff(far) <= 2, "泊松盘不随距离变 {near} {far}");
}

/// 两盏灯（`left_point` 时左边是点光源，否则都是朝下的聚光灯）各照一个方块；返回（左边影子, 左边亮处, 右边影子, 右边亮处）的红色值。
fn two_lights(left_point: bool, max_lights: u32, right_priority: Option<i32>) -> Option<[u8; 4]> {
    let eye = Transform3d::from_xyz(0.0, 9.0, 0.01).looking_at(Vec3::ZERO, Vec3::Y);
    let img = render(SIZE, move |app| {
        app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.1 });
        app.insert_resource(ClearColor(wgpu::Color::BLACK)).insert_resource(Shadows { max_lights, ..Default::default() });
        app.add_systems(Startup, move |world: &mut World| {
            world.spawn((Camera3d::default(), eye));
            solid(world, Vec4::ONE, Mesh::plane(10.0), Transform3d::default());
            for x in [-2.5, 2.5] {
                solid(world, Vec4::ONE, Mesh::cube(0.5), Transform3d::from_xyz(x, 1.0, 0.0));
                let (light, mut t) = spot_down(40f32.to_radians(), true);
                t.translation.x = x;
                let e = if left_point && x < 0.0 {
                    world.spawn((PointLight { intensity: 20.0, range: 10.0, shadows: true, ..Default::default() }, t)).id()
                } else {
                    world.spawn((light, t)).id()
                };
                if let (Some(p), true) = (right_priority, x > 0.0) {
                    world.entity_mut(e).insert(ShadowPriority(p));
                }
            }
        });
    })?;
    let at = |p: Vec3| {
        let (x, y) = pixel_sized(Camera3d::default(), eye, SIZE, p);
        img.at(x, y)[0]
    };
    // 影子取方块靠里的一侧：从正上方看，靠外的那侧被方块挡住
    Some([at(Vec3::new(-2.2, 0.0, 0.0)), at(Vec3::new(-1.3, 0.0, 0.0)), at(Vec3::new(2.2, 0.0, 0.0)), at(Vec3::new(3.7, 0.0, 0.0))])
}

#[test]
fn several_spot_lights_cast_shadows_up_to_the_limit_by_priority() {
    let Some([ls, ll, rs, rl]) = two_lights(false, 2, None) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(ll > 120 && rl > 120, "两盏都照亮 {ll} {rl}");
    assert!(ls <= 95 && rs <= 95, "两盏都投影子 {ls} {rs}");
    let [ls, ll, rs, rl] = two_lights(false, 1, None).unwrap();
    assert!(ls <= 95 && ll > 120, "上限 1 盏：先查到的那盏投影子 {ls} {ll}");
    assert!(rs > 120 && rl > 120, "另一盏不投影子但照常照明 {rs} {rl}");
    let [ls, _, rs, _] = two_lights(false, 1, Some(3)).unwrap();
    assert!(rs <= 95 && ls > 120, "优先级高的那盏投影子 {ls} {rs}");
    // 点光源（6 层）和聚光灯一起投影子
    let [ls, ll, rs, rl] = two_lights(true, 2, None).unwrap();
    // 点光源四面照，右边的影子里也有它一点光
    assert!(ls <= 95 && ll > 120 && rs + 60 < rl, "点光源和聚光灯都投影子 {ls} {ll} {rs} {rl}");
}

/// 级联调试色：近处红、远处绿，交界前的混合带里两种颜色混着。
#[test]
fn cascade_debug_colors_blend_across_the_seam() {
    let (camera, eye) = (Camera3d::default(), Transform3d::from_xyz(0.0, 4.0, 8.0).looking_at(Vec3::new(0.0, 0.0, -20.0), Vec3::Y));
    let scene = move |blend: f32| {
        render(256, move |app| {
            app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.8 });
            let s = Shadows { cascades: 2, distance: 60.0, blend, debug_cascades: true, ..Default::default() };
            app.insert_resource(ClearColor(wgpu::Color::BLACK)).insert_resource(s);
            app.add_systems(Startup, move |world: &mut World| {
                world.spawn((camera, eye));
                solid(world, Vec4::ONE, Mesh::plane(200.0), Transform3d::default());
                // 不出光，只为了开级联
                world.spawn(DirectionalLight { direction: Vec3::new(1.0, -1.0, 0.0), illuminance: 0.0, shadows: true, ..Default::default() });
            });
        })
    };
    let Some(img) = scene(0.5) else {
        return eprintln!("没有 GPU，跳过");
    };
    // 第 0 段远端约 16.2，混合带 8.1–16.2；第 1 段的混合带从 38 开始
    let forward = eye.rotation * Vec3::NEG_Z;
    let at = |img: &Pixels, depth: f32| {
        // 地面上 x = -1、沿视线前方 depth 的点
        let z = (depth - (Vec3::new(-1.0, 0.0, 0.0) - eye.translation).dot(forward)) / forward.z;
        let (x, y) = pixel_sized(camera, eye, 256, Vec3::new(-1.0, 0.0, z));
        assert!(x < 256 && y < 256, "{depth} 在画面里");
        img.at(x, y)
    };
    let (red, mixed, green) = (at(&img, 7.0), at(&img, 12.2), at(&img, 30.0));
    let [r, g] = [|c: [u8; 4]| i32::from(c[0]), |c: [u8; 4]| i32::from(c[1])];
    assert!(r(red) > g(red) + 60, "近处红 {red:?}");
    assert!(g(green) > r(green) + 60, "远处绿 {green:?}");
    assert!(r(mixed) < r(red) - 20 && g(mixed) > g(red) + 20, "混合带里红绿混着 {mixed:?}，红 {red:?}");
    let hard = scene(0.0).unwrap();
    let edge = at(&hard, 12.2);
    assert!(close(edge, red), "不混时还是第 0 段的红 {edge:?} vs {red:?}");
}

/// 相机在 `eye_y` 往下看 `ground_y` 的白色地面，红色高度雾（base 0、falloff 1、密度 2）；返回画面中间。
fn height_fogged(eye_y: f32, ground_y: f32) -> Option<[u8; 4]> {
    let img = render(SIZE, move |app| {
        unlit(app);
        app.insert_resource(Fog { color: Vec3::X, height: Some(HeightFog { base: 0.0, falloff: 1.0, density: 2.0 }), ..Default::default() });
        app.add_systems(Startup, move |world: &mut World| {
            let mut cam = Camera3d::default();
            cam.far = 1000.0;
            world.spawn((cam, Transform3d::from_xyz(0.0, eye_y, 0.01).looking_at(Vec3::new(0.0, ground_y, 0.0), Vec3::Y)));
            solid(world, Vec4::ONE, Mesh::plane(400.0), Transform3d::from_xyz(0.0, ground_y, 0.0));
        });
    })?;
    Some(img.center())
}

#[test]
fn height_fog_from_a_high_camera_is_not_nan() {
    let Some(c) = height_fogged(200.0, 0.0) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(c[0] > 240 && c[1] < 140, "正确的雾量约 1 - e^(-2) = 0.86，地面明显发红：{c:?}");
    // 两头都在 base 下面很深（指数都超过 80）：满雾，不是没有雾
    let c = height_fogged(-300.0, -500.0).unwrap();
    assert!(close(c, [255, 0, 0, 255]), "很深的地方满雾：{c:?}");
}

/// 换天空时，新天空的贴图加载、烘焙好以前还画旧的（不闪成清屏色）。
#[test]
fn the_old_sky_stays_until_the_new_one_is_ready() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let blue = {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 255, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        png
    };
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let source = Gate { open: gate.clone(), inner: MemorySource::default().with("asset:/@t/blue.png", blue) };
    let source = Mutex::new(Some(Arc::new(source) as Arc<dyn AssetSource>));
    let switch = Arc::new(AtomicBool::new(false));
    let bake = Arc::new(Mutex::new(SkyBake::default()));
    let flip = switch.clone();
    let build = move |app: &mut App| {
        app.add_plugins(AssetPlugin::new().source(source.lock().unwrap().take().expect("只建一次")));
        dark(app);
        app.insert_resource(sky(SkySource::Gradient(gradient(Vec3::X, Vec3::X, Vec3::X))));
        let flip = flip.clone();
        app.add_systems(Update, move |mut sky: ResMut<Sky>, mut server: ResMut<AssetServer>, mut images: ResMut<Assets<Image>>| {
            if flip.load(Ordering::SeqCst) && matches!(sky.source, SkySource::Gradient(_)) {
                let blue = server.load(&mut images, "asset:/@t/blue.png");
                sky.source = SkySource::Faces([blue; 6]);
            }
        });
    };
    let Some(mut offscreen) = Offscreen::new(SIZE, watch_bake(build, bake.clone())) else {
        return eprintln!("没有 GPU，跳过");
    };
    // 断言失败时也放开闸门，不然加载线程一直等着，拆 offscreen 时卡死
    struct Open(Arc<(Mutex<bool>, std::sync::Condvar)>);
    impl Drop for Open {
        fn drop(&mut self) {
            *self.0.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
            self.0.1.notify_all();
        }
    }
    let _open = Open(gate.clone());
    wait_baked(&mut offscreen, &bake);
    assert!(close(offscreen.frame(true).unwrap().center(), [255, 0, 0, 255]), "先是红的程序天空");
    switch.store(true, Ordering::SeqCst);
    for _ in 0..5 {
        let img = offscreen.frame(true).unwrap();
        assert!(close(img.center(), [255, 0, 0, 255]), "贴图还在加载：还画旧的天空 {:?}", img.center());
    }
    assert!(!bake.lock().unwrap().ready, "新天空还没好");
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    wait_baked(&mut offscreen, &bake);
    assert!(close(offscreen.frame(true).unwrap().center(), [0, 0, 255, 255]), "加载、烘焙好以后换成新的");
}

#[test]
fn a_point_light_after_a_spot_light_reads_its_own_layers() {
    // 右边的聚光灯优先级高，排在第 0 层；点光源的 6 面从第 1 层开始
    let Some([ls, ll, rs, rl]) = two_lights(true, 2, Some(3)) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(ls <= 95 && ll > 120, "点光源从自己的层读出影子 {ls} {ll}");
    assert!(rs + 60 < rl, "聚光灯也投影子 {rs} {rl}");
}

#[test]
fn metal_reflects_the_sky_by_the_split_sum() {
    // 0.5 灰的均匀天空、白色金属：亮度 0.5 × (A + B)，粗糙度 0.5 约 0.45（sRGB 约 179），粗糙度 1 约 0.16（约 110）
    let Some(mid) = sky_lit_sphere(0.0, 1.0, 1.0, 0.5, Vec3::splat(0.5)) else {
        return eprintln!("没有 GPU，跳过");
    };
    let c = mid.center();
    assert!((165..=192).contains(&c[0]), "粗糙度 0.5 {c:?}");
    let c = sky_lit_sphere(0.0, 1.0, 1.0, 1.0, Vec3::splat(0.5)).unwrap().center();
    assert!((95..=125).contains(&c[0]), "粗糙度 1 {c:?}");
}

#[test]
fn rough_metal_blurs_the_reflection() {
    // 天顶白、地平线和地面黑：光滑的球上下反差大，粗糙的被糊开
    let contrast = |roughness: f32| {
        let img = sky_lit_sphere(0.0, 1.0, 1.0, roughness, Vec3::ONE)?;
        let (up, down) = (img.at(SIZE / 2, SIZE / 2 - 14)[0], img.at(SIZE / 2, SIZE / 2 + 14)[0]);
        Some(i32::from(up) - i32::from(down))
    };
    let Some(sharp) = contrast(0.1) else {
        return eprintln!("没有 GPU，跳过");
    };
    let blurry = contrast(1.0).unwrap();
    assert!(sharp > 50 && sharp > blurry + 25, "光滑 {sharp} 粗糙 {blurry}");
}

#[test]
fn the_last_cascade_fades_out_to_lit() {
    // 两段、distance 25：第 1 段约 7.5–25，末尾混合带从约 16 开始；中间方块的影子（约 22）大半淡掉，近的（约 8）不受影响
    let fade = Shadows { cascades: 2, distance: 25.0, blend: 0.5, ..Default::default() };
    let Some(soft) = cascade_pixels(fade) else {
        return eprintln!("没有 GPU，跳过");
    };
    let hard = cascade_pixels(Shadows { blend: 0.0, ..fade }).unwrap();
    let (near, mid) = (soft[0], soft[1]);
    assert!(near.0[0] <= 95 && near.1[0] > 120, "近处照常有影子 {near:?}");
    assert!(hard[1].0[0] <= 95, "不淡出时中间的有影子 {:?}", hard[1]);
    assert!(mid.0[0] > hard[1].0[0] + 30 && mid.0[0] < mid.1[0], "淡出到照到 {mid:?} vs 不淡 {:?}", hard[1]);
}
