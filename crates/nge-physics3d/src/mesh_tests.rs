//! 按模型网格生成的碰撞体。

use glam::{Mat4, Quat, Vec3};
use nge_render3d::{Assets3d, MaterialId, Mesh, Model, Model3d, ModelId, Part, Vertex};

use super::*;
use crate::tests::{app, at, ground, pos, steps};

/// 由几个部件组成的模型：每个部件是一个网格和它的 `local`。
fn model(assets: &mut Assets3d, parts: Vec<(Mesh, Mat4)>) -> ModelId {
    let parts = parts.into_iter().map(|(mesh, local)| Part::new(assets.add_mesh(mesh).unwrap(), MaterialId::DEFAULT, local)).collect();
    assets.add_model(Model { parts, ..Default::default() }).unwrap()
}

fn cube_model(assets: &mut Assets3d) -> ModelId {
    model(assets, vec![(Mesh::cube(2.0), Mat4::IDENTITY)])
}

/// 不规则的一团点：二十面体的顶点拉长一点，加上里面的点（凸包不该要它们）。
fn rock() -> Mesh {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut points: Vec<Vec3> = [(-1.0, t, 0.0), (1.0, t, 0.0), (-1.0, -t, 0.0), (1.0, -t, 0.0), (0.0, -1.0, t), (0.0, 1.0, t), (0.0, -1.0, -t), (0.0, 1.0, -t), (t, 0.0, -1.0), (t, 0.0, 1.0), (-t, 0.0, -1.0), (-t, 0.0, 1.0)]
        .map(|(x, y, z)| Vec3::new(x * 1.3, y, z * 0.8))
        .to_vec();
    points.extend([Vec3::ZERO, Vec3::new(0.2, 0.1, -0.3), Vec3::new(-0.4, 0.3, 0.2)]);
    let vertices = points.iter().map(|p| Vertex { position: p.to_array(), normal: [0.0, 1.0, 0.0], uv: [0.0; 2] }).collect();
    // 三角形随便连：生成凸包只看顶点
    let indices = (0..points.len() as u32 - 2).flat_map(|i| [0, i + 1, i + 2]).collect();
    Mesh { vertices, indices, skin: Vec::new(), targets: Vec::new() }
}

fn positions(mesh: &Mesh) -> Vec<Vec3> {
    mesh.vertices.iter().map(|v| Vec3::from(v.position)).collect()
}

#[test]
fn a_convex_hull_keeps_only_the_outer_corners() {
    let mut assets = Assets3d::default();
    let cube = cube_model(&mut assets);
    let Ok(Shape::Convex(points)) = mesh_shape(&assets, cube, MeshShape::Convex, Vec3::ONE) else { panic!("凸包") };
    assert_eq!(points.len(), 8, "24 个顶点（每面 4 个）合成立方体的 8 个角");
    assert!(points.iter().all(|p| p.abs() == Vec3::ONE), "{points:?}");
    let Ok(Shape::Convex(points)) = mesh_shape(&assets, cube, MeshShape::Convex, Vec3::new(2.0, 1.0, 3.0)) else { panic!() };
    assert!(points.iter().all(|p| p.abs() == Vec3::new(2.0, 1.0, 3.0)), "按三个轴缩放 {points:?}");
    let r = model(&mut assets, vec![(rock(), Mat4::IDENTITY)]);
    let Ok(Shape::Convex(points)) = mesh_shape(&assets, r, MeshShape::Convex, Vec3::ONE) else { panic!() };
    assert_eq!(points.len(), 12, "里面的点不要");
}

#[test]
fn the_hull_contains_every_vertex() {
    let mut app = app();
    let mut assets = Assets3d::default();
    let r = model(&mut assets, vec![(rock(), Mat4::from_rotation_translation(Quat::from_rotation_x(0.3), Vec3::new(0.0, 0.5, 0.0)))]);
    let scale = Vec3::new(20.0, 30.0, 25.0);
    let shape = mesh_shape(&assets, r, MeshShape::Convex, scale).unwrap();
    let e = app.world_mut().spawn((Collider::new(shape), at(100.0, 0.0, 0.0))).id();
    steps(&mut app, 1);
    let local = Mat4::from_rotation_translation(Quat::from_rotation_x(0.3), Vec3::new(0.0, 0.5, 0.0));
    let physics = app.world().resource::<Physics3d>();
    for p in positions(&rock()) {
        // 每个顶点（按部件的 local 摆、再缩放）稍微往里收一点，都在凸包里面
        let world = Vec3::new(100.0, 0.0, 0.0) + local.transform_point3(p) * scale;
        let center = Vec3::new(100.0, 0.5 * 30.0, 0.0);
        let inside = center + (world - center) * 0.98;
        assert_eq!(physics.overlap_point(inside, QueryFilter::default()), vec![e], "{p} 在凸包里");
    }
    assert!(physics.overlap_point(Vec3::new(100.0 + 2.0 * 20.0 * 1.3, 0.0, 0.0), QueryFilter::default()).is_empty(), "外面的点不在");
}

#[test]
fn hulls_are_made_per_part_and_trimeshes_join_all_parts() {
    let mut assets = Assets3d::default();
    let two = model(&mut assets, vec![(Mesh::cube(2.0), Mat4::IDENTITY), (Mesh::cube(2.0), Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0)))]);
    let Ok(Shape::Hulls(parts)) = mesh_shape(&assets, two, MeshShape::Hulls, Vec3::splat(10.0)) else { panic!("每个部件一个凸包") };
    assert_eq!(parts.len(), 2);
    let center = |p: &Vec<Vec3>| p.iter().sum::<Vec3>() / p.len() as f32;
    assert!(center(&parts[0]).length() < 1e-4 && center(&parts[1]).distance(Vec3::new(50.0, 0.0, 0.0)) < 1e-4, "第二个按 local 挪过去、再缩放");
    let Ok(Shape::TriMesh(mesh)) = mesh_shape(&assets, two, MeshShape::TriMesh, Vec3::ONE) else { panic!("三角网格") };
    assert_eq!((mesh.0.len(), mesh.1.len()), (16, 24), "重复的顶点合并：两个立方体各 8 个角、12 个三角形");
    assert!(mesh.1.iter().flatten().all(|&i| (i as usize) < mesh.0.len()));
}

#[test]
fn flat_or_empty_models_make_no_shape() {
    let mut assets = Assets3d::default();
    let flat = Mesh {
        vertices: [[-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [1.0, 0.0, 1.0], [-1.0, 0.0, 1.0]].map(|p| Vertex { position: p, normal: [0.0, 1.0, 0.0], uv: [0.0; 2] }).to_vec(),
        indices: vec![0, 1, 2, 0, 2, 3],
        skin: Vec::new(),
        targets: Vec::new(),
    };
    let plane = model(&mut assets, vec![(flat, Mat4::IDENTITY)]);
    assert!(mesh_shape(&assets, plane, MeshShape::Convex, Vec3::ONE).is_err(), "平的生不出凸包");
    assert!(mesh_shape(&assets, plane, MeshShape::TriMesh, Vec3::ONE).is_ok(), "平的可以当三角网格（地面）");
    let empty = assets.add_model(Model::default()).unwrap();
    assert!(mesh_shape(&assets, empty, MeshShape::Convex, Vec3::ONE).is_err());
    let cube = cube_model(&mut assets);
    assert!(mesh_shape(&Assets3d::default(), cube, MeshShape::Convex, Vec3::ONE).is_err(), "没有这个模型");
    assert!(mesh_shape(&assets, cube, MeshShape::Convex, Vec3::new(1.0, 0.0, 1.0)).is_err(), "缩放成 0 的轴");
    assert!(mesh_shape(&assets, cube, MeshShape::Convex, Vec3::NAN).is_err());
}

#[test]
fn too_many_points_are_reported() {
    let many: Vec<Vec3> = (0..300).map(|i| {
        let (a, b) = (i as f32 * 2.399, (i as f32 / 300.0) * 2.0 - 1.0);
        Vec3::new(a.cos() * (1.0 - b * b).sqrt(), b, a.sin() * (1.0 - b * b).sqrt())
    }).collect();
    assert!(too_big(&Shape::Convex(many.clone().into())).is_some(), "球面上 300 个点的凸包超过 {MAX_HULL_POINTS}");
    assert!(too_big(&Shape::Convex(many[..8].to_vec().into())).is_none());
    assert!(too_big(&Shape::Hulls(vec![many[..8].to_vec(), many.clone()].into())).is_some(), "任何一个凸包超了都算");
    let tris = vec![[0u32, 1, 2]; MAX_TRIANGLES + 1];
    assert!(too_big(&Shape::TriMesh((many[..3].to_vec(), tris).into())).is_some());
    assert!(too_big(&Shape::Box(Vec3::ONE)).is_none());
}

#[test]
fn trimeshes_on_moving_bodies_become_hulls() {
    let mut app = app();
    ground(&mut app);
    let mut assets = Assets3d::default();
    let cube = cube_model(&mut assets);
    let shape = mesh_shape(&assets, cube, MeshShape::TriMesh, Vec3::splat(10.0)).unwrap();
    // 动态刚体上的三角网格：不换成凸包会穿过地面（三角网格没有里面）
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider::new(shape), at(0.0, 100.0, 0.0), Velocity::default())).id();
    steps(&mut app, 120);
    assert!((pos(&app, b).y - 10.0).abs() < 1.0, "落在地上 {}", pos(&app, b));
}

#[test]
fn mesh_colliders_wait_for_the_model_and_then_land() {
    let mut app = app();
    app.insert_resource(Assets3d::default());
    ground(&mut app);
    let cube = cube_model(&mut app.world_mut().resource_mut::<Assets3d>());
    let template = Collider { restitution: 0.25, group: 3, ..Collider::ball(1.0) };
    let mc = MeshCollider { kind: MeshShape::Convex, scale: Vec3::splat(10.0), collider: template };
    let b = app.world_mut().spawn((RigidBody::Dynamic, mc, at(0.0, 100.0, 0.0), Velocity::default())).id();
    steps(&mut app, 30);
    assert_eq!(pos(&app, b).y, 100.0, "模型还没加载：刚体先不建，不往下掉");
    assert_eq!(app.world().resource::<Physics3d>().len(), (1, 1), "只有地面");
    app.world_mut().entity_mut(b).insert(Model3d(cube));
    steps(&mut app, 1);
    let c = app.world().get::<Collider>(b).expect("加载完插上碰撞体").clone();
    assert!(matches!(&c.shape, Shape::Convex(p) if p.len() == 8));
    assert_eq!((c.restitution, c.group), (0.25, 3), "材质参数照 MeshCollider 的");
    steps(&mut app, 120);
    assert!((pos(&app, b).y - 10.0).abs() < 1.0, "凸包的箱子落在地上 {}", pos(&app, b));
    // 改缩放就重算
    app.world_mut().get_mut::<MeshCollider>(b).unwrap().scale = Vec3::splat(20.0);
    steps(&mut app, 1);
    assert!(matches!(&app.world().get::<Collider>(b).unwrap().shape, Shape::Convex(p) if p[0].abs() == Vec3::splat(20.0)));
}

fn flat_square() -> Mesh {
    Mesh {
        vertices: [[-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [1.0, 0.0, 1.0], [-1.0, 0.0, 1.0]].map(|p| Vertex { position: p, normal: [0.0, 1.0, 0.0], uv: [0.0; 2] }).to_vec(),
        indices: vec![0, 1, 2, 0, 2, 3],
        skin: Vec::new(),
        targets: Vec::new(),
    }
}

#[test]
fn flat_parts_are_skipped_when_making_hulls() {
    let mut assets = Assets3d::default();
    let mixed = model(&mut assets, vec![(Mesh::cube(2.0), Mat4::IDENTITY), (flat_square(), Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0)))]);
    let Ok(Shape::Hulls(parts)) = mesh_shape(&assets, mixed, MeshShape::Hulls, Vec3::ONE) else { panic!("平的部件跳过，剩下的照样生成") };
    assert_eq!(parts.len(), 1);
    assert!(parts[0].iter().all(|p| p.abs() == Vec3::ONE), "{parts:?}");
    let all_flat = model(&mut assets, vec![(flat_square(), Mat4::IDENTITY), (flat_square(), Mat4::from_translation(Vec3::Y))]);
    assert!(mesh_shape(&assets, all_flat, MeshShape::Hulls, Vec3::ONE).is_err(), "全是平的才算生不出来");
}

/// 三角形 (a, b, c) 的朝向：(b − a) × (c − a)。
fn facing(points: &[Vec3], t: [u32; 3]) -> Vec3 {
    let [a, b, c] = t.map(|i| points[i as usize]);
    (b - a).cross(c - a)
}

#[test]
fn mirrored_trimeshes_keep_their_triangles_facing_out() {
    let mut assets = Assets3d::default();
    let cube = cube_model(&mut assets);
    for scale in [Vec3::splat(10.0), Vec3::new(-10.0, 10.0, 10.0), Vec3::new(-10.0, -10.0, 10.0), Vec3::splat(-10.0)] {
        let Ok(Shape::TriMesh(mesh)) = mesh_shape(&assets, cube, MeshShape::TriMesh, scale) else { panic!() };
        for &t in &mesh.1 {
            let center = t.iter().map(|&i| mesh.0[i as usize]).sum::<Vec3>() / 3.0;
            assert!(facing(&mesh.0, t).dot(center) > 0.0, "缩放 {scale}：三角形 {t:?} 朝外");
        }
    }
}

#[test]
fn nearly_equal_vertices_are_merged() {
    let mut assets = Assets3d::default();
    // 两个三角形共一条边，但共用的两个顶点各写了两遍：一个是 0 和 -0，一个差一点点
    let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [-0.0, 0.0, -0.0], [1.0, 0.0, 1.0], [1.0 + 1e-7, 0.0, 0.0]];
    let mesh = Mesh {
        vertices: positions.map(|p| Vertex { position: p, normal: [0.0, 1.0, 0.0], uv: [0.0; 2] }).to_vec(),
        indices: vec![0, 2, 1, 5, 2, 4],
        skin: Vec::new(),
        targets: Vec::new(),
    };
    let quad = model(&mut assets, vec![(mesh, Mat4::IDENTITY)]);
    let Ok(Shape::TriMesh(mesh)) = mesh_shape(&assets, quad, MeshShape::TriMesh, Vec3::splat(100.0)) else { panic!() };
    assert_eq!(mesh.0.len(), 4, "-0 和 0、差 1e-7 的算同一个顶点 {:?}", mesh.0);
    let _ = positions[3];
}

#[test]
fn bad_shapes_given_directly_build_nothing() {
    let mut app = app();
    let shapes = [
        Shape::Convex(Vec::new().into()),
        Shape::Convex(vec![Vec3::ZERO, Vec3::X].into()),
        Shape::Convex(vec![Vec3::ZERO, Vec3::X, Vec3::Z, Vec3::new(1.0, 0.0, 1.0)].into()),
        Shape::Hulls(vec![vec![Vec3::ZERO, Vec3::X]].into()),
        Shape::TriMesh((vec![Vec3::ZERO, Vec3::X, Vec3::Z], vec![[0, 1, 9]]).into()),
    ];
    let bodies: Vec<Entity> = shapes.iter().map(|s| app.world_mut().spawn((RigidBody::Dynamic, Collider::new(s.clone()), at(0.0, 100.0, 0.0), Velocity::default())).id()).collect();
    let fixed = app.world_mut().spawn((RigidBody::Fixed, Collider::new(shapes[4].clone()), at(0.0, 0.0, 0.0))).id();
    steps(&mut app, 30);
    assert_eq!(app.world().resource::<Physics3d>().len(), (0, 0), "碰撞体生不出来的，刚体也不建（和等模型的一样）");
    for b in bodies {
        assert_eq!(pos(&app, b).y, 100.0, "不建就不往下掉");
    }
    // 改成能生成的形状就建
    app.world_mut().get_mut::<Collider>(fixed).unwrap().shape = Shape::TriMesh((vec![Vec3::ZERO, Vec3::X, Vec3::Z], vec![[0, 1, 2]]).into());
    steps(&mut app, 1);
    assert_eq!(app.world().resource::<Physics3d>().len(), (1, 1));
}

#[test]
fn pending_mesh_colliders_are_counted_and_long_waits_warned_once() {
    let mut app = app();
    app.insert_resource(Assets3d::default());
    let cube = cube_model(&mut app.world_mut().resource_mut::<Assets3d>());
    let mc = MeshCollider { kind: MeshShape::Convex, scale: Vec3::splat(10.0), collider: Collider::ball(1.0) };
    let b = app.world_mut().spawn((RigidBody::Dynamic, mc.clone(), at(0.0, 100.0, 0.0), Velocity::default())).id();
    steps(&mut app, 1);
    assert_eq!(app.world().resource::<Physics3d>().pending_meshes(), 1, "等着模型");
    assert!(!app.world().resource::<Physics3d>().warned(b), "刚开始等不报警");
    steps(&mut app, 5 * 60 + 1);
    assert!(app.world().resource::<Physics3d>().warned(b), "等了 5 秒报一次警");
    app.world_mut().entity_mut(b).insert(Model3d(cube));
    steps(&mut app, 1);
    assert_eq!(app.world().resource::<Physics3d>().pending_meshes(), 0, "加载完不再等");
    // 生不出来的（平的模型生凸包）不算在等：不然游戏会一直等下去
    let flat = model(&mut app.world_mut().resource_mut::<Assets3d>(), vec![(flat_square(), Mat4::IDENTITY)]);
    app.world_mut().spawn((RigidBody::Dynamic, mc, Model3d(flat), at(0.0, 100.0, 0.0), Velocity::default()));
    steps(&mut app, 1);
    assert_eq!(app.world().resource::<Physics3d>().pending_meshes(), 0, "生不出来的不算在等");
}

#[test]
fn changing_the_model_rebuilds_the_collider() {
    let mut app = app();
    app.insert_resource(Assets3d::default());
    let (small, big) = {
        let mut assets = app.world_mut().resource_mut::<Assets3d>();
        (model(&mut assets, vec![(Mesh::cube(2.0), Mat4::IDENTITY)]), model(&mut assets, vec![(Mesh::cube(6.0), Mat4::IDENTITY)]))
    };
    let mc = MeshCollider { kind: MeshShape::Convex, scale: Vec3::ONE, collider: Collider::ball(1.0) };
    let e = app.world_mut().spawn((RigidBody::Fixed, mc, Model3d(small), at(0.0, 0.0, 0.0))).id();
    steps(&mut app, 1);
    assert!(!app.world().resource::<Physics3d>().overlap_point(Vec3::new(2.0, 0.0, 0.0), QueryFilter::default()).contains(&e));
    app.world_mut().entity_mut(e).insert(Model3d(big));
    steps(&mut app, 1);
    assert!(matches!(&app.world().get::<Collider>(e).unwrap().shape, Shape::Convex(p) if p[0].abs() == Vec3::splat(3.0)), "换了模型就按新模型生成");
    assert!(app.world().resource::<Physics3d>().overlap_point(Vec3::new(2.0, 0.0, 0.0), QueryFilter::default()).contains(&e), "物理里也换了");
}
