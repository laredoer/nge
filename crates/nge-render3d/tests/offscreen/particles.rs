//! 3D 粒子补齐（见 docs/particles3d-more.md）：软粒子、拉长、序列帧、受光。

use nge_render3d::{Billboard, Billboards, Blend, Facing};

use super::*;

/// 每帧放一个方片，贴图由 `texture` 在启动时建。
fn one(push: Billboard, blend: Blend, texture: fn(&mut World) -> ImageId, build: impl Fn(&mut App) + Send + Sync + 'static) -> Option<Pixels> {
    render(SIZE, move |app| {
        build(app);
        let id = std::sync::Arc::new(std::sync::OnceLock::new());
        let set = id.clone();
        app.add_systems(Startup, move |world: &mut World| {
            let _ = set.set(texture(world));
        });
        app.add_systems(Update, move |mut b: ResMut<Billboards>| b.push(*id.get().unwrap(), blend, push));
    })
}

fn white(_: &mut World) -> ImageId {
    ImageId::WHITE
}

/// 一张朝相机的蓝色平面（z = 0），前面 0.25 处一张红色方片。
fn soft_over_plane(soft: f32) -> Option<Pixels> {
    let red = Billboard { position: Vec3::new(0.0, 0.0, 0.25), size: 1.0, color: Vec4::new(1.0, 0.0, 0.0, 1.0), soft, ..Default::default() };
    one(red, Blend::Alpha, white, |app| {
        unlit(app);
        app.add_systems(Startup, |world: &mut World| solid(world, Vec4::new(0.0, 0.0, 1.0, 1.0), Mesh::plane(4.0), facing_camera()));
    })
}

#[test]
fn soft_particles_fade_near_opaque_surfaces() {
    let Some(hard) = soft_over_plane(0.0) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(close(hard.center(), [255, 0, 0, 255]), "不软化 {:?}", hard.center());
    let c = soft_over_plane(1.0).unwrap().center();
    // 离平面 0.25、软化 1：剩 0.25 的红，线性 0.25 / 0.75 写进 sRGB 是 137 / 225
    assert!(c[0].abs_diff(137) <= 6 && c[2].abs_diff(225) <= 6 && c[1] == 0, "{c:?}");
    assert!(close(soft_over_plane(0.1).unwrap().center(), [255, 0, 0, 255]), "离得比软化距离远就不淡");
}

#[test]
fn soft_particles_still_hide_behind_models_and_lines_still_draw() {
    let Some(img) = render(SIZE, |app| {
        unlit(app);
        app.add_systems(Startup, |world: &mut World| solid(world, Vec4::new(0.0, 1.0, 0.0, 1.0), Mesh::cube(1.0), Transform3d::from_xyz(0.0, 0.0, 1.0)));
        app.add_systems(Update, |mut b: ResMut<Billboards>, mut gizmos: ResMut<Gizmos3d>| {
            b.push(ImageId::WHITE, Blend::Alpha, Billboard { position: Vec3::new(0.0, 0.0, -1.0), size: 4.0, color: Vec4::new(1.0, 0.0, 0.0, 1.0), soft: 0.5, ..Default::default() });
            gizmos.line(Vec3::new(-4.0, -1.5, 0.0), Vec3::new(4.0, -1.5, 0.0), Vec4::ONE);
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(close(img.center(), [0, 255, 0, 255]), "被方块挡住 {:?}", img.center());
    assert!(close(img.at(SIZE / 2, 18), [255, 0, 0, 255]), "方块外面露出方片（后面没东西，不淡） {:?}", img.at(SIZE / 2, 18));
    let line = (0..SIZE).map(|y| img.at(SIZE / 2 + 12, y)).filter(|p| close(*p, [255, 255, 255, 255])).count();
    assert!(line > 0, "调试线在软粒子之后照样画");
}

#[test]
fn velocity_facing_draws_a_streak_along_the_motion() {
    let streak = Billboard { size: 0.2, color: Vec4::new(1.0, 1.0, 0.0, 1.0), facing: Facing::Velocity { velocity: Vec3::new(10.0, 0.0, 0.0), stretch: 0.3 }, ..Default::default() };
    let Some(img) = one(streak, Blend::Alpha, white, unlit) else {
        return eprintln!("没有 GPU，跳过");
    };
    let c = SIZE / 2;
    assert!(close(img.at(c - 12, c), [255, 255, 0, 255]) && close(img.at(c + 12, c), [255, 255, 0, 255]), "沿 x 拉长 {:?}", img.at(c - 12, c));
    assert!(close(img.at(c, c - 5), [0, 0, 0, 255]), "竖着很窄");
}

/// 8 × 1 的贴图：左半红、右半绿，当两帧。
fn two_frames(world: &mut World) -> ImageId {
    let rgba = (0..8).flat_map(|x| if x < 4 { [255, 0, 0, 255] } else { [0, 255, 0, 255] }).collect();
    world.resource_mut::<Assets<Image>>().add_rgba(UVec2::new(8, 1), rgba).unwrap()
}

#[test]
fn sprite_sheet_frames_pick_their_cell_and_blend() {
    let frame = |uv, next_uv, frame_mix| Billboard { size: 2.0, uv, next_uv, frame_mix, ..Default::default() };
    let Some(second) = one(frame([0.5, 0.0, 1.0, 1.0], [0.5, 0.0, 1.0, 1.0], 0.0), Blend::Alpha, two_frames, unlit) else {
        return eprintln!("没有 GPU，跳过");
    };
    assert!(close(second.center(), [0, 255, 0, 255]), "第二帧 {:?}", second.center());
    let mixed = one(frame([0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 1.0, 1.0], 0.5), Blend::Alpha, two_frames, unlit).unwrap().center();
    assert!(mixed[0].abs_diff(188) <= 4 && mixed[1].abs_diff(188) <= 4 && mixed[2] == 0, "两帧各一半 {mixed:?}");
    let first = one(frame([0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 1.0, 1.0], 0.0), Blend::Alpha, two_frames, unlit).unwrap().center();
    assert!(close(first, [255, 0, 0, 255]), "不混只用这一帧 {first:?}");
}

/// 环境光 0、一盏方向光：受光的白色方片的中心。
fn lit_center(direction: Vec3, lit: bool) -> Option<[u8; 4]> {
    let quad = Billboard { size: 2.0, lit, ..Default::default() };
    let img = one(quad, Blend::Alpha, white, move |app| {
        app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.0 });
        app.insert_resource(ClearColor(wgpu::Color::BLACK));
        app.add_systems(Startup, move |world: &mut World| {
            world.spawn(DirectionalLight { direction, color: Vec3::ONE, illuminance: 3.0, shadows: false });
        });
    })?;
    Some(img.center())
}

#[test]
fn lit_particles_follow_the_directional_light() {
    let Some(front) = lit_center(Vec3::NEG_Z, true) else {
        return eprintln!("没有 GPU，跳过");
    };
    let side = lit_center(Vec3::X, true).unwrap();
    let back = lit_center(Vec3::Z, true).unwrap();
    // 包裹的 lambert：正面 3 / π，侧面 3 / 4π，背面 0
    assert!(front[0] > 240, "迎着光 {front:?}");
    assert!((110..170).contains(&side[0]), "侧面一半再平方 {side:?}");
    assert!(back[0] < 10, "背光 {back:?}");
    assert!(close(lit_center(Vec3::Z, false).unwrap(), [255, 255, 255, 255]), "不受光的照旧");
}
