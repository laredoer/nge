use bevy_ecs::message::Messages;
use glam::{Quat, Vec3};
use nge_core::Viewport;
use nge_ecs::App;
use nge_render3d::Transform3d;

use super::*;

pub(crate) const VIEWPORT: Viewport = Viewport { size: [100.0, 100.0], pixels: [100, 100] };

/// 按 point 用：重力 -981，一米 100。
pub(crate) fn app() -> App {
    let mut app = App::new();
    app.add_plugins(Physics3dPlugin { gravity: Vec3::new(0.0, -981.0, 0.0), length_unit: 100.0 });
    app
}

pub(crate) fn at(x: f32, y: f32, z: f32) -> Transform3d {
    Transform3d { translation: Vec3::new(x, y, z), ..Default::default() }
}

pub(crate) fn steps(app: &mut App, n: usize) {
    for _ in 0..n {
        app.tick(VIEWPORT, STEP);
    }
}

pub(crate) fn pos(app: &App, e: Entity) -> Vec3 {
    app.world().get::<Transform3d>(e).unwrap().translation
}

pub(crate) fn rot(app: &App, e: Entity) -> Quat {
    app.world().get::<Transform3d>(e).unwrap().rotation
}

/// 顶面在 y = 0、很大的地面。
pub(crate) fn ground(app: &mut App) -> Entity {
    app.world_mut().spawn((RigidBody::Fixed, Collider::cuboid(Vec3::new(2000.0, 20.0, 2000.0)), at(0.0, -10.0, 0.0))).id()
}

/// 20 见方的动态盒子。
pub(crate) fn crate_at(app: &mut App, x: f32, y: f32, z: f32) -> Entity {
    app.world_mut().spawn((RigidBody::Dynamic, Collider::cuboid(Vec3::splat(20.0)), at(x, y, z), Velocity::default())).id()
}

pub(crate) fn collisions(app: &mut App) -> Vec<Collision3d> {
    app.world_mut().resource_mut::<Messages<Collision3d>>().drain().collect()
}

#[test]
fn boxes_fall_and_rest_on_the_ground() {
    let mut app = app();
    let g = ground(&mut app);
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider { events: true, ..Collider::cuboid(Vec3::splat(20.0)) }, at(5.0, 100.0, -3.0))).id();
    let mut hits = Vec::new();
    for _ in 0..120 {
        steps(&mut app, 1);
        hits.extend(collisions(&mut app));
    }
    let p = pos(&app, b);
    assert!((p.y - 10.0).abs() < 0.5 && (p.x - 5.0).abs() < 0.5 && (p.z + 3.0).abs() < 0.5, "停在地上 {p}");
    assert!(hits.iter().any(|c| c.started && [c.a, c.b].contains(&g) && [c.a, c.b].contains(&b)), "落地有碰撞消息 {hits:?}");
    let contacts = app.world().resource::<Physics3d>().contacts(b);
    assert!(contacts.iter().any(|c| c.other == g && c.normal.y > 0.99), "法线朝上 {contacts:?}");
    let ground_side = app.world().resource::<Physics3d>().contacts(g);
    assert!(ground_side.iter().any(|c| c.other == b && c.normal.y < -0.99), "地面那边朝下 {ground_side:?}");
}

#[test]
fn sensors_report_overlap_without_blocking() {
    let mut app = app();
    ground(&mut app);
    let s = app.world_mut().spawn((Collider { sensor: true, events: true, ..Collider::cuboid(Vec3::splat(40.0)) }, at(0.0, 50.0, 0.0))).id();
    let b = crate_at(&mut app, 0.0, 120.0, 0.0);
    let mut hits = Vec::new();
    for _ in 0..120 {
        steps(&mut app, 1);
        hits.extend(collisions(&mut app));
    }
    assert!(pos(&app, b).y < 11.0, "穿过 sensor 落地 {}", pos(&app, b));
    assert!(hits.iter().any(|c| c.started && [c.a, c.b].contains(&s)), "{hits:?}");
    assert!(hits.iter().any(|c| !c.started && [c.a, c.b].contains(&s)), "离开也报 {hits:?}");
}

#[test]
fn a_tilted_box_tips_over() {
    let mut app = app();
    ground(&mut app);
    let tilt = Quat::from_rotation_z(30f32.to_radians());
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider::cuboid(Vec3::new(20.0, 60.0, 20.0)), Transform3d { translation: Vec3::new(0.0, 40.0, 0.0), rotation: tilt, ..Default::default() })).id();
    steps(&mut app, 240);
    // 立着的长边倒下来平躺：y 轴转到水平
    let up = rot(&app, b) * Vec3::Y;
    assert!(up.y.abs() < 0.1, "倒下了 {up}");
    assert!(pos(&app, b).y < 12.0, "躺在地上 {}", pos(&app, b));
}

#[test]
fn initial_velocity_angular_velocity_and_impulses() {
    let mut app = app();
    app.world_mut().resource_mut::<Physics3d>().set_gravity(Vec3::ZERO);
    let b = crate_at(&mut app, 0.0, 0.0, 0.0);
    app.world_mut().get_mut::<Velocity>(b).unwrap().linear = Vec3::new(0.0, 0.0, 60.0);
    steps(&mut app, 60);
    assert!((pos(&app, b).z - 60.0).abs() < 1.0, "按速度走 {}", pos(&app, b));
    assert_eq!(app.world().get::<Velocity>(b).unwrap().linear, Vec3::new(0.0, 0.0, 60.0), "速度写回");
    app.world_mut().get_mut::<Velocity>(b).unwrap().angular = Vec3::new(0.0, std::f32::consts::PI, 0.0);
    steps(&mut app, 30);
    let (axis, angle) = rot(&app, b).to_axis_angle();
    assert!((angle - std::f32::consts::FRAC_PI_2).abs() < 0.05 && axis.y > 0.99, "绕 y 转了 90° {axis} {angle}");
    let mass = 20f32.powi(3);
    app.world_mut().get_mut::<Velocity>(b).unwrap().linear = Vec3::ZERO;
    app.world_mut().entity_mut(b).insert(Impulse(Vec3::new(mass * 30.0, 0.0, 0.0)));
    steps(&mut app, 1);
    let v = app.world().get::<Velocity>(b).unwrap().linear;
    assert!((v.x - 30.0).abs() < 0.5, "冲量 / 质量 {v}");
    assert_eq!(app.world().get::<Impulse>(b).unwrap().0, Vec3::ZERO, "用掉清零");
}

#[test]
fn kinematic_bodies_glide_and_push_dynamic_ones() {
    let mut app = app();
    ground(&mut app);
    let k = app.world_mut().spawn((RigidBody::Kinematic, Collider::cuboid(Vec3::splat(20.0)), at(0.0, 10.0, 0.0))).id();
    let b = crate_at(&mut app, 40.0, 10.0, 0.0);
    steps(&mut app, 5);
    // 一帧走两步：挪的距离平分到两步里，第一步后在一半
    app.world_mut().get_mut::<Transform3d>(k).unwrap().translation.x += 10.0;
    app.tick(VIEWPORT, STEP * 2.0);
    assert!((pos(&app, k).x - 10.0).abs() < 1e-3);
    let speed = app.world().resource::<Physics3d>().velocity(k).unwrap().linear.x;
    assert!((speed - 300.0).abs() < 1.0, "每步 5，不是一步 10 {speed}");
    for _ in 0..60 {
        app.world_mut().get_mut::<Transform3d>(k).unwrap().translation.x += 2.0;
        steps(&mut app, 1);
    }
    assert!(pos(&app, b).x > 60.0, "推开了 {}", pos(&app, b));
    // 转也是平滑的
    app.world_mut().get_mut::<Transform3d>(k).unwrap().rotation = Quat::from_rotation_y(1.0);
    app.tick(VIEWPORT, STEP * 2.0);
    let w = app.world().resource::<Physics3d>().velocity(k).unwrap().angular.y;
    assert!((w - 30.0).abs() < 0.5, "每步 0.5 弧度 {w}");
}

#[test]
fn moving_a_dynamic_body_teleports_it() {
    let mut app = app();
    ground(&mut app);
    let b = crate_at(&mut app, 0.0, 10.0, 0.0);
    steps(&mut app, 5);
    let r = Quat::from_rotation_y(0.5);
    *app.world_mut().get_mut::<Transform3d>(b).unwrap() = Transform3d { translation: Vec3::new(300.0, 10.0, 7.0), rotation: r, ..Default::default() };
    steps(&mut app, 1);
    assert!((pos(&app, b) - Vec3::new(300.0, 10.0, 7.0)).length() < 0.5, "{}", pos(&app, b));
    assert!(rot(&app, b).angle_between(r) < 0.01);
}

#[test]
fn removing_components_and_entities_cleans_up() {
    let mut app = app();
    ground(&mut app);
    let b = crate_at(&mut app, 0.0, 50.0, 0.0);
    steps(&mut app, 1);
    assert_eq!(app.world().resource::<Physics3d>().len(), (2, 2));
    // 去掉刚体：剩下的碰撞体当成固定的，不再往下掉
    app.world_mut().entity_mut(b).remove::<RigidBody>();
    steps(&mut app, 2);
    let y = pos(&app, b).y;
    assert_eq!(app.world().resource::<Physics3d>().len(), (1, 2));
    steps(&mut app, 10);
    assert_eq!(pos(&app, b).y, y, "固定碰撞体不动");
    app.world_mut().despawn(b);
    steps(&mut app, 1);
    assert_eq!(app.world().resource::<Physics3d>().len(), (1, 1));
}

#[test]
fn changing_the_shape_rebuilds_the_collider() {
    let mut app = app();
    ground(&mut app);
    let b = crate_at(&mut app, 0.0, 10.0, 0.0);
    steps(&mut app, 30);
    app.world_mut().entity_mut(b).insert(Collider::ball(30.0));
    steps(&mut app, 60);
    assert!((pos(&app, b).y - 30.0).abs() < 0.5, "球半径 30 {}", pos(&app, b));
    app.world_mut().entity_mut(b).insert(Collider::capsule(20.0, 10.0));
    steps(&mut app, 60);
    // 胶囊竖着落下可能倒，至少不会比躺着更低
    assert!(pos(&app, b).y > 9.5, "{}", pos(&app, b));
}

#[test]
fn raycasts() {
    let mut app = app();
    let g = ground(&mut app);
    let b = crate_at(&mut app, 0.0, 10.0, 50.0);
    let s = app.world_mut().spawn((Collider { sensor: true, ..Collider::cuboid(Vec3::splat(10.0)) }, at(0.0, 10.0, 20.0))).id();
    steps(&mut app, 1);
    let physics = app.world().resource::<Physics3d>();
    let hit = physics.raycast(Vec3::new(0.0, 10.0, 0.0), Vec3::Z * 3.0, 100.0, QueryFilter::default()).unwrap();
    assert_eq!(hit.entity, b, "不算 sensor");
    assert!((hit.distance - 40.0).abs() < 0.1 && hit.normal.z < -0.99, "{hit:?}");
    assert!((hit.point - Vec3::new(0.0, 10.0, 40.0)).length() < 0.1);
    assert_eq!(physics.raycast(Vec3::new(0.0, 10.0, 0.0), Vec3::Z, 100.0, QueryFilter::default().with_sensors()).unwrap().entity, s);
    assert!(physics.raycast(Vec3::new(0.0, 10.0, 0.0), Vec3::Z, 30.0, QueryFilter::default()).is_none(), "够不着");
    // 起点在里面
    let inside = physics.raycast(Vec3::new(0.0, 10.0, 50.0), Vec3::X, 100.0, QueryFilter::default()).unwrap();
    assert_eq!((inside.entity, inside.distance, inside.normal), (b, 0.0, -Vec3::X));
    // 不算自己：往下打到地面
    let down = physics.raycast(Vec3::new(0.0, 10.0, 50.0), -Vec3::Y, 100.0, QueryFilter::default().exclude(b)).unwrap();
    assert_eq!(down.entity, g);
    assert!((down.distance - 10.0).abs() < 0.1 && down.normal.y > 0.99, "{down:?}");
    assert!(physics.raycast(Vec3::ZERO, Vec3::ZERO, 10.0, QueryFilter::default()).is_none());
    assert!(physics.raycast(Vec3::ZERO, Vec3::X, 0.0, QueryFilter::default()).is_none());
}

#[test]
fn restitution_bounces() {
    let mut app = app();
    ground(&mut app);
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider { restitution: 0.8, ..Collider::ball(10.0) }, at(0.0, 100.0, 0.0), Velocity::default())).id();
    let mut bounced = false;
    for _ in 0..90 {
        steps(&mut app, 1);
        bounced |= app.world().get::<Velocity>(b).unwrap().linear.y > 100.0;
    }
    assert!(bounced, "弹起来了");
}

#[test]
fn spawned_with_angular_velocity_spins() {
    let mut app = app();
    app.world_mut().resource_mut::<Physics3d>().set_gravity(Vec3::ZERO);
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider::cuboid(Vec3::splat(20.0)), at(0.0, 0.0, 0.0), Velocity { angular: Vec3::new(2.0, 0.0, 0.0), ..Default::default() })).id();
    steps(&mut app, 30);
    assert!(rot(&app, b).angle_between(Quat::IDENTITY) > 0.9, "转了 {}", rot(&app, b));
}

#[test]
fn massless_dynamic_bodies_still_fall() {
    let mut app = app();
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider { density: 0.0, ..Collider::cuboid(Vec3::splat(20.0)) }, at(0.0, 0.0, 0.0))).id();
    steps(&mut app, 10);
    assert!(pos(&app, b).y < -10.0, "{}", pos(&app, b));
}

#[test]
fn moving_a_bare_collider_moves_it() {
    let mut app = app();
    let c = app.world_mut().spawn((Collider::cuboid(Vec3::splat(20.0)), at(0.0, 0.0, 0.0))).id();
    steps(&mut app, 1);
    app.world_mut().get_mut::<Transform3d>(c).unwrap().translation = Vec3::new(100.0, 0.0, 0.0);
    steps(&mut app, 2);
    let hit = app.world().resource::<Physics3d>().raycast(Vec3::new(100.0, 50.0, 0.0), -Vec3::Y, 100.0, QueryFilter::default());
    assert_eq!(hit.map(|h| h.entity), Some(c));
}

#[test]
fn reinserting_the_same_collider_keeps_the_body() {
    let mut app = app();
    ground(&mut app);
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider { events: true, ..Collider::cuboid(Vec3::splat(20.0)) }, at(0.0, 10.0, 0.0))).id();
    steps(&mut app, 30);
    collisions(&mut app);
    let same = app.world().get::<Collider>(b).unwrap().clone();
    app.world_mut().entity_mut(b).insert(same);
    steps(&mut app, 5);
    assert!(collisions(&mut app).is_empty(), "没重建，不会重新开始接触");
}

#[test]
fn long_frames_catch_up_at_most_four_steps() {
    let mut app = app();
    let b = crate_at(&mut app, 0.0, 0.0, 0.0);
    app.tick(VIEWPORT, 1.0);
    let v = app.world().get::<Velocity>(b).unwrap().linear.y;
    assert!((v + 981.0 * STEP * MAX_STEPS as f32).abs() < 1.0, "{v}");
}

#[test]
fn rays_starting_inside_off_center() {
    let mut app = app();
    let b = crate_at(&mut app, 0.0, 0.0, 0.0);
    app.world_mut().resource_mut::<Physics3d>().set_gravity(Vec3::ZERO);
    steps(&mut app, 1);
    let hit = app.world().resource::<Physics3d>().raycast(Vec3::new(5.0, 3.0, 0.0), Vec3::X, 100.0, QueryFilter::default()).unwrap();
    assert_eq!((hit.entity, hit.distance, hit.normal), (b, 0.0, -Vec3::X));
}

#[test]
fn despawning_reports_stopped_contacts() {
    let mut app = app();
    let g = ground(&mut app);
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider { events: true, ..Collider::cuboid(Vec3::splat(20.0)) }, at(0.0, 10.0, 0.0))).id();
    steps(&mut app, 10);
    collisions(&mut app);
    app.world_mut().despawn(b);
    // 消息只留两帧：每步都取
    let mut hits = Vec::new();
    for _ in 0..2 {
        steps(&mut app, 1);
        hits.extend(collisions(&mut app));
    }
    assert!(hits.iter().any(|c| !c.started && [c.a, c.b].contains(&g) && [c.a, c.b].contains(&b)), "{hits:?}");
}

#[test]
fn unnormalized_rotations_are_normalized() {
    let mut app = app();
    let b = app.world_mut().spawn((RigidBody::Fixed, Collider::cuboid(Vec3::splat(20.0)), Transform3d { rotation: Quat::from_xyzw(0.0, 0.0, 0.0, 3.0), ..Default::default() })).id();
    steps(&mut app, 1);
    assert!((rot(&app, b).length() - 1.0).abs() < 1e-4, "{}", rot(&app, b));
}
