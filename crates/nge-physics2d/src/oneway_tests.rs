//! 单向平台（见 docs/gameplay2d.md）。

use glam::Vec2;
use nge_core::Viewport;
use nge_ecs::App;
use nge_render2d::Transform2d;

use super::*;

const VIEWPORT: Viewport = Viewport { size: [100.0, 100.0], pixels: [100, 100] };

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(PhysicsPlugin { gravity: Vec2::new(0.0, -1000.0), pixels_per_meter: 100.0 });
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

fn set_velocity(app: &mut App, e: Entity, v: Vec2) {
    app.world_mut().get_mut::<Velocity>(e).unwrap().linear = v;
}

/// 顶面在 y = 0 的地面，和顶面在 y = 100、厚 10 的单向平台（只有碰撞体，当固定的）。
fn level(app: &mut App) -> (Entity, Entity) {
    let ground = app.world_mut().spawn((RigidBody::Fixed, Collider::rect(Vec2::new(2000.0, 20.0)), at(0.0, -10.0))).id();
    let plank = app.world_mut().spawn((Collider { one_way: true, ..Collider::rect(Vec2::new(200.0, 10.0)) }, at(0.0, 95.0))).id();
    (ground, plank)
}

fn hero(app: &mut App, x: f32, y: f32) -> Entity {
    app.world_mut().spawn((RigidBody::Kinematic, Collider::rect(Vec2::splat(20.0)), at(x, y), Velocity::default(), Character::default())).id()
}

fn standing_on(app: &App, e: Entity, other: Entity) -> bool {
    app.world().resource::<Physics>().contacts(e).iter().any(|c| c.other == other && c.normal.y > 0.7)
}

#[test]
fn characters_jump_up_through_and_stand_on_one_way_platforms() {
    let mut app = app();
    let (ground, plank) = level(&mut app);
    let h = hero(&mut app, 0.0, 10.0);
    steps(&mut app, 5);
    assert!(standing_on(&app, h, ground));
    // 起跳 600：能跳 180 高，从平台底下穿上去
    set_velocity(&mut app, h, Vec2::new(0.0, 600.0));
    let mut highest = 0f32;
    let mut on_plank_while_rising = false;
    for _ in 0..90 {
        steps(&mut app, 1);
        highest = highest.max(pos(&app, h).y);
        let v = app.world().get::<Velocity>(h).unwrap().linear;
        on_plank_while_rising |= v.y > 0.0 && standing_on(&app, h, plank);
    }
    assert!(highest > 120.0, "穿过平台往上走，没被挡住 {highest}");
    assert!(!on_plank_while_rising, "穿过的时候不算站在平台上");
    let y = pos(&app, h).y;
    assert!((y - 110.0).abs() < 1.0, "落回来站在平台上 {y}");
    assert!(standing_on(&app, h, plank));
    steps(&mut app, 60);
    assert!((pos(&app, h).y - y).abs() < 0.1, "一直站着");
}

#[test]
fn characters_drop_through_and_land_again_from_above() {
    let mut app = app();
    let (ground, plank) = level(&mut app);
    let h = hero(&mut app, 0.0, 130.0);
    steps(&mut app, 30);
    assert!(standing_on(&app, h, plank), "落在平台上 {}", pos(&app, h));
    assert!(!app.world_mut().resource_mut::<Physics>().drop_through(ground), "不是角色、没站在单向平台上的不管");
    assert!(app.world_mut().resource_mut::<Physics>().drop_through(h), "站在单向平台上");
    steps(&mut app, 60);
    let y = pos(&app, h).y;
    assert!((y - 10.0).abs() < 1.0, "穿下去落到地面 {y}");
    assert!(standing_on(&app, h, ground));
    // 再跳上去：平台又能站
    set_velocity(&mut app, h, Vec2::new(0.0, 600.0));
    steps(&mut app, 90);
    assert!(standing_on(&app, h, plank), "又能站在平台上 {}", pos(&app, h));
}

#[test]
fn heads_touching_a_platform_from_below_do_not_drop_through_it() {
    // 地面上的角色头顶刚好贴着单向平台的底：不算站在上面，按下加跳照常跳
    let mut app = app();
    let ground = app.world_mut().spawn((RigidBody::Fixed, Collider::rect(Vec2::new(2000.0, 20.0)), at(0.0, -10.0))).id();
    app.world_mut().spawn((Collider { one_way: true, ..Collider::rect(Vec2::new(200.0, 10.0)) }, at(0.0, 25.0)));
    let h = hero(&mut app, 0.0, 10.0);
    steps(&mut app, 5);
    assert!(standing_on(&app, h, ground));
    assert!(!app.world_mut().resource_mut::<Physics>().drop_through(h), "头顶贴着平台的底");
    assert!(standing_on(&app, h, ground), "还站在地上");
}

#[test]
fn walking_off_a_one_way_platform_onto_one_below_lands() {
    // 两层单向平台，从上面那层走到边上掉下去，落在下面那层
    let mut app = app();
    app.world_mut().spawn((Collider { one_way: true, ..Collider::rect(Vec2::new(100.0, 10.0)) }, at(0.0, 195.0)));
    let low = app.world_mut().spawn((Collider { one_way: true, ..Collider::rect(Vec2::new(400.0, 10.0)) }, at(100.0, 95.0))).id();
    let h = hero(&mut app, 0.0, 215.0);
    steps(&mut app, 10);
    for _ in 0..80 {
        let vy = app.world().get::<Velocity>(h).unwrap().linear.y;
        set_velocity(&mut app, h, Vec2::new(150.0, vy));
        steps(&mut app, 1);
    }
    assert!(standing_on(&app, h, low), "落在下面那层 {}", pos(&app, h));
}

#[test]
fn dynamic_bodies_pass_up_through_and_rest_on_one_way_platforms() {
    let mut app = app();
    let (_, plank) = level(&mut app);
    let ball = app.world_mut().spawn((RigidBody::Dynamic, Collider::circle(10.0), at(0.0, 30.0), Velocity { linear: Vec2::new(0.0, 700.0), angular: 0.0 })).id();
    let mut highest = 0f32;
    for _ in 0..120 {
        steps(&mut app, 1);
        highest = highest.max(pos(&app, ball).y);
    }
    assert!(highest > 150.0, "从下面穿上去 {highest}");
    let y = pos(&app, ball).y;
    assert!((y - 110.0).abs() < 1.5, "落在平台上 {y}");
    // 从上面砸下来的箱子也停在上面
    let crate_ = app.world_mut().spawn((RigidBody::Dynamic, Collider::rect(Vec2::splat(20.0)), at(60.0, 300.0))).id();
    steps(&mut app, 90);
    assert!((pos(&app, crate_).y - 110.0).abs() < 1.5, "箱子停在平台上 {}", pos(&app, crate_));
    // 穿下去
    assert!(app.world_mut().resource_mut::<Physics>().drop_through(crate_));
    steps(&mut app, 90);
    assert!((pos(&app, crate_).y - 10.0).abs() < 1.5, "穿下去落到地面 {}", pos(&app, crate_));
    let _ = plank;
}

#[test]
fn one_way_platforms_still_block_from_above_when_moving_kinematically() {
    // 挂在 kinematic 刚体上的单向平台往上走，把站在上面的角色托着走；角色从下面跳过去不被挡
    let mut app = app();
    app.world_mut().spawn((RigidBody::Fixed, Collider::rect(Vec2::new(2000.0, 20.0)), at(0.0, -10.0)));
    let lift = app.world_mut().spawn((RigidBody::Kinematic, Collider { one_way: true, ..Collider::rect(Vec2::new(100.0, 10.0)) }, at(0.0, 45.0))).id();
    let h = hero(&mut app, 0.0, 70.0);
    steps(&mut app, 20);
    assert!(standing_on(&app, h, lift), "站在平台上 {}", pos(&app, h));
    for i in 0..60 {
        app.world_mut().get_mut::<Transform2d>(lift).unwrap().translation.y = 45.0 + i as f32;
        steps(&mut app, 1);
    }
    let y = pos(&app, h).y;
    assert!((y - (45.0 + 59.0 + 15.0)).abs() < 2.0, "被托上去 {y}");
}

#[test]
fn standing_on_the_seam_of_a_solid_block_and_a_plank_does_not_drop() {
    // 实心块 x ∈ [-100, 0]，单向板 x ∈ [0, 200]，顶面都在 y = 100；角色一半踩着实心块
    let mut app = app();
    app.world_mut().spawn((RigidBody::Fixed, Collider::rect(Vec2::new(100.0, 10.0)), at(-50.0, 95.0)));
    let plank = app.world_mut().spawn((Collider { one_way: true, ..Collider::rect(Vec2::new(200.0, 10.0)) }, at(100.0, 95.0))).id();
    app.world_mut().spawn((RigidBody::Fixed, Collider::rect(Vec2::new(2000.0, 20.0)), at(0.0, -10.0)));
    let h = hero(&mut app, -2.0, 112.0);
    steps(&mut app, 10);
    assert!(!app.world_mut().resource_mut::<Physics>().drop_through(h), "脚下有实心的，不穿");
    assert!(app.world().resource::<Physics>().dropping.is_empty(), "也不记着往下穿");
    // 往右走到单向板上：一直站着
    let mut lowest = f32::MAX;
    for _ in 0..60 {
        let vy = app.world().get::<Velocity>(h).unwrap().linear.y;
        set_velocity(&mut app, h, Vec2::new(100.0, vy));
        steps(&mut app, 1);
        lowest = lowest.min(pos(&app, h).y);
    }
    assert!(lowest > 109.0, "走到板上没掉下去 {lowest}");
    assert!(standing_on(&app, h, plank));
}

#[test]
fn landing_on_something_else_forgets_the_drop() {
    // 平台底下 20 处就是地面：穿下去落地时头还贴着平台的底
    let mut app = app();
    app.world_mut().spawn((Collider { one_way: true, ..Collider::rect(Vec2::new(200.0, 10.0)) }, at(0.0, 95.0)));
    app.world_mut().spawn((RigidBody::Fixed, Collider::rect(Vec2::new(2000.0, 20.0)), at(0.0, 60.0)));
    let h = hero(&mut app, 0.0, 115.0);
    steps(&mut app, 10);
    assert!(app.world_mut().resource_mut::<Physics>().drop_through(h));
    steps(&mut app, 30);
    assert!((pos(&app, h).y - 80.0).abs() < 1.0, "落到地面 {}", pos(&app, h));
    assert!(app.world().resource::<Physics>().dropping.is_empty(), "站到别的东西上就不算在穿了");
}

#[test]
fn dropping_through_stacked_planks_passes_the_ones_right_below() {
    // 两块单向板上下挨着（像两行单向瓦片）：穿下去两块都穿过，落到地面
    let mut app = app();
    let (ground, _) = level(&mut app);
    app.world_mut().spawn((Collider { one_way: true, ..Collider::rect(Vec2::new(200.0, 10.0)) }, at(0.0, 85.0)));
    let h = hero(&mut app, 0.0, 115.0);
    steps(&mut app, 10);
    assert!((pos(&app, h).y - 110.0).abs() < 1.0);
    assert!(app.world_mut().resource_mut::<Physics>().drop_through(h));
    assert_eq!(app.world().resource::<Physics>().dropping.len(), 2, "两块都记着");
    steps(&mut app, 60);
    assert!((pos(&app, h).y - 10.0).abs() < 1.0, "两块都穿过 {}", pos(&app, h));
    assert!(standing_on(&app, h, ground));
}

#[test]
fn boxes_barely_clearing_a_platform_land_on_it() {
    // 从平台下面往上抛，刚好够越过平台顶面：落回来停在平台上，不掉回去
    for vy in [410.0f32, 420.0, 430.0] {
        let mut app = app();
        let (_, plank) = level(&mut app);
        let b = app.world_mut().spawn((RigidBody::Dynamic, Collider::rect(Vec2::splat(20.0)), at(0.0, 30.0), Velocity { linear: Vec2::new(0.0, vy), angular: 0.0 })).id();
        let mut highest = 0f32;
        for _ in 0..90 {
            steps(&mut app, 1);
            highest = highest.max(pos(&app, b).y);
        }
        assert!(highest > 110.0, "越过了平台 vy {vy} {highest}");
        assert!((pos(&app, b).y - 110.0).abs() < 1.5, "vy {vy}：停在平台上 {}", pos(&app, b));
        let _ = plank;
    }
}
