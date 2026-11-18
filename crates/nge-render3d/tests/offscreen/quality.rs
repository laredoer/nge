//! 3D 画质（见 docs/render3d-quality.md）：天空、环境光照、雾、软阴影、多灯投影、级联过渡。

use nge_render3d::{Assets, Billboard, Billboards, Blend, CubeImage, Fog, FogMode, HeightFog, ShadowFilter, ShadowPriority, Sky, SkyGradient, SkySource};

use super::*;

fn gradient(top: Vec3, horizon: Vec3, ground: Vec3) -> SkyGradient {
    SkyGradient { top, horizon, ground, sun: Vec3::ZERO, ..Default::default() }
}

fn sky(source: SkySource) -> Sky {
    Sky { source, ..Default::default() }
}

fn dark(app: &mut App) {
    app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.0 });
    app.insert_resource(ClearColor(wgpu::Color::BLACK));
}

#[test]
fn gradient_sky_fills_the_background_behind_objects() {
    let Some(img) = render(SIZE, |app| {
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
    let Some(img) = render(SIZE, |app| {
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

fn cube_sky(from_ktx2: bool, look: Vec3) -> Option<Pixels> {
    render(SIZE, move |app| {
        dark(app);
        app.add_systems(Startup, move |world: &mut World| {
            let source = if from_ktx2 {
                let cube = CubeImage { size: 4, faces: faces() };
                SkySource::Cube(world.resource_mut::<Assets<CubeImage>>().add(cube))
            } else {
                let mut images = world.resource_mut::<Assets<Image>>();
                SkySource::Faces(faces().map(|f| images.add_rgba(UVec2::splat(4), f).unwrap()))
            };
            world.insert_resource(sky(source));
            world.spawn((Camera3d::default(), Transform3d::from_xyz(0.0, 0.0, 5.0).looking_at(Vec3::new(0.0, 0.0, 5.0) + look, Vec3::Y)));
        });
    })
}

#[test]
fn cube_sky_shows_each_face_the_way_you_look_without_mirroring() {
    for ktx2 in [false, true] {
        let Some(img) = cube_sky(ktx2, Vec3::NEG_Z) else {
            return eprintln!("没有 GPU，跳过");
        };
        let (left, right) = (img.at(10, SIZE / 2), img.at(SIZE - 10, SIZE / 2));
        assert!(close(left, [255, 0, 0, 255]) && close(right, [0, 0, 255, 255]), "ktx2={ktx2}: 朝 -Z 看左红右蓝 {left:?} {right:?}");
        let img = cube_sky(ktx2, Vec3::X).unwrap();
        assert!(close(img.center(), [255, 255, 0, 255]), "朝 +X 看是 +X 面 {:?}", img.center());
        let img = cube_sky(ktx2, Vec3::Z).unwrap();
        assert!(close(img.center(), [255, 0, 255, 255]), "朝 +Z 看是 +Z 面 {:?}", img.center());
    }
}

/// 没有灯、常量环境光 0，只有一片灰色的天空照着白球。
fn sky_lit_sphere(diffuse: f32, specular: f32, metallic: f32, roughness: f32, top: Vec3) -> Option<Pixels> {
    render(SIZE, move |app| {
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
            let quad = |position, color| Billboard { position, size: 2.0, rotation: 0.0, color };
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
