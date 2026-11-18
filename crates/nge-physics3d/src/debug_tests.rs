//! 物理调试视图：碰撞体轮廓、接触点、关节、射线和形状投射。

use bevy_ecs::system::RunSystemOnce;
use glam::{Quat, Vec3, Vec4};
use nge_render3d::{Gizmos3d, Lines};

use super::*;
use crate::debug::{CONTACT, DYNAMIC, FIXED, HIT, JOINT, KINEMATIC, MISS, NORMAL, SENSOR};
use crate::tests::{app, at, ground, steps};

fn debug_app(on_top: bool) -> nge_ecs::App {
    let mut app = app();
    app.init_resource::<Gizmos3d>().insert_resource(Physics3dDebug { enabled: true, on_top });
    app
}

/// 清空画线再走一步（没有渲染器时没人清）。
fn frame(app: &mut nge_ecs::App) {
    app.world_mut().resource_mut::<Gizmos3d>().clear();
    steps(app, 1);
}

fn segments(lines: &Lines, color: Vec4) -> Vec<(Vec3, Vec3)> {
    (0..lines.len()).filter_map(|i| lines.segment(i)).filter(|s| s.2 == color).map(|s| (s.0, s.1)).collect()
}

fn drawn(app: &nge_ecs::App, color: Vec4) -> Vec<(Vec3, Vec3)> {
    segments(app.world().resource::<Gizmos3d>().depth_tested(), color)
}

#[test]
fn nothing_is_drawn_or_traced_while_off() {
    let mut app = app();
    app.init_resource::<Gizmos3d>().init_resource::<Physics3dDebug>();
    ground(&mut app);
    steps(&mut app, 2);
    assert!(app.world().resource::<Gizmos3d>().is_empty());
    app.world().resource::<Physics3d>().raycast(Vec3::new(0.0, 100.0, 0.0), Vec3::NEG_Y, 200.0, QueryFilter::default());
    assert!(app.world().resource::<Physics3d>().traces.lock().unwrap().is_empty(), "关着时查询不记");
}

#[test]
fn outlines_are_colored_by_kind() {
    let mut app = debug_app(false);
    ground(&mut app);
    app.world_mut().spawn((RigidBody::Dynamic, Collider::ball(10.0), at(0.0, 100.0, 0.0), Velocity::default()));
    app.world_mut().spawn((RigidBody::Kinematic, Collider::capsule(10.0, 5.0), at(100.0, 50.0, 0.0)));
    app.world_mut().spawn((Collider { sensor: true, ..Collider::cuboid(Vec3::splat(10.0)) }, at(-100.0, 50.0, 0.0)));
    frame(&mut app);
    assert_eq!(drawn(&app, FIXED).len(), 12, "地面一个盒子 12 条边");
    assert_eq!(drawn(&app, SENSOR).len(), 12);
    assert_eq!(drawn(&app, DYNAMIC).len(), 3 * CIRCLE_SEGMENTS + 1, "球是三个圆加一条半径");
    assert!(drawn(&app, DYNAMIC).iter().all(|(_, b)| (b.distance(Vec3::new(0.0, 100.0, 0.0)) - 10.0).abs() < 0.5), "画在物理里的位置");
    assert!(!drawn(&app, KINEMATIC).is_empty());
    // 下一帧位置变了，轮廓跟着走
    steps(&mut app, 10);
    frame(&mut app);
    assert!(drawn(&app, DYNAMIC).iter().all(|(a, _)| a.y < 100.0));
}

#[test]
fn hulls_and_trimeshes_draw_their_edges() {
    let mut app = debug_app(false);
    let corners: Vec<Vec3> = (0..8).map(|i| Vec3::new(((i & 1) * 2) as f32 - 1.0, ((i & 2) as f32) - 1.0, ((i & 4) / 2) as f32 - 1.0) * 10.0).collect();
    app.world_mut().spawn((Collider::new(Shape::Convex(corners.clone().into())), at(0.0, 0.0, 0.0)));
    let tris = vec![[0u32, 1, 2], [1, 3, 2]];
    app.world_mut().spawn((Collider::new(Shape::TriMesh((corners[..4].to_vec(), tris).into())), at(0.0, -50.0, 0.0)));
    frame(&mut app);
    let edges = drawn(&app, FIXED);
    let (hull, mesh): (Vec<_>, Vec<_>) = edges.iter().partition(|(a, _)| a.y > -20.0);
    assert!(hull.len() >= 12, "立方体的凸包至少 12 条边 {}", hull.len());
    assert!(hull.iter().all(|(a, b)| a.abs() == Vec3::splat(10.0) && b.abs() == Vec3::splat(10.0)), "边连着角");
    assert_eq!(mesh.len(), 6, "两个三角形各 3 条边");
}

#[test]
fn contacts_joints_and_characters_are_drawn() {
    let mut app = debug_app(false);
    ground(&mut app);
    let a = app.world_mut().spawn((RigidBody::Fixed, at(0.0, 300.0, 0.0))).id();
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider::ball(5.0), at(50.0, 300.0, 0.0), Velocity::default())).id();
    let joint = app.world_mut().spawn(Joint3d::new(Some(a), b, Joint3dKind::Ball)).id();
    app.world_mut().spawn((RigidBody::Dynamic, Collider::cuboid(Vec3::splat(20.0)), at(200.0, 10.0, 0.0), Velocity::default()));
    app.world_mut().spawn((RigidBody::Kinematic, Character3d::default(), Collider::capsule(10.0, 5.0), at(-200.0, 15.5, 0.0), Velocity::default()));
    steps(&mut app, 20);
    frame(&mut app);
    assert!(app.world().resource::<Physics3d>().is_linked(joint));
    assert_eq!(drawn(&app, JOINT).len(), 1, "关节两个锚点之间一条线");
    let contacts = drawn(&app, CONTACT);
    assert!(contacts.iter().any(|(p, _)| (p.x - 200.0).abs() < 15.0 && p.y.abs() < 2.0), "箱子压在地上的接触点 {contacts:?}");
    assert!(contacts.iter().any(|(p, _)| (p.x + 200.0).abs() < 8.0 && p.y.abs() < 2.0), "角色站着的接触点 {contacts:?}");
    let axes = app.world().resource::<Gizmos3d>();
    assert!(segments(axes.depth_tested(), Vec4::new(1.0, 0.2, 0.2, 1.0)).iter().any(|(a, _)| (a.x + 200.0).abs() < 1.0), "角色画坐标轴");
}

#[test]
fn rays_and_casts_of_this_frame_are_drawn_once() {
    let mut app = debug_app(true);
    ground(&mut app);
    steps(&mut app, 1);
    app.world_mut().run_system_once(|physics: Res<Physics3d>| {
        physics.raycast(Vec3::new(0.0, 100.0, 0.0), Vec3::NEG_Y, 500.0, QueryFilter::default());
        physics.raycast(Vec3::new(0.0, 100.0, 0.0), Vec3::Y, 50.0, QueryFilter::default());
        physics.cast_shape(QueryShape::Ball(10.0), Vec3::new(50.0, 100.0, 0.0), Quat::IDENTITY, Vec3::NEG_Y, 500.0, QueryFilter::default());
    }).unwrap();
    frame(&mut app);
    let g = app.world().resource::<Gizmos3d>();
    assert!(g.depth_tested().is_empty(), "on_top 时全画在最前面");
    let top = g.top();
    let hits = segments(top, HIT);
    assert!(hits.iter().any(|(a, b)| a.y == 100.0 && b.y.abs() < 0.01 && b.x == 0.0), "射线红线到碰撞点 {hits:?}");
    assert!(segments(top, NORMAL).iter().any(|(a, b)| a.y.abs() < 0.01 && (b - *a).normalize().abs_diff_eq(Vec3::Y, 1e-3)), "绿的法线朝上");
    assert!(segments(top, MISS).iter().any(|(a, b)| a.y == 100.0 && (b.y - 150.0).abs() < 0.01), "没碰到的暗红");
    assert!(hits.iter().filter(|(a, _)| (a.distance(Vec3::new(50.0, 10.0, 0.0)) - 10.0).abs() < 0.5).count() >= CIRCLE_SEGMENTS, "形状投射在终点画个球");
    frame(&mut app);
    assert!(segments(app.world().resource::<Gizmos3d>().top(), HIT).is_empty(), "下一帧没查就没有");
}

use nge_render3d::CIRCLE_SEGMENTS;
