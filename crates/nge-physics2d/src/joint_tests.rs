use glam::Vec2;
use nge_core::Viewport;
use nge_ecs::App;
use nge_render2d::Transform2d;

use super::*;

const VIEWPORT: Viewport = Viewport { size: [100.0, 100.0], pixels: [100, 100] };

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(PhysicsPlugin::default());
    app
}

fn at(x: f32, y: f32) -> Transform2d {
    Transform2d { translation: Vec2::new(x, y), ..Default::default() }
}

fn steps(app: &mut App, n: usize) {
    for _ in 0..n {
        app.tick(VIEWPORT, STEP);
    }
}

fn pos(app: &App, e: Entity) -> Vec2 {
    app.world().get::<Transform2d>(e).unwrap().translation
}

fn angle(app: &App, e: Entity) -> f32 {
    app.world().get::<Transform2d>(e).unwrap().rotation
}

fn ball(app: &mut App, x: f32, y: f32) -> Entity {
    app.world_mut().spawn((RigidBody::Dynamic, Collider::circle(5.0), at(x, y), Velocity::default())).id()
}

/// 转动关节：b 绕世界上的 `pivot` 转（b 没转过时）。
fn pin(app: &mut App, b: Entity, pivot: Vec2) -> Entity {
    let mut joint = Joint::new(None, b, JointKind::revolute());
    joint.anchor_a = pivot;
    joint.anchor_b = pivot - pos(app, b);
    app.world_mut().spawn(joint).id()
}

fn joints(app: &App) -> usize {
    app.world().resource::<Physics>().joints()
}

/// 连在世界上 (0, 0) 的单摆：一直离支点 100，会摆下来。
#[test]
fn revolute_to_the_world_swings_at_a_fixed_distance() {
    let mut app = app();
    let b = ball(&mut app, 100.0, 0.0);
    pin(&mut app, b, Vec2::ZERO);
    // 摆长 1 米，四分之一周期约 0.5 秒：30 步后在最低点附近
    steps(&mut app, 30);
    let p = pos(&app, b);
    assert!((p.length() - 100.0).abs() < 1.0, "绳长不变 {p}");
    assert!(p.y < -90.0, "摆下来了 {p}");
    assert_eq!(joints(&app), 1);
}

/// 支点在刚体上的偏移：锚点按刚体自己的坐标算。
#[test]
fn anchors_are_local_to_each_body() {
    let mut app = app();
    let pivot = app.world_mut().spawn((RigidBody::Fixed, at(50.0, 50.0))).id();
    let b = ball(&mut app, 50.0, 0.0);
    let mut joint = Joint::new(Some(pivot), b, JointKind::revolute());
    joint.anchor_b = Vec2::new(0.0, 50.0);
    app.world_mut().spawn(joint);
    steps(&mut app, 60);
    assert!((pos(&app, b) - Vec2::new(50.0, 0.0)).length() < 1.0, "支点正好在球上方 50，球挂着不动 {}", pos(&app, b));
}

#[test]
fn revolute_limits_stop_the_swing() {
    let mut app = app();
    let b = ball(&mut app, 100.0, 0.0);
    // 球绕支点转；b 的角度相对 a（世界）不超过 ±10°
    let mut joint = Joint::new(None, b, JointKind::Revolute { limits: Some([-10f32.to_radians(), 10f32.to_radians()]), motor: None });
    joint.anchor_b = Vec2::new(-100.0, 0.0);
    app.world_mut().spawn(joint);
    steps(&mut app, 60);
    let p = pos(&app, b);
    assert!(p.y > -100.0 * 15f32.to_radians().sin(), "最多往下摆 10° 多一点 {p}");
    assert!(angle(&app, b) > -15f32.to_radians(), "{}", angle(&app, b));
}

#[test]
fn revolute_motor_spins_a_wheel() {
    let mut app = app();
    let wheel = ball(&mut app, 0.0, 0.0);
    let motor = Motor { speed: std::f32::consts::PI, max_force: f32::INFINITY };
    app.world_mut().spawn(Joint::new(None, wheel, JointKind::Revolute { limits: None, motor: Some(motor) }));
    steps(&mut app, 30);
    let w = app.world().get::<Velocity>(wheel).unwrap().angular;
    assert!((w - std::f32::consts::PI).abs() < 0.1, "转速跟上马达 {w}");
    assert!(pos(&app, wheel).length() < 0.5, "轴不动");
}

#[test]
fn fixed_joint_welds_two_bodies() {
    let mut app = app();
    let a = app.world_mut().spawn((RigidBody::Dynamic, Collider::rect(Vec2::splat(10.0)), at(0.0, 0.0), Velocity { linear: Vec2::new(100.0, 0.0), angular: 0.0 })).id();
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider::rect(Vec2::splat(10.0)), at(20.0, 0.0), Velocity::default())).id();
    let mut joint = Joint::new(Some(a), b, JointKind::Fixed);
    joint.anchor_a = Vec2::new(20.0, 0.0);
    app.world_mut().spawn(joint);
    steps(&mut app, 30);
    let d = pos(&app, b) - pos(&app, a);
    assert!((d - Vec2::new(20.0, 0.0)).length() < 1.0, "一起走，间距不变 {d}");
    assert!(pos(&app, b).x > 20.0 + 10.0, "b 被 a 带着走 {}", pos(&app, b));
}

/// `angle`：固定关节让 b 相对 a 保持这个角度。
#[test]
fn fixed_joint_keeps_the_declared_angle() {
    let mut app = app();
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider::rect(Vec2::new(20.0, 4.0)), at(0.0, 0.0), Velocity::default())).id();
    let mut joint = Joint::new(None, b, JointKind::Fixed);
    joint.angle = std::f32::consts::FRAC_PI_2;
    app.world_mut().spawn(joint);
    steps(&mut app, 60);
    assert!((angle(&app, b) - std::f32::consts::FRAC_PI_2).abs() < 0.02, "{}", angle(&app, b));
    assert!(pos(&app, b).length() < 0.5);
}

#[test]
fn rope_only_limits_the_maximum_distance() {
    let mut app = app();
    let b = ball(&mut app, 0.0, -30.0);
    app.world_mut().spawn(Joint::new(None, b, JointKind::Rope { length: 50.0 }));
    steps(&mut app, 5);
    assert!(pos(&app, b).y < -30.5, "绳子松着时自由下落 {}", pos(&app, b));
    steps(&mut app, 60);
    assert!((pos(&app, b).y + 50.0).abs() < 1.0, "拉直以后挂在 50 {}", pos(&app, b));
}

#[test]
fn spring_settles_below_rest_length() {
    let mut app = app();
    let b = ball(&mut app, 0.0, -50.0);
    app.world_mut().spawn(Joint::new(None, b, JointKind::Spring { length: 50.0, stiffness: 100.0, damping: 10.0 }));
    steps(&mut app, 300);
    let y = pos(&app, b).y;
    // 加速度型弹簧：k·Δx = g → Δx = 981 / 100 ≈ 9.8
    assert!((y + 50.0 + 9.81).abs() < 1.5, "停在静止长度往下 g/k {y}");
}

#[test]
fn prismatic_slides_along_its_axis_within_limits() {
    let mut app = app();
    let b = ball(&mut app, 0.0, 0.0);
    app.world_mut().spawn(Joint::new(None, b, JointKind::Prismatic { axis: Vec2::X, limits: Some([-10.0, 10.0]), motor: None }));
    app.world_mut().get_mut::<Velocity>(b).unwrap().linear = Vec2::new(300.0, 0.0);
    steps(&mut app, 30);
    let p = pos(&app, b);
    assert!(p.y.abs() < 0.5, "不往下掉 {p}");
    assert!((p.x - 10.0).abs() < 1.0, "滑到上限停住 {p}");
}

#[test]
fn prismatic_motor_drives_the_body() {
    let mut app = app();
    let b = ball(&mut app, 0.0, 0.0);
    let motor = Motor { speed: 50.0, max_force: f32::INFINITY };
    app.world_mut().spawn(Joint::new(None, b, JointKind::Prismatic { axis: Vec2::new(0.0, 2.0), limits: None, motor: Some(motor) }));
    steps(&mut app, 60);
    let v = app.world().get::<Velocity>(b).unwrap().linear;
    assert!((v.y - 50.0).abs() < 2.0 && v.x.abs() < 0.5, "轴会归一化，沿 y 以 50 走 {v}");
}

#[test]
fn removing_the_joint_lets_go() {
    let mut app = app();
    let b = ball(&mut app, 0.0, -50.0);
    let joint = pin(&mut app, b, Vec2::ZERO);
    steps(&mut app, 10);
    assert!((pos(&app, b).y + 50.0).abs() < 1.0);
    app.world_mut().entity_mut(joint).remove::<Joint>();
    steps(&mut app, 10);
    assert!(pos(&app, b).y < -55.0, "去掉以后掉下去 {}", pos(&app, b));
    assert_eq!(joints(&app), 0);
    assert_eq!(app.world().resource::<Physics>().len(), (1, 1), "连世界用的固定刚体不算");

    let joint = app.world_mut().spawn(Joint::new(None, b, JointKind::Rope { length: 1000.0 })).id();
    steps(&mut app, 1);
    assert_eq!(joints(&app), 1);
    app.world_mut().despawn(joint);
    steps(&mut app, 1);
    assert_eq!(joints(&app), 0, "关节实体删掉也拆掉");
}

#[test]
fn joint_changes_rebuild_it() {
    let mut app = app();
    let b = ball(&mut app, 0.0, -30.0);
    let joint = pin(&mut app, b, Vec2::ZERO);
    steps(&mut app, 10);
    app.world_mut().get_mut::<Joint>(joint).unwrap().kind = JointKind::Rope { length: 100.0 };
    steps(&mut app, 20);
    assert!(pos(&app, b).y < -40.0, "换成长绳以后能往下掉 {}", pos(&app, b));
    assert_eq!(joints(&app), 1);
}

/// 关节先于刚体存在、刚体删掉、刚体重建（换了形状或种类）：都跟着处理。
#[test]
fn joints_follow_their_bodies() {
    let mut app = app();
    let b = app.world_mut().spawn((at(0.0, -50.0), Velocity::default())).id();
    pin(&mut app, b, Vec2::ZERO);
    steps(&mut app, 2);
    assert_eq!(joints(&app), 0, "b 还不是刚体");
    app.world_mut().entity_mut(b).insert((RigidBody::Dynamic, Collider::circle(5.0)));
    steps(&mut app, 10);
    assert_eq!(joints(&app), 1, "b 成了刚体就连上");
    assert!((pos(&app, b).y + 50.0).abs() < 1.0);

    app.world_mut().entity_mut(b).insert(Collider::rect(Vec2::splat(10.0)));
    steps(&mut app, 30);
    assert_eq!(joints(&app), 1, "刚体重建以后重新连上");
    assert!((pos(&app, b).y + 50.0).abs() < 1.0, "还挂着 {}", pos(&app, b));

    app.world_mut().despawn(b);
    steps(&mut app, 2);
    assert_eq!(joints(&app), 0, "刚体没了，关节也没了");
}

#[test]
fn bad_joints_are_skipped() {
    let mut app = app();
    let b = ball(&mut app, 0.0, -50.0);
    app.world_mut().spawn(Joint::new(Some(b), b, JointKind::revolute()));
    app.world_mut().spawn(Joint::new(None, b, JointKind::Rope { length: f32::NAN }));
    app.world_mut().spawn(Joint::new(None, b, JointKind::Rope { length: -1.0 }));
    app.world_mut().spawn(Joint::new(None, b, JointKind::Rope { length: 0.0 }));
    app.world_mut().spawn(Joint::new(None, b, JointKind::Prismatic { axis: Vec2::ZERO, limits: None, motor: None }));
    let mut far = Joint::new(None, b, JointKind::revolute());
    far.anchor_a = Vec2::new(f32::INFINITY, 0.0);
    app.world_mut().spawn(far);
    let motor = |speed: f32, max_force: f32| JointKind::Revolute { limits: None, motor: Some(Motor { speed, max_force }) };
    app.world_mut().spawn(Joint::new(None, b, motor(f32::NAN, 1.0)));
    app.world_mut().spawn(Joint::new(None, b, motor(1.0, -1.0)));
    app.world_mut().spawn(Joint::new(None, b, JointKind::Revolute { limits: Some([1.0, -1.0]), motor: None }));
    app.world_mut().spawn(Joint::new(None, b, JointKind::Revolute { limits: Some([-4.0, 4.0]), motor: None }));
    app.world_mut().spawn(Joint::new(None, b, JointKind::Prismatic { axis: Vec2::X, limits: Some([5.0, 1.0]), motor: None }));
    for (length, stiffness, damping) in [(-1.0, 1.0, 1.0), (1.0, -1.0, 1.0), (1.0, 1.0, -1.0)] {
        app.world_mut().spawn(Joint::new(None, b, JointKind::Spring { length, stiffness, damping }));
    }
    steps(&mut app, 2);
    assert_eq!(joints(&app), 0);
}

/// 坏参数的关节改对了就连上。
#[test]
fn a_fixed_bad_joint_links() {
    let mut app = app();
    let b = ball(&mut app, 0.0, -50.0);
    let joint = app.world_mut().spawn(Joint::new(None, b, JointKind::Rope { length: 0.0 })).id();
    steps(&mut app, 2);
    assert!(!app.world().resource::<Physics>().is_linked(joint));
    app.world_mut().get_mut::<Joint>(joint).unwrap().kind = JointKind::Rope { length: 60.0 };
    steps(&mut app, 1);
    assert!(app.world().resource::<Physics>().is_linked(joint));
    assert_eq!(joints(&app), 1);
}

/// 只改马达速度：关节原地换参数，轮子跟着换转速。
#[test]
fn changing_the_motor_updates_in_place() {
    let mut app = app();
    let wheel = ball(&mut app, 0.0, 0.0);
    let spin = |speed: f32| JointKind::Revolute { limits: None, motor: Some(Motor { speed, max_force: f32::INFINITY }) };
    let joint = app.world_mut().spawn(Joint::new(None, wheel, spin(3.0))).id();
    steps(&mut app, 20);
    app.world_mut().get_mut::<Joint>(joint).unwrap().kind = spin(-3.0);
    steps(&mut app, 20);
    let w = app.world().get::<Velocity>(wheel).unwrap().angular;
    assert!((w + 3.0).abs() < 0.1, "{w}");
    assert_eq!(joints(&app), 1);
}

/// 转动关节的范围从 `angle` 算起：b 声明转 90°、范围 ±10°，往下摆停在 80°。
#[test]
fn revolute_limits_count_from_the_declared_angle() {
    let mut app = app();
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider::rect(Vec2::new(4.0, 40.0)), at(0.0, 0.0), Velocity::default())).id();
    app.world_mut().get_mut::<Transform2d>(b).unwrap().rotation = std::f32::consts::FRAC_PI_2;
    let mut joint = Joint::new(None, b, JointKind::Revolute { limits: Some([-10f32.to_radians(), 10f32.to_radians()]), motor: None });
    // 支点在 b 的一头（b 转了 90°，本地的 +y 朝世界的 -x）
    joint.anchor_a = Vec2::new(-20.0, 0.0);
    joint.anchor_b = Vec2::new(0.0, 20.0);
    joint.angle = std::f32::consts::FRAC_PI_2;
    app.world_mut().spawn(joint);
    steps(&mut app, 60);
    assert!((angle(&app, b) - 80f32.to_radians()).abs() < 2f32.to_radians(), "{}", angle(&app, b).to_degrees());
}

/// 挂在转过的 a 上的滑动关节：轴按 a 的坐标转。
#[test]
fn prismatic_axis_turns_with_a() {
    let mut app = app();
    let a = app.world_mut().spawn((RigidBody::Fixed, Transform2d { rotation: std::f32::consts::FRAC_PI_2, ..at(0.0, 0.0) })).id();
    let b = ball(&mut app, 0.0, 0.0);
    let mut joint = Joint::new(Some(a), b, JointKind::Prismatic { axis: Vec2::X, limits: Some([-30.0, 30.0]), motor: None });
    joint.angle = -std::f32::consts::FRAC_PI_2;
    app.world_mut().spawn(joint);
    steps(&mut app, 60);
    let p = pos(&app, b);
    assert!(p.x.abs() < 0.5 && (p.y + 30.0).abs() < 1.0, "沿世界的 y 往下滑到 -30 {p}");
    assert!(angle(&app, b).abs() < 0.02);
}

/// 两个连着的刚体默认不互相碰撞；`collide` 打开以后会碰。
#[test]
fn connected_bodies_do_not_collide_unless_asked() {
    let run = |collide: bool| {
        let mut app = app();
        app.world_mut().resource_mut::<Physics>().set_gravity(Vec2::ZERO);
        let a = app.world_mut().spawn((RigidBody::Fixed, Collider::rect(Vec2::splat(20.0)), at(0.0, 0.0))).id();
        let b = ball(&mut app, 30.0, 0.0);
        app.world_mut().get_mut::<Velocity>(b).unwrap().linear = Vec2::new(-300.0, 0.0);
        let mut joint = Joint::new(Some(a), b, JointKind::Rope { length: 100.0 });
        joint.collide = collide;
        app.world_mut().spawn(joint);
        steps(&mut app, 20);
        pos(&app, b).x
    };
    assert!(run(false) < -10.0, "穿过去 {}", run(false));
    assert!(run(true) > 10.0, "被挡住 {}", run(true));
}


/// 滑动关节的 `angle` 只管 b 转多少，滑的方向还是 `axis`（a 的坐标里）。
#[test]
fn prismatic_angle_turns_the_body_not_the_axis() {
    let mut app = app();
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider::rect(Vec2::new(20.0, 4.0)), at(0.0, 0.0), Velocity::default())).id();
    let mut joint = Joint::new(None, b, JointKind::Prismatic { axis: Vec2::X, limits: None, motor: Some(Motor { speed: 50.0, max_force: f32::INFINITY }) });
    joint.angle = std::f32::consts::FRAC_PI_2;
    app.world_mut().spawn(joint);
    steps(&mut app, 60);
    let p = pos(&app, b);
    assert!((angle(&app, b) - std::f32::consts::FRAC_PI_2).abs() < 0.02, "{}", angle(&app, b));
    assert!(p.x > 30.0 && p.y.abs() < 0.5, "沿 x 滑 {p}");
}

#[test]
fn debug_view_draws_linked_joints() {
    use crate::debug::JOINT;

    let mut app = app();
    app.world_mut().init_resource::<nge_render2d::SpriteInstances>();
    app.world_mut().resource_mut::<PhysicsDebug>().enabled = true;
    app.world_mut().resource_mut::<Physics>().set_gravity(Vec2::ZERO);
    let b = ball(&mut app, 0.0, -50.0);
    app.world_mut().spawn(Joint::new(None, b, JointKind::Rope { length: 60.0 }));
    let loose = app.world_mut().spawn(at(100.0, 0.0)).id();
    app.world_mut().spawn(Joint::new(None, loose, JointKind::revolute()));
    steps(&mut app, 1);
    let drawn: Vec<_> = app.world().resource::<nge_render2d::SpriteInstances>().iter().filter(|(_, i)| i.color == JOINT).map(|&(_, i)| i).collect();
    // 一条线 + 两个 32 段的圆；没连上的（loose 不是刚体）不画
    assert_eq!(drawn.len(), 1 + 2 * 32);
    let line = drawn.iter().max_by(|x, y| x.size.x.total_cmp(&y.size.x)).unwrap();
    assert!((line.position - Vec2::new(0.0, -25.0)).length() < 0.5 && (line.size.x - 50.0).abs() < 0.5, "从支点连到球心 {line:?}");
}
