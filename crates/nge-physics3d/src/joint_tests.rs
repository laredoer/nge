//! 3D 关节（按 point 用：重力 -981，一米 100）。

use std::f32::consts::{FRAC_PI_2, PI};

use glam::{Quat, Vec3};
use nge_render3d::Transform3d;

use super::*;
use crate::tests::{app, at, pos, rot, steps};

fn ball(app: &mut nge_ecs::App, p: Vec3) -> Entity {
    app.world_mut().spawn((RigidBody::Dynamic, Collider::ball(10.0), at(p.x, p.y, p.z), Velocity::default())).id()
}

fn joint(app: &mut nge_ecs::App, j: Joint3d) -> Entity {
    app.world_mut().spawn(j).id()
}

fn physics(app: &nge_ecs::App) -> &Physics3d {
    app.world().resource::<Physics3d>()
}

/// 一扇门：竖着的薄板（宽 100、高 200、厚 10），中心在 (50, 100, 0)，左边（x = 0）挂在世界的竖轴上。
fn door(app: &mut nge_ecs::App, limits: Option<[f32; 2]>, motor: Option<Motor3d>) -> (Entity, Entity) {
    let d = app.world_mut().spawn((RigidBody::Dynamic, Collider::cuboid(Vec3::new(100.0, 200.0, 10.0)), at(50.0, 100.0, 0.0), Velocity::default())).id();
    let kind = Joint3dKind::Revolute { axis: Vec3::Y, limits, motor };
    let j = joint(app, Joint3d { anchor_a: Vec3::new(0.0, 100.0, 0.0), anchor_b: Vec3::new(-50.0, 0.0, 0.0), ..Joint3d::new(None, d, kind) });
    (d, j)
}

/// 门绕竖轴转过的角度（从 +x 往 -z 转是正的）。
fn yaw(app: &nge_ecs::App, e: Entity) -> f32 {
    let (axis, angle) = rot(app, e).to_axis_angle();
    if axis.y < 0.0 { -angle } else { angle }
}

#[test]
fn hinge_swings_and_stops_at_its_limits() {
    let mut app = app();
    let (d, j) = door(&mut app, Some([-0.5, 0.5]), None);
    steps(&mut app, 1);
    assert!(physics(&app).is_linked(j) && physics(&app).joints() == 1);
    // 推门：门往 -z 甩
    app.world_mut().get_mut::<Velocity>(d).unwrap().angular = Vec3::Y * 6.0;
    steps(&mut app, 60);
    let a = yaw(&app, d);
    assert!((a - 0.5).abs() < 0.05, "停在上限 0.5 弧度 {a}");
    // 门不掉下去：铰链只让它绕竖轴转
    let hinge = pos(&app, d) + rot(&app, d) * Vec3::new(-50.0, 0.0, 0.0);
    assert!(hinge.distance(Vec3::new(0.0, 100.0, 0.0)) < 1.0, "转轴还在原处 {hinge}");
    app.world_mut().get_mut::<Velocity>(d).unwrap().angular = Vec3::Y * -12.0;
    steps(&mut app, 60);
    assert!((yaw(&app, d) + 0.5).abs() < 0.05, "另一边停在 -0.5 {}", yaw(&app, d));
}

#[test]
fn hinge_motor_turns_the_door_and_updates_in_place() {
    let mut app = app();
    let (d, j) = door(&mut app, None, Some(Motor3d { speed: 1.0, max_force: f32::INFINITY }));
    steps(&mut app, 30);
    let w = app.world().get::<Velocity>(d).unwrap().angular;
    assert!((w.y - 1.0).abs() < 0.05, "马达把门转到 1 弧度 / 秒 {w}");
    // 只改马达速度：原地换，不重建
    let set = |app: &mut nge_ecs::App, speed: f32| {
        let mut joint = app.world_mut().get_mut::<Joint3d>(j).unwrap();
        joint.kind = Joint3dKind::Revolute { axis: Vec3::Y, limits: None, motor: Some(Motor3d { speed, max_force: f32::INFINITY }) };
    };
    set(&mut app, -2.0);
    steps(&mut app, 30);
    assert!((app.world().get::<Velocity>(d).unwrap().angular.y + 2.0).abs() < 0.05);
    assert_eq!(physics(&app).joints(), 1, "没有多出关节");
}

#[test]
fn hinge_limits_start_from_the_declared_rotation() {
    let mut app = app();
    // a 是转过 90° 的固定刚体，门挂在它上面；声明时门相对 a 转了 -90°（门和世界一样朝）
    let frame = app.world_mut().spawn((RigidBody::Fixed, Collider::cuboid(Vec3::splat(10.0)), Transform3d { rotation: Quat::from_rotation_y(FRAC_PI_2), ..at(0.0, 100.0, 0.0) })).id();
    let d = app.world_mut().spawn((RigidBody::Dynamic, Collider::cuboid(Vec3::new(100.0, 200.0, 10.0)), at(60.0, 100.0, 0.0), Velocity::default())).id();
    let kind = Joint3dKind::Revolute { axis: Vec3::Y, limits: Some([0.0, 0.3]), motor: Some(Motor3d { speed: 3.0, max_force: f32::INFINITY }) };
    let j = Joint3d {
        a: Some(frame),
        anchor_a: Vec3::ZERO,
        anchor_b: Vec3::new(-60.0, 0.0, 0.0),
        rotation: Quat::from_rotation_y(-FRAC_PI_2),
        ..Joint3d::new(Some(frame), d, kind)
    };
    joint(&mut app, j);
    steps(&mut app, 60);
    let a = yaw(&app, d);
    assert!((a - 0.3).abs() < 0.05, "从声明的朝向（0）转到上限 0.3，不是从 a 的朝向算 {a}");
}

#[test]
fn ball_joints_keep_the_distance_and_swing_freely() {
    let mut app = app();
    let b = ball(&mut app, Vec3::new(100.0, 0.0, 50.0));
    joint(&mut app, Joint3d { anchor_a: Vec3::ZERO, anchor_b: Vec3::new(-100.0, 0.0, -50.0), ..Joint3d::new(None, b, Joint3dKind::Ball) });
    let length = Vec3::new(100.0, 0.0, 50.0).length();
    let mut lowest = 0.0f32;
    for _ in 0..30 {
        steps(&mut app, 2);
        assert!((pos(&app, b).length() - length).abs() < 1.5, "绕支点甩，离支点的距离不变 {}", pos(&app, b));
        lowest = lowest.min(pos(&app, b).y);
    }
    assert!(lowest < -length + 5.0, "甩到最低点 {lowest}");
}

#[test]
fn fixed_joints_move_together_and_keep_the_declared_rotation() {
    let mut app = app();
    let a = ball(&mut app, Vec3::ZERO);
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider::cuboid(Vec3::splat(10.0)), Transform3d { rotation: Quat::from_rotation_x(0.5), ..at(0.0, 0.0, 40.0) }, Velocity::default())).id();
    joint(&mut app, Joint3d { anchor_b: Vec3::ZERO, anchor_a: Vec3::new(0.0, 0.0, 40.0), rotation: Quat::from_rotation_x(0.5), ..Joint3d::new(Some(a), b, Joint3dKind::Fixed) });
    app.world_mut().get_mut::<Velocity>(a).unwrap().linear = Vec3::new(50.0, 0.0, 0.0);
    steps(&mut app, 40);
    assert!((pos(&app, b) - pos(&app, a)).distance(rot(&app, a) * Vec3::new(0.0, 0.0, 40.0)) < 1.0, "一起走");
    assert!((rot(&app, a).inverse() * rot(&app, b)).angle_between(Quat::from_rotation_x(0.5)) < 0.02, "保持声明的相对朝向");
}

#[test]
fn prismatic_joints_slide_along_the_axis_within_limits() {
    let mut app = app();
    let b = ball(&mut app, Vec3::ZERO);
    let kind = Joint3dKind::Prismatic { axis: Vec3::new(0.0, 1.0, 1.0), limits: Some([-50.0, 30.0]), motor: None };
    joint(&mut app, Joint3d { anchor_a: Vec3::ZERO, anchor_b: Vec3::ZERO, ..Joint3d::new(None, b, kind) });
    steps(&mut app, 60);
    let p = pos(&app, b);
    let along = Vec3::new(0.0, 1.0, 1.0).normalize();
    assert!(p.distance(along * -50.0) < 1.5, "沿斜着的轴滑到下限 {p}");
    // 马达往上推
    let kind = Joint3dKind::Prismatic { axis: Vec3::new(0.0, 1.0, 1.0), limits: Some([-50.0, 30.0]), motor: Some(Motor3d { speed: 200.0, max_force: f32::INFINITY }) };
    app.world_mut().query::<&mut Joint3d>().single_mut(app.world_mut()).unwrap().kind = kind;
    steps(&mut app, 60);
    assert!(pos(&app, b).distance(along * 30.0) < 1.5, "推到上限 {}", pos(&app, b));
}

#[test]
fn ropes_hang_taut_and_springs_settle_below_their_length() {
    let mut app = app();
    let r = ball(&mut app, Vec3::new(0.0, -20.0, 0.0));
    joint(&mut app, Joint3d { anchor_a: Vec3::ZERO, ..Joint3d::new(None, r, Joint3dKind::Rope { length: 100.0 }) });
    let s = ball(&mut app, Vec3::new(500.0, -50.0, 0.0));
    joint(&mut app, Joint3d { anchor_a: Vec3::new(500.0, 0.0, 0.0), ..Joint3d::new(None, s, Joint3dKind::Spring { length: 50.0, stiffness: 100.0, damping: 20.0 }) });
    steps(&mut app, 20);
    // 自由落体 1/3 秒：往下 ½ × 981 × (1/3)² ≈ 54
    assert!((pos(&app, r).y + 20.0 + 54.5).abs() < 3.0, "绳子松着时自由下落 {}", pos(&app, r));
    steps(&mut app, 300);
    assert!((pos(&app, r).y + 100.0).abs() < 1.5, "拉直后挂住 {}", pos(&app, r));
    // 静止长度 50 往下再 g / k = 9.81
    let y = pos(&app, s).y;
    assert!((y - (-50.0 - 981.0 / 100.0)).abs() < 1.5, "弹簧停在 {y}");
}

#[test]
fn bad_parameters_do_not_link_until_fixed() {
    let mut app = app();
    let b = ball(&mut app, Vec3::ZERO);
    let other = ball(&mut app, Vec3::X * 100.0);
    let bad = [
        Joint3dKind::Revolute { axis: Vec3::ZERO, limits: None, motor: None },
        Joint3dKind::Revolute { axis: Vec3::Y, limits: Some([1.0, -1.0]), motor: None },
        Joint3dKind::Revolute { axis: Vec3::Y, limits: Some([-PI, PI + 0.5]), motor: None },
        Joint3dKind::Revolute { axis: Vec3::Y, limits: None, motor: Some(Motor3d { speed: f32::NAN, max_force: 1.0 }) },
        Joint3dKind::Revolute { axis: Vec3::Y, limits: None, motor: Some(Motor3d { speed: 1.0, max_force: -1.0 }) },
        Joint3dKind::Prismatic { axis: Vec3::ZERO, limits: None, motor: None },
        Joint3dKind::Prismatic { axis: Vec3::X, limits: Some([f32::INFINITY, 1.0]), motor: None },
        Joint3dKind::Rope { length: 0.0 },
        Joint3dKind::Spring { length: 10.0, stiffness: -1.0, damping: 0.0 },
        Joint3dKind::Spring { length: -1.0, stiffness: 1.0, damping: 0.0 },
    ];
    for kind in bad {
        let j = joint(&mut app, Joint3d::new(Some(other), b, kind));
        steps(&mut app, 1);
        assert!(!physics(&app).is_linked(j), "{kind:?} 不连");
        app.world_mut().despawn(j);
    }
    let same = joint(&mut app, Joint3d::new(Some(b), b, Joint3dKind::Ball));
    let nan = joint(&mut app, Joint3d { anchor_a: Vec3::NAN, ..Joint3d::new(Some(other), b, Joint3dKind::Ball) });
    let turned = joint(&mut app, Joint3d { rotation: Quat::from_xyzw(0.0, 0.0, 0.0, 0.0), ..Joint3d::new(Some(other), b, Joint3dKind::Fixed) });
    steps(&mut app, 1);
    assert!(!physics(&app).is_linked(same) && !physics(&app).is_linked(nan) && !physics(&app).is_linked(turned), "两头同一个、锚点不是有限数、朝向是 0 都不连");
    app.world_mut().get_mut::<Joint3d>(nan).unwrap().anchor_a = Vec3::ZERO;
    steps(&mut app, 1);
    assert!(physics(&app).is_linked(nan), "改对了就连上");
}

#[test]
fn joints_follow_their_bodies_and_components() {
    let mut app = app();
    let nothing = app.world_mut().spawn_empty().id();
    let j = joint(&mut app, Joint3d { anchor_a: Vec3::new(0.0, 100.0, 0.0), ..Joint3d::new(None, nothing, Joint3dKind::Ball) });
    steps(&mut app, 1);
    assert!(!physics(&app).is_linked(j), "b 还不是刚体");
    let b = ball(&mut app, Vec3::ZERO);
    app.world_mut().get_mut::<Joint3d>(j).unwrap().b = b;
    steps(&mut app, 1);
    assert!(physics(&app).is_linked(j), "b 成了刚体就连上");
    // 换形状：刚体重建，关节重连
    app.world_mut().entity_mut(b).insert(Collider::ball(20.0));
    steps(&mut app, 1);
    assert!(physics(&app).is_linked(j) && physics(&app).joints() == 1, "刚体重建后重连");
    // 去掉组件就拆
    app.world_mut().entity_mut(j).remove::<Joint3d>();
    steps(&mut app, 1);
    assert_eq!(physics(&app).joints(), 0);
    let j = joint(&mut app, Joint3d::new(None, b, Joint3dKind::Ball));
    steps(&mut app, 1);
    assert_eq!(physics(&app).joints(), 1);
    app.world_mut().despawn(b);
    steps(&mut app, 1);
    assert!(!physics(&app).is_linked(j) && physics(&app).joints() == 0, "刚体删了关节也没了");
    assert_eq!(physics(&app).len().0, 0, "连世界用的固定刚体不算");
}

#[test]
fn linked_bodies_do_not_collide_unless_asked() {
    for collide in [false, true] {
        let mut app = app();
        let a = app.world_mut().spawn((RigidBody::Fixed, Collider::cuboid(Vec3::splat(40.0)), at(0.0, 0.0, 0.0))).id();
        let b = ball(&mut app, Vec3::new(0.0, 60.0, 0.0));
        // 绳子很长：只靠碰撞挡着
        joint(&mut app, Joint3d { collide, ..Joint3d::new(Some(a), b, Joint3dKind::Rope { length: 500.0 }) });
        steps(&mut app, 90);
        let y = pos(&app, b).y;
        assert_eq!(y > 25.0, collide, "collide = {collide}：y = {y}");
    }
}
