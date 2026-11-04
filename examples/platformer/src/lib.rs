//! 平台跳跃：关卡（瓦片地图）、金币、终点和相机都在 `App.tsx` 里声明；
//! Rust 只管每帧的角色控制：读键盘、看脚下有没有地、设速度，掉下去了送回起点。

use bevy_ecs::prelude::*;
use nge_assets::AssetPlugin;
use nge_audio::AudioPlugin;
use nge_core::EventKey;
use nge_ecs::glam::Vec2;
use nge_ecs::{App, Input, Outbox, Time, Update};
use nge_physics2d::{Physics, Velocity};
use nge_render2d::{Render2dPlugin, Transform2d};
use nge_render3d::Render3dPlugin;
use nge_scene::{SceneId, ScenePlugin};
use nsh_core::prelude::assets;
use serde::Serialize;

/// 角色精灵的 id（`App.tsx` 里声明）。
pub const PLAYER: &str = "player";
/// 掉出关卡、被送回起点时发给 TSX。
pub const RESPAWN: EventKey<Respawn> = EventKey::new("respawn");

#[derive(Debug, Clone, Serialize)]
pub struct Respawn {
    pub falls: u32,
}

/// 水平速度上限（point / s）和加速度：地上转向快，空中慢一点。
const SPEED: f32 = 260.0;
const GROUND_ACCEL: f32 = 3000.0;
const AIR_ACCEL: f32 = 1500.0;
/// 起跳速度；场景里重力是 1800，跳起来大约 114 point 高（640² / 3600）。
const JUMP: f32 = 640.0;
/// 走出平台边以后这么久之内还能跳（土狼时间）。
const COYOTE: f32 = 0.1;
/// 落地前这么久之内按的跳也算。
const BUFFER: f32 = 0.12;
/// 脚下的接触法线 y 至少这么大才算站在地上（墙的法线是水平的）。
const GROUND_NORMAL: f32 = 0.7;
/// 掉到这条线以下就送回起点。
const FALL_LINE: f32 = -400.0;
const LEFT: [&str; 4] = ["ArrowLeft", "KeyA", "GamepadLeft", "GamepadLeftStickLeft"];
const RIGHT: [&str; 4] = ["ArrowRight", "KeyD", "GamepadRight", "GamepadLeftStickRight"];
const JUMP_KEYS: [&str; 4] = ["Space", "ArrowUp", "KeyW", "GamepadA"];

#[derive(Resource, Debug)]
struct Controller {
    /// 离开地面多久了。
    airborne: f32,
    /// 按跳多久了（还没用掉的）。
    jump_pressed: f32,
    /// 第一次见到角色时的位置。
    spawn: Option<Vec2>,
    falls: u32,
}

impl Default for Controller {
    fn default() -> Self {
        Self { airborne: f32::INFINITY, jump_pressed: f32::INFINITY, spawn: None, falls: 0 }
    }
}

fn move_toward(from: f32, to: f32, step: f32) -> f32 {
    if (to - from).abs() <= step { to } else { from + step * (to - from).signum() }
}

fn control(
    time: Res<Time>,
    input: Res<Input>,
    physics: Res<Physics>,
    mut ctl: ResMut<Controller>,
    mut players: Query<(Entity, &SceneId, &mut Transform2d, &mut Velocity)>,
    mut outbox: ResMut<Outbox>,
) {
    let Some((entity, _, mut t, mut v)) = players.iter_mut().find(|(_, id, ..)| id.0 == PLAYER) else { return };
    let spawn = *ctl.spawn.get_or_insert(t.translation);
    let dt = time.delta;
    // 暂停（0）或者宿主给了坏的帧时间：不动，免得暂停中起跳、NaN 写进物理
    if !(dt > 0.0) {
        return;
    }
    // 脚下的东西（法线朝上）；移动平台有速度，地图的碰撞体没有。同时踩着地面和平台时跟着动得快的那个
    let ground = physics
        .contacts(entity)
        .iter()
        .filter(|c| c.normal.y > GROUND_NORMAL)
        .map(|c| physics.velocity(c.other).map_or(Vec2::ZERO, |g| g.linear))
        .max_by(|a, b| a.length_squared().total_cmp(&b.length_squared()));
    // 刚起跳的下一帧可能还有地面的预测接触点：比脚下往上走得快就不算着地，免得连跳两次（上升的平台上照样能跳）
    let grounded = ground.is_some_and(|g| v.linear.y - g.y <= 1.0);
    ctl.airborne = if grounded { 0.0 } else { ctl.airborne + dt };
    ctl.jump_pressed = if JUMP_KEYS.iter().any(|k| input.key_pressed(k)) { 0.0 } else { ctl.jump_pressed + dt };

    let accel = if grounded { GROUND_ACCEL } else { AIR_ACCEL };
    let mut velocity = v.linear;
    // 站在横移的平台上：跟着平台走（关卡没有摩擦，不加的话平台会从脚底下滑走）
    let carry = ground.filter(|_| grounded).map_or(0.0, |g| g.x);
    velocity.x = move_toward(velocity.x, input.axis(&LEFT, &RIGHT) * SPEED + carry, accel * dt);
    let holding = JUMP_KEYS.iter().any(|k| input.key_down(k));
    if ctl.jump_pressed <= BUFFER && ctl.airborne <= COYOTE {
        // 缓冲里的跳到落地时键已经松开了：直接按短跳
        velocity.y = if holding { JUMP } else { JUMP * 0.5 };
        ctl.jump_pressed = f32::INFINITY;
        ctl.airborne = f32::INFINITY;
    } else if velocity.y > 0.0 && !holding && JUMP_KEYS.iter().any(|k| input.key_released(k)) {
        // 上升时跳跃键全松开了：跳得矮
        velocity.y *= 0.5;
    }
    if velocity != v.linear {
        v.linear = velocity;
    }

    if t.translation.y < FALL_LINE {
        t.translation = spawn;
        *v = Velocity::default();
        ctl.falls += 1;
        outbox.send(RESPAWN, &Respawn { falls: ctl.falls });
    }
}

pub fn app() -> App {
    build(AudioPlugin::default())
}

fn build(audio: AudioPlugin) -> App {
    let mut app = App::new();
    app.add_plugins(AssetPlugin::new().files(assets!().iter().map(|f| (f.uri, f.source))))
        .add_plugins(audio)
        .add_plugins(Render3dPlugin)
        .add_plugins(Render2dPlugin)
        .add_plugins(ScenePlugin::new())
        .add_event(RESPAWN)
        .init_resource::<Controller>()
        .add_systems(Update, control);
    app
}

pub fn factory() -> nge_core::GameFactory {
    nge_ecs::factory(app)
}

#[cfg(test)]
mod tests {
    use nge_core::Viewport;
    use serde_json::{Value, json};

    use super::*;

    const VIEWPORT: Viewport = Viewport { size: [800.0, 600.0], pixels: [800, 600] };

    fn app() -> App {
        build(AudioPlugin::mock())
    }

    /// 一行 32 宽的地面（顶边 y = 0），从 `from` 到 `to` 列是坑。
    fn level(pit: Option<(usize, usize)>) -> Value {
        let data: Vec<i32> = (0..40).map(|c| if pit.is_some_and(|(a, b)| (a..=b).contains(&c)) { -1 } else { 1 }).collect();
        json!({
            "physics": { "gravity": [0, -1800] },
            "tilemaps": [{ "id": "level", "texture": "asset:/@platformer/tiles.png", "grid": [3, 1], "tile": 32, "cols": 40, "x": -640, "y": 0, "data": data, "solid": [1], "friction": 0 }],
            "sprites": [{ "id": PLAYER, "w": 24, "h": 32, "y": 16, "body": { "fixedRotation": true, "friction": 0 } }],
        })
    }

    fn player(app: &mut App) -> (Vec2, Vec2) {
        let world = app.world_mut();
        let (_, t, v) = world.query::<(&SceneId, &Transform2d, &Velocity)>().iter(world).find(|(id, ..)| id.0 == PLAYER).unwrap();
        (t.translation, v.linear)
    }

    fn run(app: &mut App, frames: usize) -> Vec<(String, Value)> {
        (0..frames).flat_map(|_| app.tick(VIEWPORT, 1.0 / 60.0)).collect()
    }

    fn key(app: &mut App, code: &str, down: bool) {
        app.command("key", &json!({ "code": code, "down": down }));
    }

    #[test]
    fn walks_with_the_arrows_and_stops() {
        let mut app = app();
        app.prop("scene", Some(&level(None)));
        run(&mut app, 20);
        key(&mut app, "ArrowRight", true);
        run(&mut app, 30);
        let (p, v) = player(&mut app);
        assert!(p.x > 80.0 && (v.x - SPEED).abs() < 1.0, "半秒跑到全速 {p} {v}");
        key(&mut app, "ArrowRight", false);
        run(&mut app, 10);
        assert!(player(&mut app).1.x.abs() < 1.0, "松开就停");
        key(&mut app, "KeyA", true);
        run(&mut app, 10);
        assert!(player(&mut app).1.x < 0.0, "A 往左");
    }

    #[test]
    fn jump_is_cut_only_when_every_jump_key_is_released() {
        let mut app = app();
        app.prop("scene", Some(&level(None)));
        run(&mut app, 20);
        let ground = player(&mut app).0.y;
        key(&mut app, "Space", true);
        key(&mut app, "ArrowUp", true);
        run(&mut app, 2);
        key(&mut app, "Space", false);
        run(&mut app, 1);
        let held = player(&mut app).1.y;
        assert!(held > 500.0, "还按着 ↑，不截短 {held}");
        key(&mut app, "ArrowUp", false);
        run(&mut app, 1);
        let cut = player(&mut app).1.y;
        assert!(cut < held * 0.6, "全松开才截短 {held} → {cut}");
        assert!(player(&mut app).0.y > ground);
    }

    #[test]
    fn plays_with_a_gamepad() {
        let mut app = app();
        app.prop("scene", Some(&level(None)));
        run(&mut app, 20);
        let (start, ground) = { let p = player(&mut app).0; (p.x, p.y) };
        key(&mut app, "GamepadLeftStickRight", true);
        run(&mut app, 30);
        assert!(player(&mut app).0.x > start + 50.0, "左摇杆往右跑");
        key(&mut app, "GamepadLeftStickRight", false);
        key(&mut app, "GamepadLeft", true);
        run(&mut app, 60);
        assert!(player(&mut app).1.x < -200.0, "十字键往左跑");
        key(&mut app, "GamepadLeft", false);
        run(&mut app, 30);
        key(&mut app, "GamepadA", true);
        run(&mut app, 10);
        assert!(player(&mut app).0.y > ground + 50.0, "A 跳");
    }

    /// 地面上方一块 kinematic 平台（顶边 y = 100），角色站在上面；返回平台实体。
    fn on_a_lift(app: &mut App) -> Entity {
        let mut scene = level(None);
        scene["sprites"][0]["y"] = json!(116);
        scene["sprites"].as_array_mut().unwrap().push(json!({ "id": "lift", "w": 96, "h": 16, "y": 92, "body": { "kind": "kinematic", "friction": 0 } }));
        app.prop("scene", Some(&scene));
        run(app, 30);
        let world = app.world_mut();
        world.query::<(Entity, &SceneId)>().iter(world).find(|(_, id)| id.0 == "lift").unwrap().0
    }

    /// 每帧把平台挪 `step`（和补间一样写绝对位置），走 `frames` 帧。
    fn move_lift(app: &mut App, lift: Entity, step: Vec2, frames: usize) {
        for _ in 0..frames {
            app.world_mut().get_mut::<Transform2d>(lift).unwrap().translation += step;
            run(app, 1);
        }
    }

    #[test]
    fn rides_a_sideways_platform() {
        let mut app = app();
        let lift = on_a_lift(&mut app);
        let start = player(&mut app).0.x;
        move_lift(&mut app, lift, Vec2::new(100.0 / 60.0, 0.0), 60);
        let moved = player(&mut app).0.x - start;
        assert!((moved - 100.0).abs() < 10.0, "跟着平台走，不让平台从脚底下滑走：{moved}");
    }

    #[test]
    fn jumps_from_a_rising_platform() {
        let mut app = app();
        let lift = on_a_lift(&mut app);
        move_lift(&mut app, lift, Vec2::new(0.0, 1.0), 20);
        key(&mut app, "Space", true);
        move_lift(&mut app, lift, Vec2::new(0.0, 1.0), 15);
        let (top, lift_top) = (player(&mut app).0.y - 16.0, app.world().get::<Transform2d>(lift).unwrap().translation.y + 8.0);
        assert!(top - lift_top > 50.0, "上升的平台上也能起跳：比平台高 {}", top - lift_top);
    }

    #[test]
    fn jumps_only_from_the_ground() {
        let mut app = app();
        app.prop("scene", Some(&level(None)));
        run(&mut app, 20);
        let ground = player(&mut app).0.y;
        key(&mut app, "Space", true);
        run(&mut app, 1);
        key(&mut app, "Space", false);
        run(&mut app, 1);
        key(&mut app, "Space", true);
        let mut top = ground;
        for _ in 0..20 {
            run(&mut app, 1);
            top = top.max(player(&mut app).0.y);
        }
        key(&mut app, "Space", false);
        // 第一跳松开后截短了（≈ 36），第二下在空中按的不算：远低于整跳（≈ 640² / 3600 ≈ 114）
        assert!(top - ground > 20.0 && top - ground < 60.0, "跳了、截短了、没在空中再跳 {}", top - ground);
        run(&mut app, 60);
        assert!((player(&mut app).0.y - ground).abs() < 1.0, "落回地面");

        // 一直按着的完整一跳
        key(&mut app, "ArrowUp", true);
        let mut top = ground;
        for _ in 0..40 {
            run(&mut app, 1);
            top = top.max(player(&mut app).0.y);
        }
        assert!((top - ground - 114.0).abs() < 8.0, "整跳高度 {}", top - ground);
    }

    #[test]
    fn jump_pressed_just_before_landing_still_counts() {
        let mut app = app();
        let mut scene = level(None);
        scene["sprites"][0]["y"] = json!(60);
        app.prop("scene", Some(&scene));
        // 从 60 掉下来（脚离地 44）大约要 13 帧；第 8 帧按跳
        run(&mut app, 8);
        key(&mut app, "Space", true);
        run(&mut app, 12);
        assert!(player(&mut app).1.y > 0.0, "落地就起跳 {:?}", player(&mut app));
    }

    #[test]
    fn falling_into_a_pit_respawns() {
        let mut app = app();
        app.prop("scene", Some(&level(Some((22, 24)))));
        run(&mut app, 20);
        let start = player(&mut app).0;
        key(&mut app, "ArrowRight", true);
        let respawn = (0..120).find_map(|_| run(&mut app, 1).into_iter().find(|(n, _)| n == "respawn")).expect("掉进坑里");
        assert_eq!(respawn.1, json!({ "falls": 1 }));
        let (p, v) = player(&mut app);
        assert!((p - start).length() < 0.1 && v.length() < 1e-3, "送回起点、速度清零 {p} {v}");
    }

    /// App.tsx 的金币是 sensor、角色开了 collide：走过去时发 collide，金币不挡路。
    #[test]
    fn walking_through_a_coin_reports_it() {
        let mut app = app();
        let mut scene = level(None);
        scene["sprites"][0]["body"]["collide"] = json!(true);
        scene["sprites"].as_array_mut().unwrap().push(json!({ "id": "coin-0", "x": 100, "y": 28, "w": 14, "body": { "kind": "fixed", "sensor": true } }));
        app.prop("scene", Some(&scene));
        run(&mut app, 20);
        key(&mut app, "ArrowRight", true);
        let events = run(&mut app, 40);
        assert!(events.iter().any(|(n, d)| n == "collide" && d["started"] == true && (d["a"] == "coin-0" || d["b"] == "coin-0")), "碰到金币");
        assert!(player(&mut app).0.x > 120.0, "穿过去了");
    }

    #[test]
    fn coyote_time_allows_a_late_jump_but_not_a_very_late_one() {
        for (late, jumps) in [(3, true), (12, false)] {
            let mut app = app();
            app.prop("scene", Some(&level(Some((22, 39)))));
            run(&mut app, 20);
            key(&mut app, "ArrowRight", true);
            // 跑到坑边（第 22 列左边 x = 64）后再等几帧
            let mut frames = 0;
            while player(&mut app).0.x - 12.0 < 64.0 && frames < 200 {
                run(&mut app, 1);
                frames += 1;
            }
            run(&mut app, late);
            key(&mut app, "Space", true);
            run(&mut app, 1);
            assert_eq!(player(&mut app).1.y > 0.0, jumps, "离开平台 {late} 帧后按跳 {:?}", player(&mut app));
        }
    }

    #[test]
    fn cannot_jump_off_a_wall() {
        let mut app = app();
        let mut scene = level(None);
        // 一面墙，角色悬在半空贴着它
        scene["sprites"].as_array_mut().unwrap().push(json!({ "id": "wall", "x": 40, "y": 200, "w": 20, "h": 400, "body": { "kind": "fixed", "friction": 0 } }));
        scene["sprites"][0]["x"] = json!(18);
        scene["sprites"][0]["y"] = json!(200);
        app.prop("scene", Some(&scene));
        key(&mut app, "ArrowRight", true);
        run(&mut app, 5);
        key(&mut app, "Space", true);
        run(&mut app, 2);
        assert!(player(&mut app).1.y < 0.0, "贴着墙也在往下掉 {:?}", player(&mut app));
    }

    #[test]
    fn paused_game_ignores_input() {
        let mut app = app();
        app.prop("scene", Some(&level(None)));
        run(&mut app, 20);
        let mut scene = level(None);
        scene["timeScale"] = json!(0);
        app.prop("scene", Some(&scene));
        key(&mut app, "Space", true);
        run(&mut app, 5);
        app.prop("scene", Some(&level(None)));
        run(&mut app, 1);
        assert!(player(&mut app).1.y.abs() < 1.0, "暂停中按的跳不留到继续后 {:?}", player(&mut app));
    }
}
