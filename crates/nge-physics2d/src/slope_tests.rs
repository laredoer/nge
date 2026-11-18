//! 斜坡碰撞体（瓦片斜坡用的 `Shape::Slope`）和陡坡的坡脚（见 docs/gameplay2d.md）。

use glam::Vec2;
use nge_core::Viewport;
use nge_ecs::App;
use nge_render2d::Transform2d;

use super::*;

const VIEWPORT: Viewport = Viewport { size: [100.0, 100.0], pixels: [100, 100] };
const T: f32 = 32.0;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(PhysicsPlugin { gravity: Vec2::new(0.0, -1000.0), pixels_per_meter: 100.0 });
    app
}

fn at(x: f32, y: f32) -> Transform2d {
    Transform2d { translation: Vec2::new(x, y), ..Default::default() }
}

fn step(app: &mut App) {
    app.tick(VIEWPORT, STEP);
}

fn pos(app: &App, e: Entity) -> Vec2 {
    app.world().get::<Transform2d>(e).unwrap().translation
}

/// 左下角在 (x, y) 的矩形。
fn rect(app: &mut App, x: f32, y: f32, w: f32, h: f32) {
    app.world_mut().spawn((Collider::rect(Vec2::new(w, h)), at(x + w / 2.0, y + h / 2.0)));
}

/// 左下角在 (x, y) 的一格斜坡：左边、右边的高（格的几分之几）。
fn slope(app: &mut App, x: f32, y: f32, left: f32, right: f32) {
    app.world_mut().spawn((Collider::slope(Vec2::splat(T), left, right), at(x + T / 2.0, y + T / 2.0)));
}

fn hero(app: &mut App, x: f32, y: f32) -> Entity {
    app.world_mut().spawn((RigidBody::Kinematic, Collider::rect(Vec2::splat(20.0)), at(x, y), Velocity::default(), Character::default())).id()
}

fn grounded(app: &App, e: Entity) -> bool {
    app.world().resource::<Physics>().contacts(e).iter().any(|c| c.normal.y > 0.5)
}

/// 每帧把水平速度设成 `vx` 走 `frames` 帧：（在坡附近每帧走了多远里最少的，没着地的帧数）。
fn walk(app: &mut App, h: Entity, vx: f32, frames: usize) -> (f32, usize) {
    let (mut least, mut airborne) = (f32::INFINITY, 0);
    for _ in 0..frames {
        let vy = app.world().get::<Velocity>(h).unwrap().linear.y;
        app.world_mut().get_mut::<Velocity>(h).unwrap().linear = Vec2::new(vx, vy);
        let before = pos(app, h).x;
        step(app);
        // 只看坡上和坡两头（rapier 在很宽的平地上偶尔也会顿一下，和坡无关）
        if (-2.0 * T..8.0 * T).contains(&before) {
            least = least.min((pos(app, h).x - before).abs());
        }
        airborne += usize::from(!grounded(app, h));
    }
    (least, airborne)
}

/// 平地（顶面 y = 0，x < 0）→ `tiles` 格斜坡（每格的左右高）→ 顶上的平地。斜坡下面按列填实心方块，和瓦片地图一样每格一块。
fn hill(app: &mut App, tiles: &[(f32, f32)]) -> f32 {
    rect(app, -400.0, -T, 400.0, T);
    let mut base = 0.0;
    for (i, &(left, right)) in tiles.iter().enumerate() {
        let x = i as f32 * T;
        // 这一格下面的实心方块（和地面同一层的那一块也在）
        rect(app, x, -T, T, T + base);
        slope(app, x, base, left, right);
        if right >= 1.0 {
            base += T;
        }
    }
    let end = tiles.len() as f32 * T;
    rect(app, end, -T, 400.0, T + base);
    base
}

#[test]
fn slope_shapes_are_tile_quads() {
    let mut app = app();
    let e = app.world_mut().spawn((Collider::slope(Vec2::splat(T), 0.0, 1.0), at(T / 2.0, T / 2.0))).id();
    step(&mut app);
    let physics = app.world().resource::<Physics>();
    let filter = QueryFilter::default();
    assert_eq!(physics.overlap_point(Vec2::new(28.0, 4.0), filter), [e], "右下角是实心的");
    assert!(physics.overlap_point(Vec2::new(4.0, 28.0), filter).is_empty(), "左上角是空的");
    let hit = physics.raycast(Vec2::new(16.0, 100.0), Vec2::NEG_Y, 200.0, filter).unwrap();
    assert!((hit.point.y - 16.0).abs() < 0.01 && (hit.normal - Vec2::new(-1.0, 1.0).normalize()).length() < 0.01, "{hit:?}");
    // 两边都是 0 的生不出来：没有碰撞体，不 panic
    let flat = app.world_mut().spawn((Collider::slope(Vec2::splat(T), 0.0, 0.0), at(200.0, 0.0))).id();
    step(&mut app);
    assert!(app.world().resource::<Physics>().overlap_point(Vec2::new(200.0, -10.0), filter).iter().all(|&x| x != flat));
}

#[test]
fn walks_up_and_down_45_degree_tile_slopes_without_catching() {
    let mut app = app();
    let top = hill(&mut app, &[(0.0, 1.0), (0.0, 1.0), (0.0, 1.0)]);
    let h = hero(&mut app, -100.0, 12.0);
    for _ in 0..10 {
        step(&mut app);
    }
    // 爬 45° 的坡时水平方向慢一半（rapier 把水平的位移投到坡上），每帧至少走 1
    let (least, airborne) = walk(&mut app, h, 150.0, 150);
    let p = pos(&app, h);
    assert!(p.x > 3.0 * T + 20.0 && (p.y - (top + 10.0)).abs() < 1.5, "走到坡顶的平地上 {p}");
    assert!(least > 1.0, "一路没卡住（每帧至少走 1）{least}");
    assert!(airborne <= 1, "一路贴着 {airborne}");
    let (least, airborne) = walk(&mut app, h, -150.0, 150);
    let p = pos(&app, h);
    assert!(p.x < -20.0 && (p.y - 10.0).abs() < 1.5, "走回坡底 {p}");
    assert!(least > 1.0, "下坡没卡住 {least}");
    assert!(airborne <= 2, "下坡贴着 {airborne}");
}

#[test]
fn walks_up_and_down_gentle_tile_slopes_without_catching() {
    // 两格升一格：一格的左右高是 0 → 0.5，下一格 0.5 → 1
    let mut app = app();
    let top = hill(&mut app, &[(0.0, 0.5), (0.5, 1.0), (0.0, 0.5), (0.5, 1.0)]);
    assert_eq!(top, 2.0 * T);
    let h = hero(&mut app, -100.0, 12.0);
    for _ in 0..10 {
        step(&mut app);
    }
    let (least, airborne) = walk(&mut app, h, 150.0, 150);
    let p = pos(&app, h);
    assert!(p.x > 4.0 * T + 20.0 && (p.y - (top + 10.0)).abs() < 1.5, "走到坡顶 {p}");
    assert!(least > 1.0 && airborne <= 1, "{least} {airborne}");
    let (least, airborne) = walk(&mut app, h, -150.0, 150);
    assert!(pos(&app, h).x < -20.0 && least > 1.0 && airborne <= 2, "{} {least} {airborne}", pos(&app, h));
}

#[test]
fn steep_ramps_flush_with_the_ground_are_climbed_from_the_foot() {
    // 40° … 49° 的转过角度的矩形坡，坡脚的尖角正好贴着地面，地面在坡脚处断开（像瓦片地图合并出来的两块）：
    // docs/character.md 记过陡坡的坡脚会被当成墙，这里平台跳跃那么大、那么快的角色都要从坡脚走上去
    for deg in [40.0f32, 43.0, 46.0, 49.0] {
        for (speed, size) in [(260.0, Vec2::new(24.0, 32.0)), (100.0, Vec2::new(24.0, 32.0)), (150.0, Vec2::splat(20.0))] {
            let mut app = app();
            rect(&mut app, -400.0, -20.0, 400.0, 20.0);
            rect(&mut app, 0.0, -20.0, 600.0, 20.0);
            let angle = deg.to_radians();
            let length = 300.0;
            let center = Vec2::new(angle.cos(), angle.sin()) * length / 2.0 + Vec2::new(angle.sin(), -angle.cos()) * 10.0;
            app.world_mut().spawn((Collider::rect(Vec2::new(length, 20.0)), Transform2d { translation: center, rotation: angle, ..Default::default() }));
            let h = app.world_mut().spawn((RigidBody::Kinematic, Collider::rect(size), at(-60.0, size.y / 2.0 + 1.0), Velocity::default(), Character::default())).id();
            for _ in 0..10 {
                step(&mut app);
            }
            walk(&mut app, h, speed, 90);
            let p = pos(&app, h);
            assert!(p.y > 40.0, "{deg}° 速度 {speed}：爬上去了 {p}");
        }
    }
}
