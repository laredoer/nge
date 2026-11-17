//! 变形目标：权重、和蒙皮一起、影子。

use nge_render3d::{Animation, Channel, Influence, Interpolation, MaterialId, Model, MorphTarget, MorphWeights, Node, Part, Property, Skeleton, Skin, Trs};

use super::*;

/// 中心在原点、边长 1、朝 +Z 的方块；一个目标把所有顶点挪 `delta`。
fn shifting_quad(delta: Vec3) -> Mesh {
    let mut mesh = Mesh::plane(1.0);
    for v in &mut mesh.vertices {
        let p = Vec3::from(v.position);
        v.position = [p.x, -p.z, 0.0];
        v.normal = [0.0, 0.0, 1.0];
    }
    mesh.targets = vec![MorphTarget { positions: vec![delta; mesh.vertices.len()], normals: vec![] }];
    mesh
}

fn lit_at(img: &Pixels, p: Vec3) -> bool {
    let (x, y) = pixel_in(Camera3d::default(), Transform3d::from_xyz(0.0, 0.0, 5.0), p);
    img.at(x, y)[0] > 100
}

fn weighted_quad(weight: f32) -> Option<Pixels> {
    render(SIZE, move |app| {
        unlit(app);
        app.add_systems(Startup, move |world: &mut World| {
            let mut assets = world.resource_mut::<Assets3d>();
            let mesh = assets.add_mesh(shifting_quad(Vec3::X * 1.5)).unwrap();
            let material = assets.add_material(Material { base_color: Vec4::new(1.0, 0.0, 0.0, 1.0), ..Default::default() });
            world.spawn((Mesh3d { mesh, material }, Transform3d::default(), MorphWeights(vec![weight])));
        });
    })
}

#[test]
fn morph_weights_move_vertices() {
    let Some(rest) = weighted_quad(0.0) else {
        return eprintln!("没有 GPU，跳过");
    };
    let (center, right, half) = (Vec3::ZERO, Vec3::X * 1.5, Vec3::X * 0.75);
    assert_eq!((lit_at(&rest, center), lit_at(&rest, right)), (true, false), "权重 0：在原处");
    let full = weighted_quad(1.0).unwrap();
    assert_eq!((lit_at(&full, center), lit_at(&full, right)), (false, true), "权重 1：挪到右边 1.5");
    let mid = weighted_quad(0.5).unwrap();
    assert_eq!((lit_at(&mid, center), lit_at(&mid, half)), (false, true), "权重 0.5：挪一半");
}

/// 蒙皮的方块，唯一的关节绕 z 转了 90°；目标把顶点往 +X 挪 1.5。先变形再蒙皮：挪到了 +Y。
/// `weights` 是部件的默认权重；`squish` 时播把权重设成 1 的动画。
fn skinned_morph(weights: Vec<f32>, squish: bool) -> Option<Pixels> {
    render(SIZE, move |app| {
        unlit(app);
        let weights = weights.clone();
        app.add_systems(Startup, move |world: &mut World| {
            let mut mesh = shifting_quad(Vec3::X * 1.5);
            mesh.skin = vec![Influence { joints: [0; 4], weights: [1.0, 0.0, 0.0, 0.0] }; mesh.vertices.len()];
            let mut assets = world.resource_mut::<Assets3d>();
            let mesh = assets.add_mesh(mesh).unwrap();
            let one = Channel { node: 0, property: Property::Weights(0), interpolation: Interpolation::Step, times: vec![0.0], values: vec![Vec4::X] };
            let turned = Trs { rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), ..Default::default() };
            let model = assets
                .add_model(Model {
                    parts: vec![Part { node: Some(0), skin: Some(0), weights: weights.clone(), ..Part::new(mesh, MaterialId::DEFAULT, glam::Mat4::IDENTITY) }],
                    skeleton: Skeleton { nodes: vec![Node { parent: None, rest: turned }], skins: vec![Skin { joints: vec![0], inverse_bind: vec![glam::Mat4::IDENTITY] }], names: vec![] },
                    animations: vec![Animation { name: "squish".into(), duration: 1.0, channels: vec![one] }],
                })
                .unwrap();
            let mut entity = world.spawn((Model3d(model), Transform3d::default()));
            if squish {
                entity.insert(Animator::new("squish"));
            }
        });
    })
}

#[test]
fn morph_then_skin() {
    let Some(rest) = skinned_morph(vec![], false) else {
        return eprintln!("没有 GPU，跳过");
    };
    let (center, up, right) = (Vec3::ZERO, Vec3::Y * 1.5, Vec3::X * 1.5);
    let state = |img: &Pixels| (lit_at(img, center), lit_at(img, up), lit_at(img, right));
    assert_eq!(state(&rest), (true, false, false), "没有权重：在原处");
    assert_eq!(state(&skinned_morph(vec![1.0], false).unwrap()), (false, true, false), "默认权重 1：变形后再跟关节转到上面");
    assert_eq!(state(&skinned_morph(vec![0.0], true).unwrap()), (false, true, false), "动画把权重设成 1");
}

/// 阴影场景里的方块换成可以变形的：目标把它往 -X 挪 1.5。
fn morph_shadow(weight: f32) -> Option<Pixels> {
    render(SIZE, move |app| {
        app.add_plugins(Render3dPlugin).insert_resource(AmbientLight { color: Vec3::ONE, brightness: 0.1 });
        app.insert_resource(ClearColor(wgpu::Color::BLACK));
        app.add_systems(Startup, move |world: &mut World| {
            let (camera, eye) = shadow_camera();
            world.spawn((camera, eye));
            solid(world, Vec4::ONE, Mesh::plane(6.0), Transform3d::default());
            let mut cube = Mesh::cube(0.5);
            cube.targets = vec![MorphTarget { positions: vec![Vec3::NEG_X * 1.5; cube.vertices.len()], normals: vec![] }];
            let mesh = world.resource_mut::<Assets3d>().add_mesh(cube).unwrap();
            world.spawn((Mesh3d { mesh, material: MaterialId::DEFAULT }, Transform3d::from_xyz(0.0, 1.0, 0.0), MorphWeights(vec![weight])));
            world.spawn(DirectionalLight { direction: Vec3::new(1.0, -1.0, 0.0), shadows: true, ..Default::default() });
        });
    })
}

#[test]
fn morphed_casters_cast_morphed_shadows() {
    let Some(rest) = morph_shadow(0.0) else {
        return eprintln!("没有 GPU，跳过");
    };
    let dark = |img: &Pixels, x: f32| {
        let (px, py) = pixel_of(Vec3::new(x, 0.0, 0.0));
        img.at(px, py)[0] < 110
    };
    assert_eq!((dark(&rest, 1.0), dark(&rest, -0.5)), (true, false), "没变形：影子在 x = 1");
    let moved = morph_shadow(1.0).unwrap();
    assert_eq!((dark(&moved, 1.0), dark(&moved, -0.5)), (false, true), "挪到 x = -1.5：影子跟着到 x = -0.5");
}

/// 在 x 处放一块可以变形的方块，目标挪 `delta`，权重 1；`padding` 个不画的顶点把变形数据撑大。
fn morphing_quad(world: &mut World, x: f32, delta: Vec3, padding: usize, color: Vec4) {
    let mut mesh = shifting_quad(delta);
    let extra = mesh.vertices[0];
    mesh.vertices.extend(std::iter::repeat_n(extra, padding));
    mesh.targets[0].positions.resize(mesh.vertices.len(), delta);
    let mut assets = world.resource_mut::<Assets3d>();
    let mesh = assets.add_mesh(mesh).unwrap();
    let material = assets.add_material(Material { base_color: color, ..Default::default() });
    world.spawn((Mesh3d { mesh, material }, Transform3d::from_xyz(x, 0.0, 0.0), MorphWeights(vec![1.0])));
}

/// 第一帧一个小的，第二帧再来一个大的（变形数据超过初始的 256 个 vec4）：缓冲扩容、旧数据搬过去、bind group 换新的。
#[test]
fn morph_buffer_grows_and_keeps_earlier_meshes() {
    let Some(img) = render_frames(SIZE, 2, |app| {
        unlit(app);
        app.add_systems(Startup, |world: &mut World| morphing_quad(world, -1.2, Vec3::Y * 1.5, 0, Vec4::new(1.0, 0.0, 0.0, 1.0)));
        app.add_systems(Update, |world: &mut World, mut frame: Local<u32>| {
            *frame += 1;
            if *frame == 2 {
                morphing_quad(world, 1.2, Vec3::NEG_Y * 1.5, 300, Vec4::new(0.0, 1.0, 0.0, 1.0));
            }
        });
    }) else {
        return eprintln!("没有 GPU，跳过");
    };
    let at = |p: Vec3| {
        let (x, y) = pixel_in(Camera3d::default(), Transform3d::from_xyz(0.0, 0.0, 5.0), p);
        img.at(x, y)
    };
    let (red_rest, red_moved) = (at(Vec3::new(-1.2, 0.0, 0.0)), at(Vec3::new(-1.2, 1.5, 0.0)));
    assert!(red_moved[0] > 200 && red_rest[0] < 10, "先上传的小网格扩容后照样变形：{red_moved:?} / {red_rest:?}");
    let (green_rest, green_moved) = (at(Vec3::new(1.2, 0.0, 0.0)), at(Vec3::new(1.2, -1.5, 0.0)));
    assert!(green_moved[1] > 200 && green_rest[1] < 10, "扩容那一帧上传的大网格变形：{green_moved:?} / {green_rest:?}");
}
