//! 空间查询和碰撞分组。

use glam::{Quat, Vec3};

use super::*;
use crate::tests::{app, at, ground, pos, steps};

/// 固定的盒子：中心、整个长宽高、分组、是不是 sensor。
fn block(app: &mut nge_ecs::App, center: Vec3, size: Vec3, group: u8, sensor: bool) -> Entity {
    let c = Collider { group, sensor, ..Collider::cuboid(size) };
    app.world_mut().spawn((RigidBody::Fixed, c, at(center.x, center.y, center.z))).id()
}

fn physics(app: &nge_ecs::App) -> &Physics3d {
    app.world().resource::<Physics3d>()
}

fn sorted(mut v: Vec<Entity>) -> Vec<Entity> {
    v.sort();
    v
}

#[test]
fn shapes_cast_forward_and_stop_at_the_first_surface() {
    let mut app = app();
    let wall = block(&mut app, Vec3::new(100.0, 0.0, 0.0), Vec3::new(20.0, 100.0, 100.0), 0, false);
    let far = block(&mut app, Vec3::new(300.0, 0.0, 0.0), Vec3::new(20.0, 100.0, 100.0), 0, false);
    steps(&mut app, 1);
    let p = physics(&app);
    let f = QueryFilter::default();
    // 半径 10 的球：球心走到 90 - 10 = 80 时碰到墙面 x = 90
    let hit = p.cast_shape(QueryShape::Ball(10.0), Vec3::ZERO, Quat::IDENTITY, Vec3::X * 3.0, 500.0, f).expect("碰到墙");
    assert_eq!(hit.entity, wall);
    assert!((hit.distance - 80.0).abs() < 0.5, "{hit:?}");
    assert!((hit.point.x - 90.0).abs() < 0.5 && hit.normal.distance(Vec3::NEG_X) < 1e-3, "碰撞点在墙面上，法线朝回 {hit:?}");
    // 胶囊斜着放平（绕 z 转 90°）：半长 = 半高 + 半径 = 25
    let lying = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let hit = p.cast_shape(QueryShape::Capsule { half_height: 20.0, radius: 5.0 }, Vec3::ZERO, lying, Vec3::X, 500.0, f).unwrap();
    assert!((hit.distance - 65.0).abs() < 0.5, "躺着的胶囊在 90 - 25 碰到 {hit:?}");
    let hit = p.cast_shape(QueryShape::Box(Vec3::splat(40.0)), Vec3::ZERO, Quat::IDENTITY, Vec3::X, 500.0, f).unwrap();
    assert!((hit.distance - 70.0).abs() < 0.5, "{hit:?}");
    // 够不着、方向反了
    assert!(p.cast_shape(QueryShape::Ball(10.0), Vec3::ZERO, Quat::IDENTITY, Vec3::X, 50.0, f).is_none());
    assert!(p.cast_shape(QueryShape::Ball(10.0), Vec3::ZERO, Quat::IDENTITY, Vec3::NEG_X, 500.0, f).is_none());
    // 跳过第一面墙
    let hit = p.cast_shape(QueryShape::Ball(10.0), Vec3::ZERO, Quat::IDENTITY, Vec3::X, 500.0, f.exclude(wall)).unwrap();
    assert_eq!(hit.entity, far);
}

#[test]
fn a_cast_that_starts_inside_hits_at_once() {
    let mut app = app();
    let wall = block(&mut app, Vec3::new(10.0, 0.0, 0.0), Vec3::new(20.0, 100.0, 100.0), 0, false);
    steps(&mut app, 1);
    // 往里、往外、横着扫都一样：一开始就碰到
    for (x, dir) in [(0.0, Vec3::X), (-5.0, Vec3::NEG_X), (5.0, Vec3::Y), (10.0, Vec3::X)] {
        let hit = physics(&app).cast_shape(QueryShape::Ball(15.0), Vec3::new(x, 0.0, 0.0), Quat::IDENTITY, dir, 100.0, QueryFilter::default()).unwrap();
        assert_eq!((hit.entity, hit.distance), (wall, 0.0), "x = {x}");
        assert!(hit.normal.distance(-dir) < 1e-3 && hit.point == Vec3::new(x, 0.0, 0.0), "法线迎着方向，碰撞点是起点 {hit:?}");
    }
}

#[test]
fn bad_query_input_finds_nothing() {
    let mut app = app();
    block(&mut app, Vec3::ZERO, Vec3::splat(100.0), 0, false);
    steps(&mut app, 1);
    let (p, f) = (physics(&app), QueryFilter::default());
    let ball = QueryShape::Ball(5.0);
    assert!(p.cast_shape(ball, Vec3::NAN, Quat::IDENTITY, Vec3::X, 10.0, f).is_none());
    assert!(p.cast_shape(ball, Vec3::ZERO, Quat::IDENTITY, Vec3::ZERO, 10.0, f).is_none());
    assert!(p.cast_shape(ball, Vec3::ZERO, Quat::IDENTITY, Vec3::X, 0.0, f).is_none());
    assert!(p.cast_shape(QueryShape::Ball(-1.0), Vec3::ZERO, Quat::IDENTITY, Vec3::X, 10.0, f).is_none());
    assert!(p.overlap(QueryShape::Box(Vec3::new(1.0, 0.0, 1.0)), Vec3::ZERO, Quat::IDENTITY, f).is_empty());
    assert!(p.overlap(ball, Vec3::ZERO, Quat::from_xyzw(0.0, 0.0, 0.0, 0.0), f).is_empty());
    assert!(p.overlap_point(Vec3::INFINITY, f).is_empty());
    assert!(p.closest_point(Vec3::NAN, 10.0, f).is_none());
    assert!(p.closest_point(Vec3::ZERO, -1.0, f).is_none());
}

#[test]
fn overlaps_find_every_body_in_the_shape() {
    let mut app = app();
    let a = block(&mut app, Vec3::new(0.0, 0.0, 0.0), Vec3::splat(10.0), 0, false);
    let b = block(&mut app, Vec3::new(30.0, 0.0, 0.0), Vec3::splat(10.0), 0, false);
    let s = block(&mut app, Vec3::new(0.0, 30.0, 0.0), Vec3::splat(10.0), 0, true);
    block(&mut app, Vec3::new(200.0, 0.0, 0.0), Vec3::splat(10.0), 0, false);
    steps(&mut app, 1);
    let (p, f) = (physics(&app), QueryFilter::default());
    assert_eq!(sorted(p.overlap(QueryShape::Ball(40.0), Vec3::ZERO, Quat::IDENTITY, f)), sorted(vec![a, b]), "sensor 默认不算");
    assert_eq!(sorted(p.overlap(QueryShape::Ball(40.0), Vec3::ZERO, Quat::IDENTITY, f.with_sensors())), sorted(vec![a, b, s]));
    // 细长的盒子转 90°（绕 z）：从沿 x 变成沿 y，碰到 a 和上面的 sensor，碰不到右边的 b
    let bar = QueryShape::Box(Vec3::new(80.0, 2.0, 2.0));
    assert_eq!(sorted(p.overlap(bar, Vec3::ZERO, Quat::IDENTITY, f.with_sensors())), sorted(vec![a, b]));
    assert_eq!(sorted(p.overlap(bar, Vec3::ZERO, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), f.with_sensors())), sorted(vec![a, s]));
    assert_eq!(p.overlap_point(Vec3::new(31.0, 2.0, -3.0), f), vec![b]);
    assert!(p.overlap_point(Vec3::new(15.0, 0.0, 0.0), f).is_empty());
    assert_eq!(p.overlap(QueryShape::Capsule { half_height: 10.0, radius: 3.0 }, Vec3::new(30.0, 15.0, 0.0), Quat::IDENTITY, f), vec![b]);
}

#[test]
fn closest_points_lie_on_the_nearest_surface() {
    let mut app = app();
    let a = block(&mut app, Vec3::ZERO, Vec3::splat(20.0), 0, false);
    let b = block(&mut app, Vec3::new(100.0, 0.0, 0.0), Vec3::splat(20.0), 0, false);
    steps(&mut app, 1);
    let (p, f) = (physics(&app), QueryFilter::default());
    let hit = p.closest_point(Vec3::new(70.0, 0.0, 0.0), 100.0, f).unwrap();
    assert_eq!(hit.entity, b, "离 b 的面 20，离 a 的面 60");
    assert!(hit.point.distance(Vec3::new(90.0, 0.0, 0.0)) < 1e-3 && (hit.distance - 20.0).abs() < 1e-3 && !hit.inside, "{hit:?}");
    assert!(p.closest_point(Vec3::new(50.0, 0.0, 0.0), 30.0, f).is_none(), "最远查 30，两边都差 40");
    let inside = p.closest_point(Vec3::new(2.0, 1.0, 0.0), 10.0, f).unwrap();
    assert_eq!((inside.entity, inside.inside, inside.distance), (a, true, 0.0));
    assert_eq!(inside.point, Vec3::new(2.0, 1.0, 0.0));
    assert_eq!(p.closest_point(Vec3::new(70.0, 0.0, 0.0), 100.0, f.exclude(b)).unwrap().entity, a);
}

#[test]
fn queries_filter_by_collision_group() {
    let mut app = app();
    let loose = block(&mut app, Vec3::new(0.0, 0.0, 0.0), Vec3::splat(10.0), 0, false);
    let red = block(&mut app, Vec3::new(20.0, 0.0, 0.0), Vec3::splat(10.0), 1, false);
    let blue = block(&mut app, Vec3::new(40.0, 0.0, 0.0), Vec3::splat(10.0), 2, false);
    steps(&mut app, 1);
    let (p, f) = (physics(&app), QueryFilter::default());
    let all = |f: QueryFilter| sorted(p.overlap(QueryShape::Ball(100.0), Vec3::ZERO, Quat::IDENTITY, f));
    assert_eq!(all(f), sorted(vec![loose, red, blue]));
    assert_eq!(all(f.group(1)), sorted(vec![loose, blue]), "像分组 1 一样查：跳过同组的");
    assert_eq!(all(f.only(1 << 2)), vec![blue], "只查分组 2");
    assert_eq!(all(f.only(1)), vec![loose], "第 0 位是不分组的");
    assert_eq!(all(f.only(1 | 1 << 1).group(1)), vec![loose], "两个一起用");
    // 射线也按分组过滤
    let ray = |f: QueryFilter| p.raycast(Vec3::new(-50.0, 0.0, 0.0), Vec3::X, 200.0, f).map(|h| h.entity);
    assert_eq!(ray(f), Some(loose));
    assert_eq!(ray(f.only(1 << 1 | 1 << 2)), Some(red));
    assert_eq!(ray(f.only(1 << 2)), Some(blue));
}

#[test]
fn colliders_in_the_same_group_pass_through_each_other() {
    let mut app = app();
    ground(&mut app);
    let mut spawn = |group: u8, x: f32, y: f32| {
        let c = Collider { group, ..Collider::cuboid(Vec3::splat(20.0)) };
        app.world_mut().spawn((RigidBody::Dynamic, c, at(x, y, 0.0), Velocity::default())).id()
    };
    // 同组的两个叠着：上面的穿过下面的落到地上；不同组、不分组的照常叠
    let (low, high) = (spawn(3, 0.0, 10.0), spawn(3, 0.0, 60.0));
    let (other_low, other_high) = (spawn(3, 100.0, 10.0), spawn(4, 100.0, 60.0));
    let (loose_low, loose_high) = (spawn(0, 200.0, 10.0), spawn(3, 200.0, 60.0));
    steps(&mut app, 90);
    assert!((pos(&app, high).y - 10.0).abs() < 2.0, "同组的穿下去 {}", pos(&app, high));
    assert!((pos(&app, other_high).y - 30.0).abs() < 2.0 && (pos(&app, loose_high).y - 30.0).abs() < 2.0, "不同组、不分组的摞着");
    let _ = (low, other_low, loose_low);
}

