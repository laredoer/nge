use bevy_ecs::message::Messages;
use glam::Vec2;
use nge_core::Viewport;
use nge_ecs::App;
use nge_render2d::{DrawAt, Transform2d};

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

/// 画在哪（插值过的位置）。
fn drawn(app: &App, e: Entity) -> Vec2 {
    app.world().get::<DrawAt>(e).expect("有刚体的实体每帧都有 DrawAt").translation
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
    assert!((pos(&app, b).x - 360.0).abs() < 1e-2, "一秒走 60: {}", pos(&app, b));
    assert!((drawn(&app, b).x - 359.0).abs() < 1e-2, "画的位置比物理慢一步: {}", drawn(&app, b));

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

/// 没有重力、以 60 point / s 往右走的动态刚体。
fn mover(app: &mut App) -> Entity {
    app.world_mut().resource_mut::<Physics>().set_gravity(Vec2::ZERO);
    app.world_mut().spawn((RigidBody::Dynamic, at(0.0, 0.0), Velocity { linear: Vec2::new(60.0, 0.0), angular: 0.0 })).id()
}

#[test]
fn fixed_step_accumulates_and_caps() {
    let mut app = app();
    let e = mover(&mut app);
    app.tick(VIEWPORT, STEP);
    app.tick(VIEWPORT, STEP);
    assert!((pos(&app, e).x - 2.0).abs() < 1e-3, "Transform2d 是物理的真实位置: {}", pos(&app, e));
    // 画的位置在上一步和这一步之间插值，比物理慢一步
    assert!((drawn(&app, e).x - 1.0).abs() < 1e-3, "两步后画在第一步: {}", drawn(&app, e));
    app.tick(VIEWPORT, STEP / 2.0);
    assert_eq!(pos(&app, e).x, 2.0, "不够一步不动");
    assert!((drawn(&app, e).x - 1.5).abs() < 1e-3, "不够一步时画的位置插值: {}", drawn(&app, e));
    app.tick(VIEWPORT, 1.0);
    assert!((pos(&app, e).x - (2.0 + MAX_STEPS as f32)).abs() < 1e-3, "卡顿时最多补 MAX_STEPS 步: {}", pos(&app, e));
    // 没有碰撞体的刚体照样能动
    assert_eq!(app.world().resource::<Physics>().len(), (1, 0));
}

/// 每帧走 `dt`，返回后 `n` 帧每帧的位移。
fn frame_moves(dt: impl Fn(usize) -> f32, n: usize) -> Vec<(f32, f32)> {
    let mut app = app();
    let e = mover(&mut app);
    for i in 0..10 {
        app.tick(VIEWPORT, dt(i));
    }
    (10..10 + n)
        .map(|i| {
            let before = drawn(&app, e).x;
            app.tick(VIEWPORT, dt(i));
            (drawn(&app, e).x - before, 60.0 * dt(i))
        })
        .collect()
}

#[test]
fn moves_every_frame_at_120hz() {
    for (moved, expected) in frame_moves(|_| STEP / 2.0, 20) {
        assert!((moved - expected).abs() < 1e-3, "每帧都走半步，不是一帧一步一帧不动: {moved}");
    }
}

#[test]
fn jittery_frame_times_still_move_evenly() {
    // 帧时间在一步上下抖：不插值时会一帧走 0 步、下一帧走 2 步
    for (moved, expected) in frame_moves(|i| if i % 2 == 0 { STEP * 0.97 } else { STEP * 1.03 }, 20) {
        assert!((moved - expected).abs() < 1e-3, "位移跟着帧时间走: {moved} / {expected}");
    }
}

#[test]
fn teleport_is_not_interpolated() {
    let mut app = app();
    let e = mover(&mut app);
    steps(&mut app, 5);
    app.world_mut().get_mut::<Transform2d>(e).unwrap().translation.x = 500.0;
    app.tick(VIEWPORT, STEP / 2.0);
    assert_eq!(pos(&app, e).x, 500.0, "不够一步的帧里游戏的改动不被覆盖");
    app.tick(VIEWPORT, STEP);
    let x = drawn(&app, e).x;
    assert!((500.0..=501.5).contains(&x), "画面从新位置接着走，不从旧位置滑过来: {x}");
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

/// 两个一样的 600 point / s 动态刚体，`act` 只对第一个做；之后两者的位置应该一样。
fn same_path_as_untouched(act: impl Fn(&mut App, Entity)) -> (f32, f32) {
    let mut app = app();
    app.world_mut().resource_mut::<Physics>().set_gravity(Vec2::ZERO);
    let spawn = |app: &mut App, y: f32| {
        app.world_mut().spawn((RigidBody::Dynamic, Collider::circle(5.0), at(0.0, y), Velocity { linear: Vec2::new(600.0, 0.0), angular: 0.0 })).id()
    };
    let (a, b) = (spawn(&mut app, 0.0), spawn(&mut app, 100.0));
    steps(&mut app, 3);
    // 停在两步之间的一帧：显示的位置落后物理半步
    app.tick(VIEWPORT, STEP / 2.0);
    act(&mut app, a);
    steps(&mut app, 3);
    (pos(&app, a).x, pos(&app, b).x)
}

#[test]
fn rebuilding_a_body_keeps_its_physics_position() {
    let (rebuilt, untouched) = same_path_as_untouched(|app, e| {
        app.world_mut().entity_mut(e).insert(Collider { restitution: 0.5, ..Collider::circle(5.0) });
    });
    assert!((rebuilt - untouched).abs() < 1e-2, "重建不退回显示的位置: {rebuilt} / {untouched}");
}

#[test]
fn setting_only_the_rotation_keeps_the_translation() {
    let (rotated, untouched) = same_path_as_untouched(|app, e| app.world_mut().get_mut::<Transform2d>(e).unwrap().rotation = 1.0);
    assert!((rotated - untouched).abs() < 1e-2, "只改角度，位置不被拉回: {rotated} / {untouched}");
}

#[test]
fn a_box_on_a_rising_platform_is_drawn_resting_on_it() {
    let mut app = app();
    let lift = app.world_mut().spawn((RigidBody::Kinematic, Collider::rect(Vec2::new(200.0, 20.0)), at(0.0, -10.0), Velocity::default())).id();
    let crate_ = app.world_mut().spawn((RigidBody::Dynamic, Collider::rect(Vec2::new(20.0, 20.0)), at(0.0, 10.0), Velocity::default())).id();
    steps(&mut app, 30);
    // 120 Hz：游戏每帧把平台往上挪（和补间一样写绝对位置）
    let mut gaps = Vec::new();
    for frame in 0..120 {
        app.world_mut().get_mut::<Transform2d>(lift).unwrap().translation.y = -10.0 + 120.0 * frame as f32 / 120.0;
        app.tick(VIEWPORT, STEP / 2.0);
        if frame > 20 {
            gaps.push(drawn(&app, crate_).y - drawn(&app, lift).y);
        }
    }
    for gap in gaps {
        assert!((gap - 20.0).abs() < 0.5, "画面上箱子贴着平台，不陷进去：间距 {gap}");
    }
}

#[test]
fn draw_at_follows_fixed_bodies_and_goes_away_with_the_body() {
    let mut app = app();
    let e = app.world_mut().spawn((RigidBody::Fixed, Collider::circle(5.0), at(3.0, 4.0))).id();
    steps(&mut app, 1);
    assert_eq!(drawn(&app, e), Vec2::new(3.0, 4.0));
    app.world_mut().get_mut::<Transform2d>(e).unwrap().translation.x = 9.0;
    app.tick(VIEWPORT, STEP / 2.0);
    assert_eq!(drawn(&app, e).x, 9.0, "固定刚体画在 Transform2d 上");
    app.world_mut().entity_mut(e).remove::<RigidBody>();
    steps(&mut app, 2);
    assert!(app.world().get::<DrawAt>(e).is_none(), "去掉刚体时一起去掉");
}

#[test]
fn velocity_of_any_body() {
    let mut app = app();
    app.world_mut().resource_mut::<Physics>().set_gravity(Vec2::ZERO);
    let k = app.world_mut().spawn((RigidBody::Kinematic, Collider::circle(5.0), at(0.0, 0.0))).id();
    let wall = app.world_mut().spawn(Collider::circle(5.0)).id();
    steps(&mut app, 1);
    app.world_mut().get_mut::<Transform2d>(k).unwrap().translation.x = 1.0;
    steps(&mut app, 1);
    let v = app.world().resource::<Physics>().velocity(k).unwrap();
    assert!((v.linear.x - 60.0).abs() < 0.1, "kinematic 没有 Velocity 组件也查得到真实速度: {v:?}");
    assert_eq!(app.world().resource::<Physics>().velocity(wall), None, "只有碰撞体的没有速度");
}

#[test]
fn dragging_a_dynamic_body_every_frame_is_drawn_every_frame() {
    let mut app = app();
    app.world_mut().resource_mut::<Physics>().set_gravity(Vec2::ZERO);
    let e = app.world_mut().spawn((RigidBody::Dynamic, Collider::circle(5.0), at(0.0, 0.0))).id();
    steps(&mut app, 2);
    // 120 Hz 下每帧把它挪一点（比如鼠标拖）：不够一步的帧也要画在新位置
    for frame in 1..=10 {
        app.world_mut().get_mut::<Transform2d>(e).unwrap().translation.x = frame as f32;
        app.tick(VIEWPORT, STEP / 2.0);
        assert_eq!(drawn(&app, e).x, frame as f32, "第 {frame} 帧");
    }
}

#[test]
fn rebuilding_a_body_does_not_jump_the_drawing() {
    let mut app = app();
    let e = mover(&mut app);
    app.world_mut().entity_mut(e).insert(Collider::circle(5.0));
    // 120 Hz：每帧画面走半步（0.5）
    for _ in 0..6 {
        app.tick(VIEWPORT, STEP / 2.0);
    }
    for frame in 0..6 {
        if frame == 2 {
            app.world_mut().entity_mut(e).insert(Collider::circle(6.0));
        }
        let before = drawn(&app, e).x;
        app.tick(VIEWPORT, STEP / 2.0);
        let moved = drawn(&app, e).x - before;
        assert!((moved - 0.5).abs() < 1e-2, "第 {frame} 帧（第 2 帧重建，那一帧不够一步）画面照样走半步，不往前跳: {moved}");
    }
}

/// 地面（顶面 y = 0）、右边一堵墙（左面 x = 100）、墙前面一个 sensor、一个空中的箱子。
fn query_world(app: &mut App) -> (Entity, Entity, Entity, Entity) {
    let g = ground(app);
    let wall = app.world_mut().spawn((RigidBody::Fixed, Collider::rect(Vec2::new(20.0, 200.0)), at(110.0, 100.0))).id();
    let sensor = app.world_mut().spawn((RigidBody::Fixed, Collider { sensor: true, ..Collider::rect(Vec2::splat(20.0)) }, at(50.0, 50.0))).id();
    let b = app.world_mut().spawn((RigidBody::Fixed, Collider::rect(Vec2::splat(20.0)), at(0.0, 50.0))).id();
    steps(app, 1);
    (g, wall, sensor, b)
}

#[test]
fn raycast_finds_the_first_solid_collider() {
    let mut app = app();
    let (g, wall, sensor, b) = query_world(&mut app);
    let physics = app.world().resource::<Physics>();
    let hit = physics.raycast(Vec2::new(-200.0, 50.0), Vec2::new(5.0, 0.0), 1000.0, QueryFilter::default()).expect("碰到箱子");
    assert_eq!(hit.entity, b);
    assert!((hit.point - Vec2::new(-10.0, 50.0)).length() < 1e-3 && (hit.distance - 190.0).abs() < 1e-3, "{hit:?}");
    assert!((hit.normal - Vec2::NEG_X).length() < 1e-3, "碰到的面朝左 {hit:?}");

    let past = physics.raycast(Vec2::new(20.0, 50.0), Vec2::X, 1000.0, QueryFilter::default()).unwrap();
    assert_eq!(past.entity, wall, "默认穿过 sensor");
    let with = physics.raycast(Vec2::new(20.0, 50.0), Vec2::X, 1000.0, QueryFilter::default().with_sensors()).unwrap();
    assert_eq!(with.entity, sensor);
    assert!(physics.raycast(Vec2::new(20.0, 50.0), Vec2::X, 50.0, QueryFilter::default()).is_none(), "够不着墙");

    let down = physics.raycast(Vec2::new(0.0, 50.0), Vec2::NEG_Y, 1000.0, QueryFilter::default().exclude(b)).unwrap();
    assert_eq!(down.entity, g, "不算自己");
    assert!((down.distance - 50.0).abs() < 1e-3 && (down.normal - Vec2::Y).length() < 1e-3, "{down:?}");
    let inside = physics.raycast(Vec2::new(0.0, 50.0), Vec2::NEG_Y, 1000.0, QueryFilter::default()).unwrap();
    assert_eq!((inside.entity, inside.distance, inside.normal), (b, 0.0, Vec2::Y), "起点在里面立刻碰到，法线迎着射线");

    for (dir, max) in [(Vec2::ZERO, 10.0), (Vec2::new(f32::NAN, 1.0), 10.0), (Vec2::X, 0.0), (Vec2::X, f32::NAN)] {
        assert!(physics.raycast(Vec2::ZERO, dir, max, QueryFilter::default()).is_none(), "{dir} {max}");
    }
}

#[test]
fn overlaps_list_colliders_at_a_point_or_in_a_box() {
    let mut app = app();
    let (g, wall, sensor, b) = query_world(&mut app);
    let physics = app.world().resource::<Physics>();
    let sorted = |mut v: Vec<Entity>| {
        v.sort();
        v
    };
    assert_eq!(physics.overlap_point(Vec2::new(0.0, 55.0), QueryFilter::default()), [b]);
    assert!(physics.overlap_point(Vec2::new(50.0, 50.0), QueryFilter::default()).is_empty(), "默认不算 sensor");
    assert_eq!(physics.overlap_point(Vec2::new(50.0, 50.0), QueryFilter::default().with_sensors()), [sensor]);
    // 从箱子到墙、碰到地面的一条
    let all = physics.overlap_box(Vec2::new(50.0, 20.0), Vec2::new(55.0, 25.0), QueryFilter::default().with_sensors());
    assert_eq!(sorted(all), sorted(vec![g, wall, sensor, b]));
    let some = physics.overlap_box(Vec2::new(50.0, 20.0), Vec2::new(55.0, 25.0), QueryFilter::default().exclude(g));
    assert_eq!(sorted(some), sorted(vec![wall, b]));
    assert!(physics.overlap_box(Vec2::new(-300.0, 300.0), Vec2::splat(10.0), QueryFilter::default()).is_empty());
    assert!(physics.overlap_box(Vec2::ZERO, Vec2::new(f32::NAN, 1.0), QueryFilter::default()).is_empty(), "坏的尺寸");
    assert_eq!(physics.overlap_box(Vec2::new(0.0, 50.0), Vec2::new(-5.0, -5.0), QueryFilter::default()), [b], "负的半宽取绝对值");
}

#[test]
fn queries_see_the_world_as_of_the_last_step() {
    let mut app = app();
    let b = app.world_mut().spawn((RigidBody::Fixed, Collider::rect(Vec2::splat(20.0)), at(0.0, 0.0))).id();
    assert!(app.world().resource::<Physics>().overlap_point(Vec2::ZERO, QueryFilter::default()).is_empty(), "还没进物理");
    steps(&mut app, 1);
    assert_eq!(app.world().resource::<Physics>().overlap_point(Vec2::ZERO, QueryFilter::default()), [b]);
    app.world_mut().get_mut::<Transform2d>(b).unwrap().translation = Vec2::new(500.0, 0.0);
    assert_eq!(app.world().resource::<Physics>().overlap_point(Vec2::ZERO, QueryFilter::default()), [b], "下一步之前还在原处");
    steps(&mut app, 1);
    let physics = app.world().resource::<Physics>();
    assert!(physics.overlap_point(Vec2::ZERO, QueryFilter::default()).is_empty());
    assert_eq!(physics.overlap_point(Vec2::new(500.0, 0.0), QueryFilter::default()), [b]);
}

#[test]
fn ray_from_a_circle_centre_has_a_usable_normal() {
    let mut app = app();
    let c = app.world_mut().spawn((RigidBody::Fixed, Collider::circle(20.0), at(0.0, 0.0))).id();
    steps(&mut app, 1);
    let hit = app.world().resource::<Physics>().raycast(Vec2::ZERO, Vec2::new(3.0, 4.0), 100.0, QueryFilter::default()).unwrap();
    assert_eq!(hit.entity, c);
    assert!((hit.normal - Vec2::new(-0.6, -0.8)).length() < 1e-5, "不是 NaN {hit:?}");
}

/// 只有碰撞体、没有刚体的实体（比如瓦片地图下面的碰撞体）也能排除。
#[test]
fn excluding_a_collider_without_a_body() {
    let mut app = app();
    let only = app.world_mut().spawn((Collider::rect(Vec2::splat(20.0)), at(0.0, 0.0))).id();
    let below = app.world_mut().spawn((Collider::rect(Vec2::splat(20.0)), at(0.0, -40.0))).id();
    steps(&mut app, 1);
    let physics = app.world().resource::<Physics>();
    let down = |f| physics.raycast(Vec2::new(0.0, 50.0), Vec2::NEG_Y, 200.0, f).map(|h| h.entity);
    assert_eq!(down(QueryFilter::default()), Some(only));
    assert_eq!(down(QueryFilter::default().exclude(only)), Some(below));
}
