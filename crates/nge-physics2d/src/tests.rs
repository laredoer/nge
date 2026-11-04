use bevy_ecs::message::Messages;
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

/// 推进 n 个固定步长。
fn steps(app: &mut App, n: usize) {
    for _ in 0..n {
        app.tick(VIEWPORT, STEP);
    }
}

fn pos(app: &App, e: Entity) -> Vec2 {
    app.world().get::<Transform2d>(e).unwrap().translation
}

/// 一块宽 1000、顶面在 y = 0 的地面。
fn ground(app: &mut App) -> Entity {
    app.world_mut().spawn((RigidBody::Fixed, Collider::rect(Vec2::new(1000.0, 20.0)), at(0.0, -10.0))).id()
}

/// 推进 n 步，收集这期间的碰撞消息（消息只留两帧）。
fn collisions(app: &mut App, n: usize) -> Vec<Collision> {
    let mut all = Vec::new();
    for _ in 0..n {
        app.tick(VIEWPORT, STEP);
        all.extend(app.world_mut().resource_mut::<Messages<Collision>>().drain());
    }
    all
}

#[test]
fn dynamic_bodies_fall_and_fixed_ones_stay() {
    let mut app = app();
    let ball = app.world_mut().spawn((RigidBody::Dynamic, Collider::circle(10.0), at(0.0, 100.0), Velocity::default())).id();
    let wall = app.world_mut().spawn((RigidBody::Fixed, Collider::rect(Vec2::splat(10.0)), at(50.0, 100.0))).id();
    steps(&mut app, 30);
    assert!(pos(&app, ball).y < 100.0 - 50.0, "半秒掉了 ½·981·0.25 ≈ 120: {}", pos(&app, ball));
    assert!(app.world().get::<Velocity>(ball).unwrap().linear.y < -400.0, "速度写回");
    assert_eq!(pos(&app, wall), Vec2::new(50.0, 100.0));
}

#[test]
fn boxes_land_on_the_ground() {
    let mut app = app();
    ground(&mut app);
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider::rect(Vec2::splat(20.0)), at(0.0, 50.0))).id();
    steps(&mut app, 180);
    let y = pos(&app, b).y;
    assert!((y - 10.0).abs() < 1.0, "停在地面上: {y}");
}

#[test]
fn collision_messages_only_for_colliders_with_events() {
    let mut app = app();
    let g = ground(&mut app);
    let quiet = app.world_mut().spawn((RigidBody::Dynamic, Collider::rect(Vec2::splat(20.0)), at(-100.0, 15.0))).id();
    assert!(collisions(&mut app, 30).is_empty());
    let loud = app.world_mut().spawn((RigidBody::Dynamic, Collider { events: true, ..Collider::circle(10.0) }, at(100.0, 15.0))).id();
    let started: Vec<_> = collisions(&mut app, 30).into_iter().filter(|c| c.started).collect();
    assert_eq!(started.len(), 1, "{started:?}");
    let pair = [started[0].a, started[0].b];
    assert!(pair.contains(&g) && pair.contains(&loud) && !pair.contains(&quiet));

    app.world_mut().get_mut::<Transform2d>(loud).unwrap().translation.y = 500.0;
    assert!(collisions(&mut app, 1).iter().any(|c| !c.started), "挪开后发停止接触");
}

#[test]
fn game_can_teleport_set_velocity_and_push() {
    let mut app = app();
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider::circle(10.0), at(0.0, 0.0), Velocity::default(), Impulse::default())).id();
    app.world_mut().resource_mut::<Physics>().set_gravity(Vec2::ZERO);
    steps(&mut app, 1);

    app.world_mut().get_mut::<Transform2d>(b).unwrap().translation = Vec2::new(300.0, 0.0);
    steps(&mut app, 1);
    assert!((pos(&app, b) - Vec2::new(300.0, 0.0)).length() < 1e-3, "瞬移: {}", pos(&app, b));

    app.world_mut().get_mut::<Velocity>(b).unwrap().linear = Vec2::new(60.0, 0.0);
    steps(&mut app, 60);
    assert!((pos(&app, b).x - 360.0).abs() < 1.0, "一秒走 60: {}", pos(&app, b));

    let mass = app.world().resource::<Physics>().mass(b).unwrap();
    app.world_mut().get_mut::<Impulse>(b).unwrap().0 = Vec2::new(0.0, 100.0 * mass);
    steps(&mut app, 1);
    let v = app.world().get::<Velocity>(b).unwrap().linear;
    assert!((v - Vec2::new(60.0, 100.0)).length() < 1e-2, "冲量 = 质量 × 速度变化: {v}");
    assert_eq!(app.world().get::<Impulse>(b).unwrap().0, Vec2::ZERO, "用完清零");
}

#[test]
fn kinematic_bodies_follow_the_transform_and_push_others() {
    let mut app = app();
    app.world_mut().resource_mut::<Physics>().set_gravity(Vec2::ZERO);
    let paddle = app.world_mut().spawn((RigidBody::Kinematic, Collider::rect(Vec2::splat(20.0)), at(0.0, 0.0))).id();
    let ball = app.world_mut().spawn((RigidBody::Dynamic, Collider::circle(10.0), at(40.0, 0.0))).id();
    steps(&mut app, 1);
    for i in 1..=30 {
        app.world_mut().get_mut::<Transform2d>(paddle).unwrap().translation.x = i as f32 * 3.0;
        steps(&mut app, 1);
    }
    assert!((pos(&app, paddle).x - 90.0).abs() < 1e-3);
    assert!(pos(&app, ball).x > 100.0, "被推开: {}", pos(&app, ball));
}

#[test]
fn bodies_follow_component_changes_and_despawns() {
    let mut app = app();
    let e = app.world_mut().spawn((RigidBody::Dynamic, Collider::circle(10.0), at(0.0, 100.0))).id();
    steps(&mut app, 1);
    assert_eq!(app.world().resource::<Physics>().len(), (1, 1));

    // 同样的值重新插入不重建（位置不会被拉回）
    let y = pos(&app, e).y;
    app.world_mut().entity_mut(e).insert((RigidBody::Dynamic, Collider::circle(10.0)));
    steps(&mut app, 1);
    assert!(pos(&app, e).y < y);

    app.world_mut().entity_mut(e).insert(RigidBody::Fixed);
    steps(&mut app, 2);
    let y = pos(&app, e).y;
    steps(&mut app, 5);
    assert_eq!(pos(&app, e).y, y, "换成固定刚体");

    app.world_mut().entity_mut(e).remove::<RigidBody>();
    steps(&mut app, 1);
    assert_eq!(app.world().resource::<Physics>().len(), (0, 1), "只剩碰撞体");

    app.world_mut().despawn(e);
    steps(&mut app, 1);
    assert_eq!(app.world().resource::<Physics>().len(), (0, 0));
}

#[test]
fn fixed_step_accumulates_and_caps() {
    let mut app = app();
    let e = app.world_mut().spawn((RigidBody::Dynamic, at(0.0, 0.0), Velocity { linear: Vec2::new(60.0, 0.0), angular: 0.0 })).id();
    app.world_mut().resource_mut::<Physics>().set_gravity(Vec2::ZERO);
    app.tick(VIEWPORT, STEP / 2.0);
    assert_eq!(pos(&app, e).x, 0.0, "不够一步不动");
    app.tick(VIEWPORT, STEP / 2.0);
    assert!((pos(&app, e).x - 1.0).abs() < 1e-3, "{}", pos(&app, e));
    app.tick(VIEWPORT, 0.04);
    assert!((pos(&app, e).x - 3.0).abs() < 1e-3, "40ms 是 2 步多一点: {}", pos(&app, e));
    // 没有碰撞体的刚体照样能动
    assert_eq!(app.world().resource::<Physics>().len(), (1, 0));
}

/// `cargo test -p nge-physics2d --release -- --ignored --nocapture`
#[test]
#[ignore]
fn thousand_bodies_step_time() {
    let mut app = app();
    ground(&mut app);
    for i in 0..1000 {
        let (x, y) = ((i % 40) as f32 * 22.0 - 440.0, 20.0 + (i / 40) as f32 * 22.0);
        app.world_mut().spawn((RigidBody::Dynamic, Collider::circle(10.0), at(x, y), Velocity::default()));
    }
    // 从生成就开始计：这段时间球都在下落、互相挤，没有休眠
    let start = std::time::Instant::now();
    let mut worst = std::time::Duration::ZERO;
    for _ in 0..120 {
        let t = std::time::Instant::now();
        app.tick(VIEWPORT, STEP);
        worst = worst.max(t.elapsed());
    }
    let avg = start.elapsed() / 120;
    println!("1000 个刚体：平均 {avg:?}，最长 {worst:?}");
    assert!(avg < std::time::Duration::from_millis(2), "{avg:?}");
}

#[test]
fn dynamic_bodies_without_mass_still_fall() {
    let mut app = app();
    let bare = app.world_mut().spawn((RigidBody::Dynamic, at(0.0, 100.0))).id();
    let empty = app.world_mut().spawn((RigidBody::Dynamic, Collider { density: 0.0, ..Collider::rect(Vec2::ZERO) }, at(50.0, 100.0))).id();
    steps(&mut app, 30);
    assert!(pos(&app, bare).y < 50.0 && pos(&app, empty).y < 50.0, "{} {}", pos(&app, bare), pos(&app, empty));
    assert!(app.world().resource::<Physics>().mass(bare).unwrap() > 0.0);
}

#[test]
fn removing_or_rebuilding_a_touching_collider_reports_stopped() {
    let mut app = app();
    let g = ground(&mut app);
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider { events: true, ..Collider::circle(10.0) }, at(0.0, 10.0))).id();
    assert!(collisions(&mut app, 10).iter().any(|c| c.started));

    app.world_mut().entity_mut(b).insert(Collider { events: true, restitution: 0.5, ..Collider::circle(10.0) });
    let rebuilt: Vec<_> = collisions(&mut app, 5).iter().map(|c| c.started).collect();
    assert_eq!(rebuilt, [false, true], "重建：先停止再开始");

    app.world_mut().despawn(b);
    let gone = collisions(&mut app, 2);
    assert_eq!(gone.len(), 1);
    assert!(!gone[0].started && [gone[0].a, gone[0].b].contains(&g) && [gone[0].a, gone[0].b].contains(&b), "删除时也发停止接触");
}

#[test]
fn kinematic_moves_evenly_across_substeps() {
    let mut app = app();
    let k = app.world_mut().spawn((RigidBody::Kinematic, at(0.0, 0.0), Velocity::default())).id();
    steps(&mut app, 1);
    app.world_mut().get_mut::<Transform2d>(k).unwrap().translation.x = 60.0;
    app.tick(VIEWPORT, STEP * 2.0 + 1e-4);
    assert!((pos(&app, k).x - 60.0).abs() < 1e-3);
    let v = app.world().get::<Velocity>(k).unwrap().linear.x;
    assert!((v - 60.0 / (2.0 * STEP)).abs() < 1.0, "两步平分位移: {v}");
}

#[test]
fn rotation_is_not_wrapped_on_write_back() {
    let mut app = app();
    let e = app.world_mut().spawn((RigidBody::Fixed, Collider::circle(1.0), Transform2d { rotation: 4.0, ..at(0.0, 0.0) })).id();
    steps(&mut app, 2);
    assert!((app.world().get::<Transform2d>(e).unwrap().rotation - 4.0).abs() < 1e-4);
}

#[test]
fn bad_frame_times_do_not_freeze_the_simulation() {
    let mut app = app();
    let e = app.world_mut().spawn((RigidBody::Dynamic, at(0.0, 0.0))).id();
    app.world_mut().resource_mut::<nge_ecs::Time>().delta = f32::NAN;
    app.world_mut().run_schedule(nge_ecs::PostUpdate);
    steps(&mut app, 5);
    assert!(pos(&app, e).y < 0.0);
}

#[test]
fn contacts_report_the_other_side_and_its_normal() {
    let mut app = app();
    let g = ground(&mut app);
    let wall = app.world_mut().spawn((Collider::rect(Vec2::new(20.0, 200.0)), at(-20.0, 100.0))).id();
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider::rect(Vec2::splat(20.0)), at(0.0, 30.0))).id();
    assert!(app.world().resource::<Physics>().contacts(b).is_empty(), "还没建到物理里");
    steps(&mut app, 60);
    let contacts = app.world().resource::<Physics>().contacts(b);
    let on = |e: Entity| contacts.iter().find(|c| c.other == e).map(|c| c.normal);
    let up = on(g).expect("站在地上");
    assert!((up - Vec2::Y).length() < 1e-3, "法线从对方指向自己：地面朝上 {up}");
    let side = on(wall).expect("贴着墙");
    assert!((side - Vec2::X).length() < 1e-3, "墙在左边，法线朝右 {side}");
    let theirs = app.world().resource::<Physics>().contacts(g);
    assert!(theirs.iter().any(|c| c.other == b && (c.normal + Vec2::Y).length() < 1e-3), "从地面看，法线朝下");
    let air = app.world_mut().spawn((RigidBody::Dynamic, Collider::circle(5.0), at(300.0, 300.0))).id();
    steps(&mut app, 1);
    assert!(app.world().resource::<Physics>().contacts(air).is_empty());
}

#[test]
fn locked_rotation_keeps_a_box_upright() {
    let mut app = app();
    ground(&mut app);
    // 偏心落在台阶边上的箱子会翻；锁住旋转的不翻
    app.world_mut().spawn((RigidBody::Fixed, Collider::rect(Vec2::splat(20.0)), at(0.0, 10.0)));
    let free = app.world_mut().spawn((RigidBody::Dynamic, Collider::rect(Vec2::splat(20.0)), at(14.0, 40.0))).id();
    let locked = app.world_mut().spawn((RigidBody::Dynamic, LockRotation, Collider::rect(Vec2::splat(20.0)), at(-14.0, 40.0))).id();
    steps(&mut app, 120);
    let rot = |app: &App, e: Entity| app.world().get::<Transform2d>(e).unwrap().rotation;
    assert!(rot(&app, free).abs() > 0.3, "没锁的翻了 {}", rot(&app, free));
    assert!(rot(&app, locked).abs() < 1e-3, "锁住的不转 {}", rot(&app, locked));
    // 去掉锁，再放回台阶边上（不给角速度）：这回碰撞会让它翻
    app.world_mut().entity_mut(locked).remove::<LockRotation>().insert(at(-14.0, 40.0));
    steps(&mut app, 120);
    assert!(rot(&app, locked).abs() > 0.3, "去掉锁以后会翻 {}", rot(&app, locked));
}

#[test]
fn locking_stops_existing_spin() {
    let mut app = app();
    let b = app.world_mut().spawn((RigidBody::Dynamic, LockRotation, Collider::rect(Vec2::splat(10.0)), at(0.0, 0.0), Velocity { linear: Vec2::ZERO, angular: 5.0 })).id();
    steps(&mut app, 30);
    assert!(app.world().get::<Transform2d>(b).unwrap().rotation.abs() < 1e-3, "锁住时原来的角速度也停");
    let only_lock = app.world_mut().spawn(LockRotation).id();
    steps(&mut app, 1);
    let before = app.world().resource::<Physics>().contacts(only_lock).len();
    app.world_mut().despawn(only_lock);
    steps(&mut app, 1);
    assert_eq!(before, 0);
}

#[test]
fn sensors_are_not_contacts() {
    let mut app = app();
    ground(&mut app);
    let zone = app.world_mut().spawn((Collider { sensor: true, ..Collider::rect(Vec2::splat(40.0)) }, at(0.0, 10.0))).id();
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider::rect(Vec2::splat(20.0)), at(0.0, 30.0))).id();
    steps(&mut app, 60);
    let contacts = app.world().resource::<Physics>().contacts(b);
    assert!(!contacts.is_empty() && contacts.iter().all(|c| c.other != zone), "只算地面，不算 sensor {contacts:?}");
}
