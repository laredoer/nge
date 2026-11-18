//! 3D 角色控制器（按 point 用：重力 -981，一米 100）。

use bevy_ecs::message::Messages;
use glam::{Quat, Vec3};
use nge_render3d::Transform3d;

use super::*;
use crate::tests::{app, at, crate_at, ground, pos, steps};

/// 站着时胶囊中心离地多高：半高 20 + 半径 15。
const STAND: f32 = 35.0;

fn hero(app: &mut nge_ecs::App, x: f32, y: f32, z: f32, c: Character3d) -> Entity {
    let collider = Collider { events: true, ..Collider::capsule(20.0, 15.0) };
    app.world_mut().spawn((RigidBody::Kinematic, collider, at(x, y, z), Velocity::default(), c)).id()
}

fn velocity(app: &nge_ecs::App, e: Entity) -> Vec3 {
    app.world().get::<Velocity>(e).unwrap().linear
}

fn set_velocity(app: &mut nge_ecs::App, e: Entity, v: Vec3) {
    app.world_mut().get_mut::<Velocity>(e).unwrap().linear = v;
}

fn state(app: &nge_ecs::App, e: Entity) -> CharacterState3d {
    app.world().get::<CharacterState3d>(e).expect("角色自动带着状态").clone()
}

/// 固定的盒子：中心、整个长宽高、绕 z 转的角度（度）。
fn block(app: &mut nge_ecs::App, center: Vec3, size: Vec3, degrees: f32) -> Entity {
    let t = Transform3d { translation: center, rotation: Quat::from_rotation_z(degrees.to_radians()), ..Default::default() };
    app.world_mut().spawn((RigidBody::Fixed, Collider::cuboid(size), t)).id()
}

/// 厚 20 的坡：顶面从原点往 +x 方向斜上去，长 `length`。
fn slope(app: &mut nge_ecs::App, degrees: f32, length: f32) -> Entity {
    let a = degrees.to_radians();
    let center = Vec3::new(a.cos(), a.sin(), 0.0) * length / 2.0 + Vec3::new(a.sin(), -a.cos(), 0.0) * 10.0;
    block(app, center, Vec3::new(length, 20.0, 200.0), degrees)
}

#[test]
fn falls_lands_and_reports_the_ground() {
    let mut app = app();
    let g = ground(&mut app);
    let h = hero(&mut app, 0.0, 200.0, 0.0, Character3d::default());
    steps(&mut app, 10);
    assert!(velocity(&app, h).y < -100.0 && !state(&app, h).grounded, "受重力往下掉");
    assert!(state(&app, h).velocity.y < -100.0, "输出的速度是实际的位移 ÷ 时间");
    steps(&mut app, 80);
    let p = pos(&app, h);
    assert!((p.y - STAND).abs() < 2.0, "站在地上 {p}");
    assert_eq!(velocity(&app, h).y, 0.0, "落地后往下的速度清零");
    let s = state(&app, h);
    assert!(s.grounded && s.ground_normal.distance(Vec3::Y) < 1e-3, "{s:?}");
    assert!(s.velocity.length() < 1.0, "站着不动 {:?}", s.velocity);
    let c = app.world().resource::<Physics3d>().contacts(h);
    assert!(c.iter().any(|c| c.other == g && c.normal.y > 0.99), "接触到地面，法线朝上 {c:?}");
    steps(&mut app, 60);
    assert!((pos(&app, h).y - p.y).abs() < 0.1, "站稳了不往下沉");
}

#[test]
fn walks_up_a_gentle_slope_but_not_a_steep_one() {
    for (degrees, climbs) in [(20.0, true), (65.0, false)] {
        let mut app = app();
        ground(&mut app);
        slope(&mut app, degrees, 400.0);
        let h = hero(&mut app, -60.0, STAND + 1.0, 0.0, Character3d::default());
        steps(&mut app, 10);
        for _ in 0..120 {
            let v = velocity(&app, h);
            set_velocity(&mut app, h, Vec3::new(150.0, v.y, 0.0));
            steps(&mut app, 1);
        }
        let p = pos(&app, h);
        if climbs {
            assert!(p.x > 80.0 && p.y > STAND + 40.0, "{degrees}° 的坡走上去了 {p}");
            assert!(state(&app, h).grounded, "爬坡时一直贴着地");
            assert!(state(&app, h).ground_normal.y < 0.97, "地面法线是坡的法线 {:?}", state(&app, h).ground_normal);
        } else {
            assert!(p.y < STAND + 25.0, "{degrees}° 的坡爬不上去 {p}");
        }
    }
}

#[test]
fn steps_up_low_ledges_only() {
    for (height, climbs) in [(12.0, true), (35.0, false)] {
        let mut app = app();
        ground(&mut app);
        block(&mut app, Vec3::new(300.0, height / 2.0, 0.0), Vec3::new(400.0, height, 200.0), 0.0);
        let h = hero(&mut app, 0.0, STAND + 1.0, 0.0, Character3d { step: 20.0, ..Default::default() });
        steps(&mut app, 10);
        for _ in 0..90 {
            let v = velocity(&app, h);
            set_velocity(&mut app, h, Vec3::new(150.0, v.y, 0.0));
            steps(&mut app, 1);
        }
        let p = pos(&app, h);
        if climbs {
            assert!(p.x > 120.0 && (p.y - (STAND + height)).abs() < 3.0, "迈上 {height} 高的台阶 {p}");
        } else {
            assert!(p.x < 100.0 && (p.y - STAND).abs() < 3.0, "{height} 太高，停在前面 {p}");
            assert_eq!(velocity(&app, h).x, 0.0, "撞墙速度清零");
        }
    }
}

#[test]
fn slides_along_walls() {
    let mut app = app();
    let g = ground(&mut app);
    let wall = block(&mut app, Vec3::new(100.0, 100.0, 0.0), Vec3::new(20.0, 200.0, 2000.0), 0.0);
    let h = hero(&mut app, 0.0, STAND + 1.0, 0.0, Character3d::default());
    steps(&mut app, 10);
    for _ in 0..60 {
        let v = velocity(&app, h);
        set_velocity(&mut app, h, Vec3::new(200.0, v.y, -100.0));
        steps(&mut app, 1);
    }
    let p = pos(&app, h);
    assert!(p.x < 90.0 - 14.0 && p.x > 70.0, "停在墙前 {p}");
    assert!(p.z < -60.0, "沿墙往 -z 滑 {p}");
    let v = velocity(&app, h);
    assert!(v.x.abs() < 1.0 && (v.z + 100.0).abs() < 2.0, "朝墙的分量去掉、沿墙的留着 {v}");
    let s = state(&app, h);
    assert!(s.hits.iter().any(|c| c.other == wall && c.normal.x < -0.99), "碰撞列表里有墙，法线朝角色 {:?}", s.hits);
    assert!(s.velocity.x.abs() < 1.0 && (s.velocity.z + 100.0).abs() < 2.0, "实际速度只剩沿墙的 {:?}", s.velocity);
    let _ = g;
}

#[test]
fn hitting_the_ceiling_stops_the_jump() {
    let mut app = app();
    ground(&mut app);
    block(&mut app, Vec3::new(0.0, 110.0, 0.0), Vec3::new(400.0, 20.0, 400.0), 0.0);
    let h = hero(&mut app, 0.0, STAND + 1.0, 0.0, Character3d::default());
    steps(&mut app, 10);
    set_velocity(&mut app, h, Vec3::new(0.0, 600.0, 0.0));
    steps(&mut app, 8);
    assert!(pos(&app, h).y < 100.0 - STAND + 1.0, "顶到天花板 {}", pos(&app, h));
    assert!(velocity(&app, h).y <= 0.0, "往上的速度清零 {}", velocity(&app, h));
}

/// 角色站在一块 kinematic 平台上：返回（平台，角色）。
fn on_a_lift(app: &mut nge_ecs::App) -> (Entity, Entity) {
    let lift = app.world_mut().spawn((RigidBody::Kinematic, Collider::cuboid(Vec3::new(200.0, 20.0, 200.0)), at(0.0, 90.0, 0.0))).id();
    let h = hero(app, 0.0, 100.0 + STAND + 1.0, 0.0, Character3d::default());
    steps(app, 20);
    assert!(state(app, h).grounded, "先站稳");
    (lift, h)
}

#[test]
fn rides_moving_platforms() {
    for step in [Vec3::new(2.0, 0.0, -1.0), Vec3::new(0.0, 1.5, 0.0), Vec3::new(0.0, -1.5, 0.0)] {
        let mut app = app();
        let (lift, h) = on_a_lift(&mut app);
        let before = pos(&app, h);
        for _ in 0..40 {
            app.world_mut().get_mut::<Transform3d>(lift).unwrap().translation += step;
            steps(&mut app, 1);
        }
        let moved = pos(&app, h) - before;
        assert!(moved.distance(step * 40.0) < 3.0, "跟着平台走了 {moved}，平台走了 {}", step * 40.0);
        assert!(state(&app, h).grounded, "一直站在平台上");
    }
}

#[test]
fn pushes_dynamic_boxes() {
    let mut app = app();
    ground(&mut app);
    let b = crate_at(&mut app, 60.0, 10.0, 0.0);
    let h = hero(&mut app, 0.0, STAND + 1.0, 0.0, Character3d::default());
    steps(&mut app, 10);
    let start = pos(&app, b).x;
    for _ in 0..60 {
        let v = velocity(&app, h);
        set_velocity(&mut app, h, Vec3::new(120.0, v.y, 0.0));
        steps(&mut app, 1);
    }
    assert!(pos(&app, b).x > start + 30.0, "箱子被推走 {}", pos(&app, b));
}

#[test]
fn teleporting_does_not_slide_and_forgets_the_platform() {
    let mut app = app();
    ground(&mut app);
    let (lift, h) = on_a_lift(&mut app);
    app.world_mut().get_mut::<Transform3d>(h).unwrap().translation = Vec3::new(500.0, STAND + 1.0, 0.0);
    steps(&mut app, 1);
    assert!((pos(&app, h).x - 500.0).abs() < 0.5, "瞬移过去 {}", pos(&app, h));
    for _ in 0..20 {
        app.world_mut().get_mut::<Transform3d>(lift).unwrap().translation.x += 3.0;
        steps(&mut app, 1);
    }
    assert!((pos(&app, h).x - 500.0).abs() < 0.5, "不再跟着原来的平台走 {}", pos(&app, h));
}

#[test]
fn bad_velocity_is_reset_and_sensors_still_report() {
    let mut app = app();
    ground(&mut app);
    let coin = app.world_mut().spawn((Collider { sensor: true, ..Collider::ball(10.0) }, at(60.0, STAND, 0.0))).id();
    let h = hero(&mut app, 0.0, STAND + 1.0, 0.0, Character3d::default());
    steps(&mut app, 5);
    set_velocity(&mut app, h, Vec3::NAN);
    steps(&mut app, 1);
    assert!(velocity(&app, h).is_finite() && pos(&app, h).is_finite(), "NaN 的速度清零");
    let mut met = false;
    for _ in 0..40 {
        let v = velocity(&app, h);
        set_velocity(&mut app, h, Vec3::new(150.0, v.y, 0.0));
        steps(&mut app, 1);
        met |= app.world_mut().resource_mut::<Messages<Collision3d>>().drain().any(|c| c.started && [c.a, c.b] == [h, coin] || [c.b, c.a] == [h, coin]);
    }
    assert!(met, "走过 sensor 有碰撞事件");
    assert!(pos(&app, h).x > 80.0, "sensor 不挡路");
}

#[test]
fn parameters_follow_the_component() {
    let mut app = app();
    ground(&mut app);
    slope(&mut app, 40.0, 400.0);
    let h = hero(&mut app, -60.0, STAND + 1.0, 0.0, Character3d { max_slope: 30f32.to_radians(), ..Default::default() });
    let walk = |app: &mut nge_ecs::App| {
        for _ in 0..90 {
            let v = velocity(app, h);
            set_velocity(app, h, Vec3::new(150.0, v.y, 0.0));
            steps(app, 1);
        }
    };
    walk(&mut app);
    assert!(pos(&app, h).y < STAND + 25.0, "最多爬 30°，40° 上不去 {}", pos(&app, h));
    app.world_mut().get_mut::<Character3d>(h).unwrap().max_slope = 50f32.to_radians();
    walk(&mut app);
    assert!(pos(&app, h).y > STAND + 60.0, "放宽到 50° 就上去了 {}", pos(&app, h));
}

#[test]
fn slides_down_slopes_too_steep_to_stand_on() {
    for degrees in [55.0f32, 65.0] {
        let mut app = app();
        ground(&mut app);
        slope(&mut app, degrees, 600.0);
        // 落在坡中间：x = 200 处坡面高 200·tan
        let top = 200.0 * degrees.to_radians().tan();
        let h = hero(&mut app, 200.0, top + 80.0, 0.0, Character3d::default());
        let mut stood = 0;
        for _ in 0..90 {
            steps(&mut app, 1);
            let s = state(&app, h);
            if s.grounded && pos(&app, h).y > STAND + 30.0 {
                stood += 1;
            }
            assert!(!s.grounded || s.ground_normal.y >= 50f32.to_radians().cos() - 1e-3, "{degrees}°：着地时法线要够平 {s:?}");
        }
        assert_eq!(stood, 0, "{degrees}° 的坡上不算着地");
        steps(&mut app, 180);
        let p = pos(&app, h);
        assert!(p.y < STAND + 20.0 && p.x < 60.0, "{degrees}° 的坡上站不住，滑到坡底 {p}");
        let s = state(&app, h);
        assert!(s.grounded && s.ground_normal.y > 0.99, "坡底的平地上才着地 {s:?}");
    }
}

#[test]
fn turntables_carry_the_character_along_an_arc() {
    for turn in [false, true] {
        let mut app = app();
        let table = app.world_mut().spawn((RigidBody::Kinematic, Collider::cuboid(Vec3::new(400.0, 20.0, 400.0)), at(0.0, 90.0, 0.0))).id();
        let h = hero(&mut app, 150.0, 100.0 + STAND + 1.0, 0.0, Character3d { turn_with_ground: turn, ..Default::default() });
        steps(&mut app, 20);
        assert!(state(&app, h).grounded, "先站稳");
        for i in 1..=45 {
            app.world_mut().get_mut::<Transform3d>(table).unwrap().rotation = Quat::from_rotation_y((2.0 * i as f32).to_radians());
            steps(&mut app, 1);
        }
        // 绕 y 转 90°：(150, _, 0) 转到 (0, _, -150)
        let p = pos(&app, h);
        assert!(Vec3::new(p.x, 0.0, p.z).distance(Vec3::new(0.0, 0.0, -150.0)) < 5.0, "跟着转盘沿圆弧走 {p}");
        assert!(state(&app, h).grounded, "一直站在转盘上");
        let facing = app.world().get::<Transform3d>(h).unwrap().rotation;
        let angle = facing.angle_between(Quat::from_rotation_y(90f32.to_radians()));
        if turn {
            assert!(angle < 0.05, "turn_with_ground：朝向跟着转 {facing:?}");
        } else {
            assert!(facing.angle_between(Quat::IDENTITY) < 1e-3, "默认朝向不跟着转 {facing:?}");
        }
    }
}

#[test]
fn spinning_dynamic_bodies_carry_the_character() {
    let mut app = app();
    ground(&mut app);
    let disc = app.world_mut().spawn((RigidBody::Dynamic, Collider { friction: 0.0, ..Collider::cuboid(Vec3::new(400.0, 20.0, 400.0)) }, at(0.0, 10.0, 0.0), Velocity::default())).id();
    let h = hero(&mut app, 150.0, 20.0 + STAND + 1.0, 0.0, Character3d::default());
    steps(&mut app, 20);
    assert!(state(&app, h).grounded, "先站稳");
    let spin = |app: &mut nge_ecs::App| app.world_mut().get_mut::<Velocity>(disc).unwrap().angular = Vec3::new(0.0, std::f32::consts::FRAC_PI_2, 0.0);
    // 每秒转 90°，转一秒
    for _ in 0..60 {
        spin(&mut app);
        steps(&mut app, 1);
    }
    let p = pos(&app, h);
    assert!(Vec3::new(p.x, 0.0, p.z).distance(Vec3::new(0.0, 0.0, -150.0)) < 10.0, "跟着转动的动态刚体走圆弧 {p}");
    let mut app = app_with_sliding_box();
    let (b, h) = (app.1, app.2);
    let before = (pos(&app.0, h), pos(&app.0, b));
    for _ in 0..40 {
        app.0.world_mut().get_mut::<Velocity>(b).unwrap().linear = Vec3::new(120.0, 0.0, 0.0);
        steps(&mut app.0, 1);
    }
    let (hero_moved, box_moved) = (pos(&app.0, h) - before.0, pos(&app.0, b) - before.1);
    assert!(box_moved.x > 50.0 && (hero_moved.x - box_moved.x).abs() < 3.0, "站在动态箱子上跟着它走 {hero_moved} / {box_moved}");
    let _ = disc;
}

/// 无摩擦地上的大箱子，角色站在上面：（app，箱子，角色）。
fn app_with_sliding_box() -> (nge_ecs::App, Entity, Entity) {
    let mut app = app();
    app.world_mut().spawn((RigidBody::Fixed, Collider { friction: 0.0, ..Collider::cuboid(Vec3::new(4000.0, 20.0, 4000.0)) }, at(0.0, -10.0, 0.0)));
    let b = app.world_mut().spawn((RigidBody::Dynamic, Collider { friction: 0.0, ..Collider::cuboid(Vec3::new(200.0, 40.0, 200.0)) }, at(0.0, 20.0, 0.0), Velocity::default())).id();
    let h = hero(&mut app, 0.0, 40.0 + STAND + 1.0, 0.0, Character3d::default());
    steps(&mut app, 20);
    assert!(state(&app, h).grounded, "先站稳");
    (app, b, h)
}

#[test]
fn default_snap_keeps_the_character_on_the_ground_downhill() {
    // 下 30° 的坡：每步往下 250/60·tan30° ≈ 2.4，不贴地就飞出去
    let mut app = app();
    ground(&mut app);
    slope(&mut app, 30.0, 600.0);
    let h = hero(&mut app, 400.0, 400.0 * 30f32.to_radians().tan() + STAND + 20.0, 0.0, Character3d::default());
    steps(&mut app, 60);
    assert!(state(&app, h).grounded, "先站在坡上");
    for i in 0..60 {
        let v = velocity(&app, h);
        set_velocity(&mut app, h, Vec3::new(-250.0, v.y, 0.0));
        steps(&mut app, 1);
        assert!(state(&app, h).grounded, "下坡第 {i} 帧还贴着坡 {}", pos(&app, h));
    }
}

#[test]
fn jumps_off_rising_and_sinking_platforms() {
    for dy in [3.0f32, -3.0] {
        let mut app = app();
        let (lift, h) = on_a_lift(&mut app);
        let lift_y = |app: &nge_ecs::App| pos(app, lift).y;
        // 平台先动起来，站在上面跟着走
        for _ in 0..10 {
            app.world_mut().get_mut::<Transform3d>(lift).unwrap().translation.y += dy;
            steps(&mut app, 1);
        }
        assert!(state(&app, h).grounded, "跟着平台走时站着");
        set_velocity(&mut app, h, Vec3::new(0.0, 400.0, 0.0));
        let takeoff = pos(&app, h).y;
        for i in 0..20 {
            app.world_mut().get_mut::<Transform3d>(lift).unwrap().translation.y += dy;
            steps(&mut app, 1);
            assert!(pos(&app, h).y - lift_y(&app) - 10.0 - STAND > 0.5 && !state(&app, h).grounded, "平台每帧动 {dy}：第 {i} 帧已经离开平台");
        }
        // 起跳速度是世界里的，不叠平台的：20 帧后高 400·(1/3) − 981·(1/3)²/2 ≈ 79（起跳那一步还跟平台走了 {dy}）
        let rise = pos(&app, h).y - takeoff;
        assert!((rise - 79.0).abs() < 6.0, "平台每帧动 {dy}：跳起来 {rise}");
        assert!(velocity(&app, h).y > 0.0 && velocity(&app, h).y < 400.0, "还在往上、受重力减速 {}", velocity(&app, h));
    }
}

#[test]
fn paused_frames_keep_the_state_and_long_frames_report_the_simulated_time() {
    let mut app = app();
    ground(&mut app);
    let h = hero(&mut app, 0.0, STAND + 1.0, 0.0, Character3d::default());
    steps(&mut app, 10);
    set_velocity(&mut app, h, Vec3::new(150.0, 0.0, 0.0));
    steps(&mut app, 1);
    let (before, p) = (state(&app, h), pos(&app, h));
    assert!(before.grounded && (before.velocity.x - 150.0).abs() < 1.0, "{before:?}");
    // 暂停（delta = 0）：不动，状态留着上一帧的
    app.tick(crate::tests::VIEWPORT, 0.0);
    assert_eq!(pos(&app, h), p, "暂停时不动");
    assert_eq!(state(&app, h), before, "暂停时状态不变");
    // 一帧 1 秒，只走 MAX_STEPS 步：速度按实际走的时间算，不是除以 1 秒
    app.tick(crate::tests::VIEWPORT, 1.0);
    let s = state(&app, h);
    assert!((s.velocity.x - 150.0).abs() < 1.0, "按走了的 {} 步算速度 {:?}", MAX_STEPS, s.velocity);
    assert!((pos(&app, h).x - p.x - 150.0 * STEP * MAX_STEPS as f32).abs() < 0.5, "只走了 MAX_STEPS 步 {}", pos(&app, h));
    assert!(s.grounded, "{s:?}");
}

#[test]
fn removing_the_controller_turns_it_back_into_a_plain_kinematic_body() {
    let mut app = app();
    ground(&mut app);
    let h = hero(&mut app, 0.0, 200.0, 0.0, Character3d::default());
    steps(&mut app, 5);
    let p = pos(&app, h);
    assert!(p.y < 200.0, "角色受重力往下掉 {p}");
    app.world_mut().entity_mut(h).remove::<Character3d>();
    steps(&mut app, 30);
    let q = pos(&app, h);
    // 普通 kinematic 刚体：位置只听游戏的，不受重力
    assert!((q.y - p.y).abs() < 0.5, "不再受重力 {q}");
    assert!(!app.world().resource::<Physics3d>().is_character(h), "物理里不再当角色");
    // 再加上又是角色
    app.world_mut().entity_mut(h).insert(Character3d::default());
    steps(&mut app, 120);
    assert!((pos(&app, h).y - STAND).abs() < 2.0 && state(&app, h).grounded, "又落到地上 {}", pos(&app, h));
}

#[test]
fn teleporting_clears_the_ground_state() {
    let mut app = app();
    ground(&mut app);
    let h = hero(&mut app, 0.0, STAND + 1.0, 0.0, Character3d::default());
    steps(&mut app, 10);
    assert!(state(&app, h).grounded);
    // 瞬移到半空：这一帧报的是落下去，不是还站着，速度也不算瞬移的那段
    app.world_mut().get_mut::<Transform3d>(h).unwrap().translation = Vec3::new(300.0, 400.0, 0.0);
    steps(&mut app, 1);
    let s = state(&app, h);
    assert!(!s.grounded && s.ground_normal == Vec3::ZERO, "瞬移到半空不算着地 {s:?}");
    assert!(s.velocity.length() < 50.0, "瞬移的位移不算速度 {:?}", s.velocity);
}

#[test]
fn controllers_on_non_kinematic_bodies_are_warned_once() {
    let mut app = app();
    let e = app.world_mut().spawn((RigidBody::Dynamic, Collider::capsule(20.0, 15.0), at(0.0, 100.0, 0.0), Velocity::default(), Character3d::default())).id();
    let k = hero(&mut app, 100.0, 100.0, 0.0, Character3d::default());
    steps(&mut app, 2);
    let physics = app.world().resource::<Physics3d>();
    assert!(!physics.is_character(e) && physics.warned(e), "动态刚体上的 Character3d 不起作用，warn 一次");
    assert!(physics.is_character(k) && !physics.warned(k));
}
