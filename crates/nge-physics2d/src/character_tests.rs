use glam::Vec2;
use nge_core::Viewport;
use nge_ecs::App;
use nge_render2d::{DrawAt, Transform2d};

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

fn velocity(app: &App, e: Entity) -> Vec2 {
    app.world().get::<Velocity>(e).unwrap().linear
}

fn set_velocity(app: &mut App, e: Entity, v: Vec2) {
    app.world_mut().get_mut::<Velocity>(e).unwrap().linear = v;
}

/// 一块固定的矩形：中心、宽高、角度（度）。
fn block(app: &mut App, center: Vec2, size: Vec2, degrees: f32) -> Entity {
    let t = Transform2d { translation: center, rotation: degrees.to_radians(), ..Default::default() };
    app.world_mut().spawn((RigidBody::Fixed, Collider::rect(size), t)).id()
}

/// 顶面在 y = 0、很宽的地面。
fn ground(app: &mut App) -> Entity {
    block(app, Vec2::new(0.0, -10.0), Vec2::new(4000.0, 20.0), 0.0)
}

/// 厚 20 的坡的中心：顶面从原点往右上走 `length`。
fn slope_center(angle: f32, length: f32) -> Vec2 {
    Vec2::new(angle.cos(), angle.sin()) * length / 2.0 + Vec2::new(angle.sin(), -angle.cos()) * 10.0
}

/// 20 × 20 的角色。
fn hero(app: &mut App, x: f32, y: f32, c: Character) -> Entity {
    app.world_mut().spawn((RigidBody::Kinematic, Collider::rect(Vec2::splat(20.0)), at(x, y), Velocity::default(), c)).id()
}

fn contacts(app: &App, e: Entity) -> Vec<Contact> {
    app.world().resource::<Physics>().contacts(e)
}

fn on_ground(app: &App, e: Entity) -> bool {
    contacts(app, e).iter().any(|c| c.normal.y > 0.7)
}

#[test]
fn falls_lands_and_stays_on_the_ground() {
    let mut app = app();
    let g = ground(&mut app);
    let h = hero(&mut app, 0.0, 100.0, Character::default());
    steps(&mut app, 10);
    assert!(velocity(&app, h).y < -100.0 && pos(&app, h).y < 100.0, "受重力往下掉");
    steps(&mut app, 60);
    let y = pos(&app, h).y;
    assert!((y - 10.0).abs() < 1.0, "站在地上 {y}");
    assert_eq!(velocity(&app, h).y, 0.0, "落地后往下的速度清零");
    let c = contacts(&app, h);
    assert!(c.iter().any(|c| c.other == g && c.normal.y > 0.99), "接触到地面，法线朝上 {c:?}");
    steps(&mut app, 60);
    assert!((pos(&app, h).y - y).abs() < 0.1, "一直站着，不往下陷");
    // 重力倍数
    let mut light = self::app();
    let h = hero(&mut light, 0.0, 100.0, Character { gravity_scale: 0.0, ..Character::default() });
    steps(&mut light, 30);
    assert_eq!(pos(&light, h), Vec2::new(0.0, 100.0));
}

#[test]
fn walks_up_and_down_a_gentle_slope_but_not_a_steep_one() {
    // 30° 的坡：从 x = 0 起往右上，下面接地面
    let mut app = app();
    ground(&mut app);
    let length = 400.0;
    let angle = 30f32.to_radians();
    block(&mut app, slope_center(angle, length), Vec2::new(length, 20.0), 30.0);
    let h = hero(&mut app, -60.0, 10.0, Character::default());
    steps(&mut app, 5);
    set_velocity(&mut app, h, Vec2::new(150.0, 0.0));
    let mut was_grounded = 0;
    for _ in 0..90 {
        steps(&mut app, 1);
        was_grounded += usize::from(on_ground(&app, h));
        let v = velocity(&app, h);
        set_velocity(&mut app, h, Vec2::new(150.0, v.y));
    }
    let up = pos(&app, h);
    assert!(up.x > 100.0 && up.y > 60.0, "爬上去了 {up}");
    assert!(was_grounded >= 85, "一直贴着坡 {was_grounded}");
    // 往回走：下坡也贴着，不飞出去
    let mut airborne = 0;
    for _ in 0..60 {
        let v = velocity(&app, h);
        set_velocity(&mut app, h, Vec2::new(-150.0, v.y));
        steps(&mut app, 1);
        airborne += usize::from(!on_ground(&app, h));
    }
    assert!(pos(&app, h).y < up.y - 30.0, "下来了");
    assert!(airborne <= 2, "下坡贴着地 {airborne}");

    // 60° 的坡爬不上
    let mut app = self::app();
    ground(&mut app);
    let angle = 60f32.to_radians();
    block(&mut app, slope_center(angle, length), Vec2::new(length, 20.0), 60.0);
    let h = hero(&mut app, -60.0, 10.0, Character::default());
    for _ in 0..120 {
        let v = velocity(&app, h);
        set_velocity(&mut app, h, Vec2::new(150.0, v.y));
        steps(&mut app, 1);
    }
    assert!(pos(&app, h).y < 40.0, "太陡，上不去 {}", pos(&app, h));
    // 坡度上限改大就上得去。坡脚往地里埋一点：和地面齐平时坡脚是个尖角，rapier 会把它当墙挡住
    let mut app = self::app();
    ground(&mut app);
    block(&mut app, slope_center(angle, length) - Vec2::Y * 2.0, Vec2::new(length, 20.0), 60.0);
    let h = hero(&mut app, -60.0, 10.0, Character { max_slope: 70f32.to_radians(), ..Character::default() });
    let mut stopped = 0;
    for _ in 0..120 {
        let v = velocity(&app, h);
        set_velocity(&mut app, h, Vec2::new(150.0, v.y));
        steps(&mut app, 1);
        let y = pos(&app, h).y;
        stopped += usize::from(y > 15.0 && y < 50.0 && velocity(&app, h).x == 0.0);
    }
    assert_eq!(stopped, 0, "坡上不当墙清零速度");
    assert!(pos(&app, h).y > 60.0, "上去了 {}", pos(&app, h));
}

#[test]
fn steps_over_low_ledges_only_when_allowed() {
    for (step, height, passes) in [(8.0, 6.0, true), (0.0, 6.0, false), (8.0, 14.0, false)] {
        let mut app = app();
        ground(&mut app);
        block(&mut app, Vec2::new(100.0, height / 2.0), Vec2::new(400.0, height), 0.0);
        let h = hero(&mut app, -150.0, 10.0, Character { step, ..Character::default() });
        for _ in 0..120 {
            let v = velocity(&app, h);
            set_velocity(&mut app, h, Vec2::new(150.0, v.y));
            steps(&mut app, 1);
        }
        let x = pos(&app, h).x;
        assert_eq!(x > 0.0, passes, "台阶 {step}、高 {height}：{x}");
    }
}

#[test]
fn walls_and_ceilings_stop_the_velocity() {
    let mut app = app();
    ground(&mut app);
    let wall = block(&mut app, Vec2::new(100.0, 100.0), Vec2::new(20.0, 200.0), 0.0);
    let h = hero(&mut app, 0.0, 10.0, Character::default());
    steps(&mut app, 2);
    set_velocity(&mut app, h, Vec2::new(300.0, 0.0));
    steps(&mut app, 30);
    assert!((pos(&app, h).x - 80.0).abs() < 1.0, "停在墙前 {}", pos(&app, h));
    assert_eq!(velocity(&app, h).x, 0.0, "朝墙的速度清零");
    assert!(contacts(&app, h).iter().any(|c| c.other == wall && c.normal.x < -0.99), "{:?}", contacts(&app, h));
    // 天花板：顶面在 y = 60
    let mut app = self::app();
    ground(&mut app);
    block(&mut app, Vec2::new(0.0, 70.0), Vec2::new(200.0, 20.0), 0.0);
    let h = hero(&mut app, 0.0, 10.0, Character::default());
    steps(&mut app, 2);
    set_velocity(&mut app, h, Vec2::new(0.0, 600.0));
    let mut stopped = false;
    for _ in 0..10 {
        steps(&mut app, 1);
        stopped |= velocity(&app, h).y <= 0.0 && pos(&app, h).y > 40.0;
    }
    assert!(stopped, "顶到天花板，往上的速度清零");
    assert!(pos(&app, h).y < 51.0, "没穿过去 {}", pos(&app, h));
}

/// 一块 kinematic 平台（100 × 20，顶面在 y = 0），每帧按 `velocity` 挪它的 `Transform2d`。
fn ride(platform_velocity: Vec2) -> (Vec2, Vec2, bool) {
    let mut app = app();
    let platform = app.world_mut().spawn((RigidBody::Kinematic, Collider::rect(Vec2::new(100.0, 20.0)), at(0.0, -10.0))).id();
    let h = hero(&mut app, 0.0, 10.0, Character::default());
    steps(&mut app, 5);
    let start = pos(&app, h);
    let mut always = true;
    // 平台刚开始动的这一帧角色就跟着挪一样多（跟的是平台这一步的位移，不是上一步的速度）
    let (p0, h0) = (pos(&app, platform), pos(&app, h));
    app.world_mut().get_mut::<Transform2d>(platform).unwrap().translation += platform_velocity * STEP;
    steps(&mut app, 1);
    let (dp, dh) = (pos(&app, platform) - p0, pos(&app, h) - h0);
    assert!((dp - dh).length() < 0.05, "平台挪了 {dp}，角色挪了 {dh}");
    for _ in 0..29 {
        app.world_mut().get_mut::<Transform2d>(platform).unwrap().translation += platform_velocity * STEP;
        steps(&mut app, 1);
        always &= on_ground(&app, h);
    }
    (start, pos(&app, h), always)
}

#[test]
fn rides_moving_platforms() {
    for v in [Vec2::new(100.0, 0.0), Vec2::new(0.0, 100.0), Vec2::new(0.0, -100.0), Vec2::new(-80.0, 60.0)] {
        let (start, end, always) = ride(v);
        let moved = end - start;
        // 平台 30 帧走了 v / 2；角色比平台晚一步跟上
        assert!((moved - v / 2.0).length() < v.length() * STEP * 1.5 + 0.5, "平台 {v}：角色挪了 {moved}");
        assert!(always, "平台 {v}：一直站在上面");
    }
}

#[test]
fn pushes_dynamic_bodies_and_touches_kinematic_ones() {
    let mut app = app();
    ground(&mut app);
    let crate_ = app.world_mut().spawn((RigidBody::Dynamic, Collider { friction: 0.2, ..Collider::rect(Vec2::splat(30.0)) }, at(60.0, 15.0))).id();
    let h = hero(&mut app, 0.0, 10.0, Character::default());
    steps(&mut app, 5);
    for _ in 0..60 {
        let v = velocity(&app, h);
        set_velocity(&mut app, h, Vec2::new(120.0, v.y));
        steps(&mut app, 1);
    }
    assert!(pos(&app, crate_).x > 90.0, "箱子被推走了 {}", pos(&app, crate_));
    // kinematic 的敌人：挡住、碰得到，法线从敌人指向角色
    let mut app = self::app();
    ground(&mut app);
    let enemy = app.world_mut().spawn((RigidBody::Kinematic, Collider::rect(Vec2::splat(20.0)), at(60.0, 10.0))).id();
    let h = hero(&mut app, 0.0, 10.0, Character::default());
    steps(&mut app, 2);
    set_velocity(&mut app, h, Vec2::new(200.0, 0.0));
    steps(&mut app, 20);
    let side = contacts(&app, h).into_iter().find(|c| c.other == enemy).expect("碰到敌人");
    assert!(side.normal.x < -0.99, "{side:?}");
    // 从上面落下来
    app.world_mut().get_mut::<Transform2d>(h).unwrap().translation = Vec2::new(60.0, 60.0);
    set_velocity(&mut app, h, Vec2::ZERO);
    steps(&mut app, 30);
    let top = contacts(&app, h).into_iter().find(|c| c.other == enemy).expect("踩到敌人");
    assert!(top.normal.y > 0.99, "{top:?}");
}

#[test]
fn riding_into_a_wall_stops_at_the_wall() {
    // 平台往右走，角色也往右走，前面有一堵固定的墙（左面在 x = 60，底在平台上方）
    let mut app = app();
    let platform = app.world_mut().spawn((RigidBody::Kinematic, Collider::rect(Vec2::new(400.0, 20.0)), at(0.0, -10.0))).id();
    block(&mut app, Vec2::new(70.0, 62.0), Vec2::new(20.0, 100.0), 0.0);
    // 起点错开一点：不然每帧走的距离正好凑到墙前
    let h = hero(&mut app, 1.3, 10.0, Character::default());
    steps(&mut app, 5);
    for _ in 0..60 {
        app.world_mut().get_mut::<Transform2d>(platform).unwrap().translation.x += 100.0 * STEP;
        let v = velocity(&app, h);
        set_velocity(&mut app, h, Vec2::new(150.0, v.y));
        steps(&mut app, 1);
        assert!(pos(&app, h).x < 50.1, "插进墙里 {}", pos(&app, h));
    }
}

#[test]
fn teleporting_off_a_platform_stops_riding_it() {
    let mut app = app();
    ground(&mut app);
    let platform = app.world_mut().spawn((RigidBody::Kinematic, Collider::rect(Vec2::new(100.0, 20.0)), at(0.0, 40.0))).id();
    let h = hero(&mut app, 0.0, 60.0, Character::default());
    steps(&mut app, 5);
    app.world_mut().get_mut::<Transform2d>(platform).unwrap().translation.x += 100.0 * STEP;
    steps(&mut app, 1);
    app.world_mut().get_mut::<Transform2d>(platform).unwrap().translation.x += 100.0 * STEP;
    app.world_mut().get_mut::<Transform2d>(h).unwrap().translation = Vec2::new(300.0, 10.0);
    steps(&mut app, 1);
    assert!((pos(&app, h).x - 300.0).abs() < 0.1, "还被平台带着走 {}", pos(&app, h));
}

#[test]
fn nan_velocity_does_not_freeze_the_character() {
    let mut app = app();
    ground(&mut app);
    let h = hero(&mut app, 0.0, 10.0, Character::default());
    steps(&mut app, 2);
    set_velocity(&mut app, h, Vec2::new(0.0, f32::NAN));
    steps(&mut app, 1);
    // 游戏常这样写：只改横向速度，竖直的沿用
    for _ in 0..10 {
        let v = velocity(&app, h);
        set_velocity(&mut app, h, Vec2::new(150.0, v.y));
        steps(&mut app, 1);
    }
    assert!(pos(&app, h).x > 10.0, "动不了 {}", pos(&app, h));
}

#[test]
fn walls_moving_beside_do_not_carry() {
    let mut app = app();
    ground(&mut app);
    let wall = app.world_mut().spawn((RigidBody::Kinematic, Collider::rect(Vec2::new(20.0, 400.0)), at(21.0, 200.0))).id();
    let h = hero(&mut app, 0.0, 10.0, Character::default());
    steps(&mut app, 5);
    assert!(contacts(&app, h).iter().any(|c| c.other == wall), "贴着墙");
    let mut highest = pos(&app, h).y;
    for _ in 0..30 {
        app.world_mut().get_mut::<Transform2d>(wall).unwrap().translation.y += 3.0;
        steps(&mut app, 1);
        highest = highest.max(pos(&app, h).y);
    }
    assert!(highest < 11.0, "只跟脚下的走 {highest}");
}

#[test]
fn parameters_follow_the_component() {
    let mut app = app();
    let h = hero(&mut app, 0.0, 100.0, Character::default());
    steps(&mut app, 10);
    app.world_mut().get_mut::<Character>(h).unwrap().gravity_scale = 0.0;
    let v = velocity(&app, h);
    steps(&mut app, 10);
    assert_eq!(velocity(&app, h), v, "没有重力就不再加速");
}

#[test]
fn sensors_report_collisions_with_characters() {
    let mut app = app();
    ground(&mut app);
    let coin = app.world_mut().spawn((Collider { sensor: true, events: true, ..Collider::rect(Vec2::splat(10.0)) }, at(60.0, 10.0))).id();
    let h = hero(&mut app, 0.0, 10.0, Character::default());
    steps(&mut app, 2);
    set_velocity(&mut app, h, Vec2::new(200.0, 0.0));
    let mut started = Vec::new();
    for _ in 0..30 {
        steps(&mut app, 1);
        started.extend(app.world_mut().resource_mut::<bevy_ecs::message::Messages<Collision>>().drain().filter(|c| c.started));
    }
    assert!(started.iter().any(|c| [c.a, c.b].contains(&coin) && [c.a, c.b].contains(&h)), "{started:?}");
    assert!(pos(&app, h).x > 80.0, "sensor 不挡路");
}

#[test]
fn moving_the_transform_teleports() {
    let mut app = app();
    ground(&mut app);
    let h = hero(&mut app, 0.0, 10.0, Character::default());
    steps(&mut app, 5);
    app.world_mut().get_mut::<Transform2d>(h).unwrap().translation = Vec2::new(500.0, 10.0);
    steps(&mut app, 1);
    assert!((pos(&app, h) - Vec2::new(500.0, 10.0)).length() < 0.5, "{}", pos(&app, h));
    let drawn = app.world().get::<DrawAt>(h).unwrap().translation;
    assert!((drawn - Vec2::new(500.0, 10.0)).length() < 0.5, "画面不从旧位置滑过去 {drawn}");
    // 去掉 Character 就是普通的 kinematic：不受重力
    app.world_mut().entity_mut(h).remove::<Character>();
    app.world_mut().get_mut::<Transform2d>(h).unwrap().translation = Vec2::new(500.0, 200.0);
    steps(&mut app, 30);
    assert_eq!(pos(&app, h), Vec2::new(500.0, 200.0));
}
