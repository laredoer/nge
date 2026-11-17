//! 平台跳跃：关卡（瓦片地图）、金币、终点和相机都在 `game.tsx` 里声明；
//! Rust 只管每帧的角色控制：问动作（`move`、`jump`、`restart`，绑定见 controls.json 和 docs/input-actions.md）、看脚下有没有地、设速度，
//! 踩敌人、被敌人撞或掉下去了送回起点，重新开始；起跳、踩敌人和掉坑的音效、落地和受伤时手柄的震动也在这里放。
//! 第二个手柄接入时 TSX 往场景里放第二个角色（`player2`），这里让它能跑能跳。

mod debug;

use std::collections::HashSet;

use bevy_ecs::prelude::*;
use nge_assets::{AssetPlugin, AssetServer, Assets, Handle};
use nge_audio::{Audio, AudioPlugin, Play, Sound};
use nge_core::EventKey;
use nge_ecs::glam::Vec2;
use nge_ecs::{ActionMap, Actions, App, Input, Outbox, Rumble, Startup, Time, Update};
use nge_physics2d::{Contact, Physics, Velocity};
use nge_render2d::{Render2dPlugin, Transform2d};
use nge_render3d::Render3dPlugin;
use nge_scene::{LevelStart, SceneId, ScenePlugin};
use nsh_core::prelude::assets;
use serde::Serialize;

/// 角色精灵的 id（`game.tsx` 里声明）。
pub const PLAYER: &str = "player";
/// 第二个角色的精灵 id（第二个手柄接入时 `game.tsx` 放进场景）。
pub const GUEST: &str = "player2";
/// 掉出关卡、被送回起点时发给 TSX。
pub const RESPAWN: EventKey<Respawn> = EventKey::new("respawn");

/// 踩到敌人时发给 TSX（TSX 把它去掉刚体、藏起来）。
pub const STOMP: EventKey<Stomp> = EventKey::new("stomp");
/// id 以它开头的精灵是敌人。
pub const ENEMY: &str = "enemy-";
/// 按 R 重新开始时发给 TSX（角色已经回到起点、掉坑次数清零）。
pub const RESTART: EventKey<Restart> = EventKey::new("restart");

#[derive(Debug, Clone, Serialize)]
pub struct Respawn {
    pub falls: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Restart {}

#[derive(Debug, Clone, Serialize)]
pub struct Stomp {
    pub id: String,
    /// 踩的时候角色在哪（飘字用；敌人在巡逻，TSX 不知道它现在的位置）。
    pub x: f32,
    pub y: f32,
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
/// 动作名（绑定在 controls.json 里，TSX 的改键面板能改）。
const MOVE: &str = "move";
const JUMP_ACTION: &str = "jump";
const RESTART_ACTION: &str = "restart";
/// 在空中超过这么久，落地时手柄轻震一下。
const LAND_RUMBLE_AIR: f32 = 0.2;
const PERF_KEY: &str = "KeyP";

/// 默认绑定：和 TSX 读的是同一份文件，TSX 的 `actions` 到之前（和测试里）也能玩。
fn default_controls() -> ActionMap {
    ActionMap::from_json(include_str!("../controls.json")).unwrap_or_else(|e| {
        log::error!("controls.json: {e}");
        ActionMap::default()
    })
}
const JUMP_SOUND: &str = "asset:/@platformer/jump.wav";
const FALL_SOUND: &str = "asset:/@platformer/fall.wav";
const STOMP_SOUND: &str = "asset:/@platformer/stomp.wav";

/// 起跳、掉坑的音效；没有音频设备时是空的。
#[derive(Resource, Default)]
struct Sfx {
    jump: Option<Handle<Sound>>,
    fall: Option<Handle<Sound>>,
    stomp: Option<Handle<Sound>>,
}

/// 启动时就加载，第一次起跳不用等解码。
fn load_sfx(audio: Res<Audio>, mut server: ResMut<AssetServer>, mut sounds: ResMut<Assets<Sound>>, mut sfx: ResMut<Sfx>) {
    if audio.enabled() {
        let mut load = |uri| Some(server.load(&mut sounds, uri));
        *sfx = Sfx { jump: load(JUMP_SOUND), fall: load(FALL_SOUND), stomp: load(STOMP_SOUND) };
    }
}

fn play(audio: &mut Audio, sound: &Option<Handle<Sound>>, volume: f32) {
    if let Some(sound) = sound {
        audio.play(*sound, Play { volume, ..Play::default() });
    }
}

#[derive(Resource, Debug)]
struct Controller {
    /// 离开地面多久了。
    airborne: f32,
    /// 按跳多久了（还没用掉的）。
    jump_pressed: f32,
    /// 这一次腾空多久了（落地震动用；`airborne` 起跳时会被设成无穷大）。
    air_time: f32,
    /// 第一次见到角色时的位置。
    spawn: Option<Vec2>,
    falls: u32,
    /// 踩过的敌人：TSX 去掉它的刚体之前不再算。
    stomped: HashSet<Entity>,
}

impl Default for Controller {
    fn default() -> Self {
        Self { airborne: f32::INFINITY, jump_pressed: f32::INFINITY, air_time: 0.0, spawn: None, falls: 0, stomped: HashSet::new() }
    }
}

fn move_toward(from: f32, to: f32, step: f32) -> f32 {
    if (to - from).abs() <= step { to } else { from + step * (to - from).signum() }
}

#[allow(clippy::too_many_arguments)]
fn control(
    time: Res<Time>,
    actions: Res<Actions>,
    physics: Res<Physics>,
    mut ctl: ResMut<Controller>,
    mut players: Query<(Entity, &SceneId, &mut Transform2d, &mut Velocity)>,
    names: Query<&SceneId>,
    mut outbox: ResMut<Outbox>,
    mut audio: ResMut<Audio>,
    sfx: Res<Sfx>,
    mut rumble: ResMut<Rumble>,
) {
    let Some((entity, _, mut t, mut v)) = players.iter_mut().find(|(_, id, ..)| id.0 == PLAYER) else { return };
    let spawn = *ctl.spawn.get_or_insert(t.translation);
    let dt = time.delta;
    // 暂停（0）或者宿主给了坏的帧时间：不动，免得暂停中起跳、NaN 写进物理
    if !(dt > 0.0) {
        return;
    }
    // 和起跳同一帧时重来优先，这一下跳不算
    if actions.pressed(RESTART_ACTION) {
        t.translation = spawn;
        *v = Velocity::default();
        *ctl = Controller { spawn: Some(spawn), ..Controller::default() };
        outbox.send(RESTART, &Restart {});
        return;
    }
    let contacts = physics.contacts(entity);
    let is_enemy = |other: Entity| names.get(other).is_ok_and(|id| id.0.starts_with(ENEMY));
    // 碰到的敌人：从上面落下来是踩到，别的方向是被撞
    let (stomps, hits): (Vec<&Contact>, Vec<&Contact>) =
        contacts.iter().filter(|c| is_enemy(c.other) && !ctl.stomped.contains(&c.other)).partition(|c| c.normal.y > GROUND_NORMAL);
    // 脚下有东西（法线朝上）。角色控制器的接触是走完这一帧之后贴着的，起跳后的下一帧已经离开地面
    let grounded = contacts.iter().any(|c| c.normal.y > GROUND_NORMAL);
    // 腾空够久再落地：手柄轻震一下
    if grounded && ctl.air_time > LAND_RUMBLE_AIR {
        rumble.play(0, 0.25, 0.5, 0.08);
    }
    ctl.air_time = if grounded { 0.0 } else { ctl.air_time + dt };
    ctl.airborne = if grounded { 0.0 } else { ctl.airborne + dt };
    ctl.jump_pressed = if actions.pressed(JUMP_ACTION) { 0.0 } else { ctl.jump_pressed + dt };

    let accel = if grounded { GROUND_ACCEL } else { AIR_ACCEL };
    let mut velocity = v.linear;
    velocity.x = move_toward(velocity.x, actions.value(MOVE) * SPEED, accel * dt);
    let holding = actions.down(JUMP_ACTION);
    if ctl.jump_pressed <= BUFFER && ctl.airborne <= COYOTE {
        // 缓冲里的跳到落地时键已经松开了：直接按短跳
        velocity.y = if holding { JUMP } else { JUMP * 0.5 };
        ctl.jump_pressed = f32::INFINITY;
        ctl.airborne = f32::INFINITY;
        if stomps.is_empty() {
            play(&mut audio, &sfx.jump, 0.5);
        }
    } else if velocity.y > 0.0 && actions.released(JUMP_ACTION) {
        // 上升时跳跃键全松开了：跳得矮
        velocity.y *= 0.5;
    }
    if !stomps.is_empty() {
        velocity.y = if holding { JUMP } else { JUMP * 0.5 };
        play(&mut audio, &sfx.stomp, 0.6);
    }
    for c in &stomps {
        ctl.stomped.insert(c.other);
        if let Ok(id) = names.get(c.other) {
            outbox.send(STOMP, &Stomp { id: id.0.clone(), x: t.translation.x, y: t.translation.y });
        }
    }
    if velocity != v.linear {
        v.linear = velocity;
    }

    if t.translation.y < FALL_LINE || !hits.is_empty() {
        t.translation = spawn;
        *v = Velocity::default();
        // 掉坑前按的跳不留到起点；踩过的敌人留着（只有 R 才让它们回来）
        let stomped = std::mem::take(&mut ctl.stomped);
        *ctl = Controller { spawn: Some(spawn), falls: ctl.falls + 1, stomped, ..Controller::default() };
        outbox.send(RESPAWN, &Respawn { falls: ctl.falls });
        play(&mut audio, &sfx.fall, 0.6);
        // 被撞、掉下去：重震一下
        rumble.play(0, 1.0, 0.5, 0.3);
    }
}

/// 第二个玩家（见 docs/input-actions.md）：只会跑、跳；一出现就挪到第一个角色身边，掉下去也回到他身边。不踩敌人、不算失手。
fn control_guest(
    time: Res<Time>,
    actions: Res<Actions>,
    physics: Res<Physics>,
    mut placed: Local<Option<Entity>>,
    mut players: Query<(Entity, &SceneId, &mut Transform2d, &mut Velocity)>,
) {
    let host = players.iter().find(|(_, id, ..)| id.0 == PLAYER).map(|(_, _, t, _)| t.translation);
    let Some((entity, _, mut t, mut v)) = players.iter_mut().find(|(_, id, ..)| id.0 == GUEST) else {
        *placed = None;
        return;
    };
    // 刚出现（手柄接上、重连；换关时实体是重建的，也算新出现）或者掉下去了：到第一个角色现在的位置（两个角色同一个碰撞分组，叠着也不会挤开）
    if (*placed != Some(entity) || t.translation.y < FALL_LINE)
        && let Some(host) = host
    {
        t.translation = host;
        *v = Velocity::default();
    }
    *placed = Some(entity);
    let dt = time.delta;
    if !(dt > 0.0) {
        return;
    }
    let input = actions.player(1);
    let grounded = physics.contacts(entity).iter().any(|c| c.normal.y > GROUND_NORMAL);
    let mut velocity = v.linear;
    velocity.x = move_toward(velocity.x, input.value(MOVE) * SPEED, if grounded { GROUND_ACCEL } else { AIR_ACCEL } * dt);
    if grounded && input.pressed(JUMP_ACTION) {
        velocity.y = JUMP;
    }
    if velocity != v.linear {
        v.linear = velocity;
    }
}

/// 换关：重生点、掉坑次数、踩过的敌人都是上一关的，重新来。
fn reset_on_level_start(mut starts: MessageReader<LevelStart>, mut ctl: ResMut<Controller>) {
    if starts.read().count() > 0 {
        *ctl = Controller::default();
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
        .add_event(RESTART)
        .add_event(STOMP)
        .init_resource::<Controller>()
        .init_resource::<Sfx>()
        .add_systems(Startup, load_sfx)
        .init_resource::<debug::DebugView>()
        .add_systems(Update, (reset_on_level_start, control, control_guest, debug::debug_view, toggle_perf).chain());
    app.world_mut().resource_mut::<Actions>().set_map(default_controls());
    app
}

pub fn factory() -> nge_core::GameFactory {
    nge_ecs::factory(app)
}

/// `nsh run` / `nsh package` 生成的宿主要求应用的 crate 有它；TSX 不从这里导入东西，只带上资源（见 docs/native-components.md）。
#[nsh_core::prelude::native_module("platformer/native")]
fn native(registry: &mut nsh_core::prelude::Registry) {
    registry.assets(assets!());
    // Android 的生成宿主不调组件注册函数：在这里把游戏视图注册进仓库里改过的 nsh-platform-android（见 docs/android.md）
    #[cfg(target_os = "android")]
    nge_platform::android::register(factory());
}

/// 把游戏视图注册进生成的宿主。
#[cfg(target_os = "macos")]
#[nsh_core::prelude::native_components(macos)]
fn components(components: &nsh_platform_macos::Components) {
    components.register(nge_core::GAME_COMPONENT, nge_platform::macos::component(factory()));
}

#[cfg(target_os = "ios")]
#[nsh_core::prelude::native_components(ios)]
fn components(components: &nsh_platform_ios::Components) {
    components.register(nge_core::GAME_COMPONENT, nge_platform::ios::component(factory()));
}

/// 按 P 开关左上角的性能面板（见 docs/perf-overlay.md）。
fn toggle_perf(input: Res<Input>, mut overlay: ResMut<nge_text::PerfOverlay>) {
    if input.key_pressed(PERF_KEY) {
        overlay.0 = !overlay.0;
    }
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
            "sprites": [{ "id": PLAYER, "w": 24, "h": 32, "y": 16, "body": { "kind": "character", "friction": 0 } }],
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
    fn networking_never_blocks_and_remote_players_are_not_the_player() {
        let mut app = app();
        let mut scene = level(None);
        // 连不上的中继：照常单机玩，状态是连接中
        scene["net"] = json!({ "url": "ws://127.0.0.1:1", "room": "platformer-1", "name": "P100", "sync": [PLAYER] });
        scene["sprites"].as_array_mut().unwrap().push(json!({ "id": "net:7:player", "x": 100, "y": 16, "w": 24, "h": 32, "color": "#5dade280" }));
        app.prop("scene", Some(&scene));
        let started = std::time::Instant::now();
        let events = run(&mut app, 20);
        key(&mut app, "ArrowRight", true);
        run(&mut app, 30);
        assert!(started.elapsed().as_secs_f32() < 2.0, "联网不卡主循环");
        assert!(events.iter().any(|(name, e)| name == "net" && e["status"] == "connecting"), "{events:?}");
        assert!(player(&mut app).1.x > 0.0, "自己照常走");
        let world = app.world_mut();
        let (_, t) = world.query::<(&SceneId, &Transform2d)>().iter(world).find(|(id, _)| id.0 == "net:7:player").unwrap();
        assert_eq!(t.translation, Vec2::new(100.0, 16.0), "没收到数据的远端角色不动，也不受控制");
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

    /// 一个手柄的整份状态（平台层按设备发的，见 docs/input-actions.md）。
    fn pad(app: &mut App, id: u32, keys: &[&str], left: [f32; 2]) {
        app.command("pad", &json!({ "id": id, "keys": keys, "left": left }));
    }

    #[test]
    fn plays_with_a_gamepad() {
        let mut app = app();
        app.prop("scene", Some(&level(None)));
        run(&mut app, 20);
        let (start, ground) = { let p = player(&mut app).0; (p.x, p.y) };
        pad(&mut app, 0, &["GamepadLeftStickRight"], [1.0, 0.0]);
        run(&mut app, 30);
        assert!(player(&mut app).0.x > start + 50.0, "左摇杆往右跑");
        pad(&mut app, 0, &["GamepadLeft"], [0.0; 2]);
        run(&mut app, 60);
        assert!(player(&mut app).1.x < -200.0, "十字键往左跑");
        pad(&mut app, 0, &[], [0.0; 2]);
        run(&mut app, 30);
        pad(&mut app, 0, &["GamepadA"], [0.0; 2]);
        run(&mut app, 10);
        assert!(player(&mut app).0.y > ground + 50.0, "A 跳");
    }

    #[test]
    fn the_stick_sets_the_speed_all_the_way() {
        let mut app = app();
        app.prop("scene", Some(&level(None)));
        run(&mut app, 20);
        // 推过一半时平台层还会把方向当成键发过来：速度照样跟着推的力度，不从一半跳到全速
        for (push, keys) in [(0.25, &[][..]), (0.55, &["GamepadLeftStickRight"][..]), (0.9, &["GamepadLeftStickRight"][..])] {
            pad(&mut app, 0, keys, [push, 0.0]);
            run(&mut app, 60);
            let speed = player(&mut app).1.x;
            assert!((speed - push * SPEED).abs() < 2.0, "推 {push}，走 {push} 的速度：{speed}");
        }
    }

    /// 触屏按钮按动作发（`action` command）：改了键也照样跳，不会串到别的动作上。
    #[test]
    fn touch_buttons_drive_actions_whatever_the_bindings() {
        let mut app = app();
        app.prop("scene", Some(&level(None)));
        run(&mut app, 20);
        let (start, ground) = { let p = player(&mut app).0; (p.x, p.y) };
        // 跳和重来的键对调了
        let mut swapped: Value = serde_json::from_str(include_str!("../controls.json")).unwrap();
        swapped["players"][0]["jump"]["keys"] = json!(["KeyR"]);
        swapped["players"][0]["restart"]["keys"] = json!(["Space"]);
        app.prop("actions", Some(&swapped));
        app.command("action", &json!({ "action": "move", "down": true, "x": 1, "source": "right" }));
        run(&mut app, 30);
        assert!(player(&mut app).0.x > start + 50.0, "→ 按钮往右走");
        app.command("action", &json!({ "action": "move", "down": false, "source": "right" }));
        run(&mut app, 20);
        app.command("action", &json!({ "action": "jump", "down": true }));
        let events = run(&mut app, 10);
        assert!(player(&mut app).0.y > ground + 50.0, "跳按钮还是跳");
        assert!(events.iter().all(|(n, _)| n != "restart"), "没有变成重来：{events:?}");
        app.command("action", &json!({ "action": "jump", "down": false }));
        run(&mut app, 60);
        app.command("action", &json!({ "action": "restart", "down": true }));
        assert!(run(&mut app, 2).iter().any(|(n, _)| n == "restart"), "重来按钮还是重来");
    }

    /// 面板上绑键的三步（`controls.test.tsx` 生成同一份夹具）：平时、等按键、绑成了 X。
    #[test]
    fn remap_fixture_drives_the_engine() {
        let steps: Vec<Value> = serde_json::from_str(include_str!("../remap-fixture.json")).unwrap();
        assert_eq!(steps[0]["players"], serde_json::from_str::<Value>(include_str!("../controls.json")).unwrap()["players"], "夹具的第一步是默认绑定");
        let mut app = app();
        app.prop("scene", Some(&level(None)));
        app.prop("actions", Some(&steps[0]));
        run(&mut app, 20);
        let ground = player(&mut app).0.y;
        app.prop("actions", Some(&steps[1]));
        key(&mut app, "KeyX", true);
        run(&mut app, 3);
        app.prop("actions", Some(&steps[2]));
        run(&mut app, 10);
        assert!((player(&mut app).0.y - ground).abs() < 1.0, "绑上去的 X 还按着：不跳");
        key(&mut app, "KeyX", false);
        key(&mut app, "Space", true);
        run(&mut app, 10);
        assert!((player(&mut app).0.y - ground).abs() < 1.0, "空格不再是跳");
        key(&mut app, "KeyX", true);
        run(&mut app, 10);
        assert!(player(&mut app).0.y > ground + 50.0, "松开再按 X 才跳");
    }

    /// 默认绑定是 controls.json（TSX 读的同一份）；TSX 发来的 `actions` 换掉它。
    #[test]
    fn controls_follow_the_bindings() {
        let defaults = default_controls();
        assert_eq!(defaults.players.len(), 2, "两个玩家的表");
        for action in ["move", "jump", "restart"] {
            assert!(defaults.players[0].contains_key(action), "{action}");
        }
        let mut app = app();
        app.prop("scene", Some(&level(None)));
        run(&mut app, 20);
        let ground = player(&mut app).0.y;
        let mut rebound: Value = serde_json::from_str(include_str!("../controls.json")).unwrap();
        rebound["players"][0]["jump"]["keys"] = json!(["KeyX"]);
        app.prop("actions", Some(&rebound));
        key(&mut app, "Space", true);
        run(&mut app, 10);
        assert!((player(&mut app).0.y - ground).abs() < 1.0, "空格不再是跳");
        key(&mut app, "KeyX", true);
        run(&mut app, 10);
        assert!(player(&mut app).0.y > ground + 50.0, "改成 X 跳");
    }

    /// `game.tsx` 往场景里放的第二个角色（和第一个同一个碰撞分组）。
    fn guest_sprite(x: f32) -> Value {
        json!({ "id": GUEST, "x": x, "y": 16, "w": 24, "h": 32, "body": { "kind": "character", "friction": 0, "group": 1 } })
    }

    /// 地面上两个角色：`player` 和声明在右边 100 处的 `player2`。
    fn two_players() -> Value {
        let mut scene = level(None);
        scene["sprites"][0]["body"]["group"] = json!(1);
        scene["sprites"].as_array_mut().unwrap().push(guest_sprite(100.0));
        scene
    }

    #[test]
    fn the_second_player_appears_beside_the_first_wherever_that_is() {
        let mut app = app();
        let mut alone = level(None);
        alone["sprites"][0]["body"]["group"] = json!(1);
        app.prop("scene", Some(&alone));
        run(&mut app, 20);
        key(&mut app, "ArrowRight", true);
        run(&mut app, 60);
        key(&mut app, "ArrowRight", false);
        run(&mut app, 20);
        let first = player(&mut app).0;
        assert!(first.x > 150.0);
        // 第二个手柄接入：TSX 把它声明在起点旁边，Rust 挪到第一个角色身边
        app.prop("scene", Some(&two_players()));
        run(&mut app, 20);
        assert!((guest(&mut app).0 - first).length() < 4.0, "出现在第一个角色身边 {:?} {first}", guest(&mut app).0);
        // 手柄拔掉又接上：重新出现在第一个角色现在的位置
        key(&mut app, "ArrowLeft", true);
        run(&mut app, 30);
        key(&mut app, "ArrowLeft", false);
        app.prop("scene", Some(&alone));
        run(&mut app, 20);
        let moved = player(&mut app).0;
        app.prop("scene", Some(&two_players()));
        run(&mut app, 20);
        assert!((guest(&mut app).0 - moved).length() < 4.0, "重连后在现在的位置 {:?} {moved}", guest(&mut app).0);
        // 换关：跟着第一个角色到新关卡的起点
        let mut next = two_players();
        next["level"] = json!(2);
        next["sprites"][0]["x"] = json!(-300);
        app.prop("scene", Some(&next));
        run(&mut app, 20);
        assert!((guest(&mut app).0.x + 300.0).abs() < 4.0, "新关卡里在第一个角色的起点 {:?}", guest(&mut app).0);
    }

    #[test]
    fn the_two_players_walk_through_each_other() {
        let mut app = app();
        app.prop("scene", Some(&two_players()));
        pad(&mut app, 0, &[], [0.0; 2]);
        pad(&mut app, 1, &[], [0.0; 2]);
        run(&mut app, 20);
        // 第二个角色先走到右边，第一个再穿过它
        pad(&mut app, 1, &[], [1.0, 0.0]);
        run(&mut app, 30);
        pad(&mut app, 1, &[], [0.0; 2]);
        run(&mut app, 20);
        let (resting, ground) = (guest(&mut app).0, player(&mut app).0.y);
        assert!(resting.x > player(&mut app).0.x + 60.0);
        key(&mut app, "ArrowRight", true);
        run(&mut app, 90);
        assert!(player(&mut app).0.x > resting.x + 60.0, "穿过去了，没被挡住 {:?}", player(&mut app).0);
        assert!((guest(&mut app).0 - resting).length() < 1.0, "也没把它推走");
        assert!((player(&mut app).0.y - ground).abs() < 1.0, "没踩到它头上");
    }

    fn guest(app: &mut App) -> (Vec2, Vec2) {
        let world = app.world_mut();
        let (_, t, v) = world.query::<(&SceneId, &Transform2d, &Velocity)>().iter(world).find(|(id, ..)| id.0 == GUEST).unwrap();
        (t.translation, v.linear)
    }

    #[test]
    fn the_second_pad_moves_the_second_player() {
        let mut app = app();
        app.prop("scene", Some(&two_players()));
        pad(&mut app, 0, &[], [0.0; 2]);
        pad(&mut app, 1, &[], [0.0; 2]);
        run(&mut app, 20);
        let (first, second) = (player(&mut app).0, guest(&mut app).0);
        pad(&mut app, 1, &["GamepadLeftStickRight"], [1.0, 0.0]);
        run(&mut app, 30);
        assert!(guest(&mut app).0.x > second.x + 50.0, "第二个手柄让第二个角色跑");
        assert!((player(&mut app).0.x - first.x).abs() < 1.0, "第一个角色不动");
        pad(&mut app, 1, &["GamepadA"], [0.0; 2]);
        run(&mut app, 8);
        assert!(guest(&mut app).0.y > second.y + 40.0, "第二个手柄的 A 让它跳");
        assert!((player(&mut app).0.y - first.y).abs() < 1.0);
        // 第一个手柄只管第一个角色
        pad(&mut app, 1, &[], [0.0; 2]);
        run(&mut app, 60);
        let rest = guest(&mut app).0;
        pad(&mut app, 0, &["GamepadLeft"], [0.0; 2]);
        run(&mut app, 30);
        assert!(player(&mut app).0.x < first.x - 50.0);
        assert!((guest(&mut app).0.x - rest.x).abs() < 1.0);
    }

    #[test]
    fn the_second_player_also_has_keys_and_comes_back_after_a_fall() {
        let mut app = app();
        let mut scene = level(Some((22, 24)));
        // 第二个角色站在坑的左边
        scene["sprites"].as_array_mut().unwrap().push(guest_sprite(0.0));
        scene["sprites"][0]["x"] = json!(-200);
        scene["sprites"][0]["body"]["group"] = json!(1);
        app.prop("scene", Some(&scene));
        run(&mut app, 20);
        let start = guest(&mut app).0;
        key(&mut app, "KeyH", true);
        let fell = (0..180).any(|_| {
            run(&mut app, 1);
            guest(&mut app).0.y < -100.0
        });
        assert!(fell, "H 往右走进坑里");
        key(&mut app, "KeyH", false);
        let events = run(&mut app, 60);
        assert!((guest(&mut app).0 - start).length() < 1.0, "回到第一个角色身边 {:?}", guest(&mut app));
        assert!(events.iter().all(|(n, _)| n != "respawn"), "不算第一个玩家的失手");
    }

    fn rumbles(events: &[(String, Value)]) -> Vec<&Value> {
        events.iter().filter(|(n, _)| n == "nge.rumble").map(|(_, e)| e).collect()
    }

    #[test]
    fn landing_and_getting_hurt_rumble_the_first_players_pad() {
        let mut app = app();
        app.prop("scene", Some(&level(Some((22, 24)))));
        pad(&mut app, 3, &[], [0.0; 2]);
        let events = run(&mut app, 20);
        assert!(rumbles(&events).is_empty(), "站着、刚开始不震");
        // 跳一下再落地
        key(&mut app, "Space", true);
        let events = run(&mut app, 60);
        key(&mut app, "Space", false);
        let landed = rumbles(&events);
        assert_eq!(landed.len(), 1, "落地震一下：{events:?}");
        assert_eq!(landed[0]["pad"], json!(3), "第一个玩家的手柄");
        // 掉进坑里：重一点
        key(&mut app, "ArrowRight", true);
        let mut events = Vec::new();
        for _ in 0..120 {
            events.extend(run(&mut app, 1));
            if events.iter().any(|(n, _)| n == "respawn") {
                break;
            }
        }
        let hurt = rumbles(&events);
        assert_eq!(hurt.len(), 1, "{events:?}");
        assert!(hurt[0]["low"].as_f64().unwrap() > landed[0]["low"].as_f64().unwrap(), "受伤比落地重");
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
        assert!((p - start).length() < 0.5 && v.length() < 1e-3, "送回起点、速度清零 {p} {v}");
    }

    /// 换关时重生点、掉坑次数重新算：新关卡的起点不一样。
    #[test]
    fn a_new_level_resets_the_spawn_and_the_falls() {
        let mut app = app();
        let scene = |level: u32, x: f32| {
            let mut s = level_with_pit();
            s["level"] = json!(level);
            s["sprites"][0]["x"] = json!(x);
            s
        };
        app.prop("scene", Some(&scene(1, 0.0)));
        run(&mut app, 20);
        key(&mut app, "ArrowRight", true);
        (0..120).find(|_| run(&mut app, 1).iter().any(|(n, _)| n == "respawn")).expect("第一关掉坑");
        key(&mut app, "ArrowRight", false);
        app.prop("scene", Some(&scene(2, -200.0)));
        run(&mut app, 20);
        key(&mut app, "ArrowRight", true);
        let respawn = (0..240).find_map(|_| run(&mut app, 1).into_iter().find(|(n, _)| n == "respawn")).expect("第二关掉坑");
        assert_eq!(respawn.1, json!({ "falls": 1 }), "掉坑次数从 0 重新数");
        assert!((player(&mut app).0.x + 200.0).abs() < 0.1, "送回第二关的起点");
    }

    fn level_with_pit() -> Value {
        level(Some((22, 24)))
    }

    fn voices(app: &mut App) -> usize {
        nge_assets::AssetServer::apply(app.world_mut(), true);
        run(app, 1);
        app.world().resource::<nge_audio::Audio>().voices()
    }

    #[test]
    fn jumping_and_falling_play_sounds() {
        let mut app = app();
        app.prop("scene", Some(&level(Some((22, 24)))));
        run(&mut app, 20);
        assert_eq!(voices(&mut app), 0, "站着不出声");
        key(&mut app, "Space", true);
        run(&mut app, 1);
        assert_eq!(voices(&mut app), 1, "起跳");
        key(&mut app, "Space", false);
        key(&mut app, "ArrowRight", true);
        (0..120).find(|_| run(&mut app, 1).iter().any(|(n, _)| n == "respawn")).expect("掉进坑里");
        assert_eq!(voices(&mut app), 2, "掉坑");
    }

    #[test]
    fn restart_wins_over_a_jump_and_is_ignored_while_paused() {
        let mut app = app();
        app.prop("scene", Some(&level(None)));
        run(&mut app, 20);
        let ground = player(&mut app).0.y;
        key(&mut app, "Space", true);
        key(&mut app, "KeyR", true);
        let events = run(&mut app, 1);
        assert!(events.iter().any(|(n, _)| n == "restart") && events.iter().all(|(n, _)| n != "respawn"), "{events:?}");
        run(&mut app, 10);
        assert!((player(&mut app).0.y - ground).abs() < 1.0, "同一帧的跳不算");
        assert_eq!(voices(&mut app), 0, "重来不出声");
        key(&mut app, "Space", false);
        key(&mut app, "KeyR", false);

        let mut paused = level(None);
        paused["timeScale"] = json!(0);
        app.prop("scene", Some(&paused));
        run(&mut app, 1);
        key(&mut app, "GamepadMenu", true);
        assert!(run(&mut app, 5).iter().all(|(n, _)| n != "restart"), "暂停时不重来");
    }

    /// TSX 那边的接线：触屏按钮和摇杆按动作发，两个角色同一个碰撞分组。
    #[test]
    fn tsx_uses_action_buttons_and_groups_the_players() {
        let tsx = include_str!("../game.tsx");
        assert!(!tsx.contains("KeyButton"), "触屏按钮不发死键名");
        for button in [r#"action="restart""#, r#"action="jump""#] {
            assert!(tsx.contains(&format!("<ActionButton game={{props.game}} {button}")), "{button}");
        }
        assert!(tsx.contains(r#"<VirtualStick game={props.game} action="move""#), "走路是摇杆，驱动 move");
        assert!(tsx.contains(r#"id: "player""#) && tsx.contains("collide: true, group: PLAYER_GROUP }"), "第一个角色带分组");
        assert!(include_str!("../controls.ts").contains(&format!("export const GUEST = \"{GUEST}\"")));
    }

    /// game.tsx 里用到的声音都在打包的资源里。
    #[test]
    fn sounds_are_bundled() {
        let uris: Vec<_> = assets!().iter().map(|f| f.uri).collect();
        for name in ["jump", "fall", "stomp", "coin", "win"] {
            assert!(uris.contains(&format!("asset:/@platformer/{name}.wav").as_str()), "{name} 不在 {uris:?}");
        }
        let tsx = [include_str!("../game.tsx"), include_str!("../levels.ts")].concat();
        assert!(tsx.contains("asset:/@platformer/coin.wav") && tsx.contains("asset:/@platformer/win.wav"));
        assert!(tsx.contains(&format!("id: `{ENEMY}${{i}}`")), "levels.ts 的敌人 id 用 {ENEMY} 开头");
        assert!(uris.contains(&"asset:/@platformer/crate.glb") && tsx.contains("asset:/@platformer/crate.glb"), "2.5D 的箱子");
        assert!(uris.contains(&"asset:/@platformer/hills.ktx2") && tsx.contains("asset:/@platformer/hills.ktx2"), "横向重复的远山");
        assert!(!uris.iter().any(|u| u.ends_with("hills.png")), "PNG 源图放在 tools/，不打包");
    }

    /// 远山是 BC3 压缩贴图，带 mipmap，宽高是块的整数倍（GPU 能直接用）。
    #[test]
    fn hills_are_a_compressed_texture() {
        let image = nge_assets::Image::decode(include_bytes!("../assets/hills.ktx2")).unwrap();
        let c = image.compressed.expect("压缩贴图");
        assert_eq!((image.size, c.format, c.srgb, c.levels.len()), (nge_ecs::glam::UVec2::new(256, 160), nge_assets::BlockFormat::Bc3, true, 9));
    }

    #[test]
    fn r_restarts_from_the_spawn() {
        let mut app = app();
        app.prop("scene", Some(&level(Some((22, 24)))));
        run(&mut app, 20);
        let start = player(&mut app).0;
        key(&mut app, "ArrowRight", true);
        (0..120).find(|_| run(&mut app, 1).iter().any(|(n, _)| n == "respawn")).expect("掉进坑里");
        run(&mut app, 30);
        key(&mut app, "ArrowRight", false);
        key(&mut app, "KeyR", true);
        let events = run(&mut app, 1);
        assert_eq!(events.iter().filter(|(n, _)| n == "restart").count(), 1, "{events:?}");
        let (p, v) = player(&mut app);
        assert!((p - start).length() < 0.5 && v.length() < 1e-3, "回到起点、速度清零 {p} {v}");
        assert!(run(&mut app, 10).iter().all(|(n, _)| n != "restart"), "按住不重复");
        key(&mut app, "KeyR", false);
        key(&mut app, "ArrowRight", true);
        let respawn = (0..120).find_map(|_| run(&mut app, 1).into_iter().find(|(n, _)| n == "respawn")).expect("又掉进坑里");
        assert_eq!(respawn.1, json!({ "falls": 1 }), "掉坑次数从 0 重新数");
    }

    /// 场景里加一只不动的敌人（kinematic，24 × 24，站在地面上）。
    fn with_enemy(mut scene: Value, x: f32) -> Value {
        let enemy = json!({ "id": format!("{ENEMY}0"), "x": x, "y": 12, "w": 24, "h": 24, "body": { "kind": "kinematic", "friction": 0 } });
        scene["sprites"].as_array_mut().unwrap().push(enemy);
        scene
    }

    /// 30° 的斜坡（和 levels.ts 的 rampOf 一样摆：顶面从 (100, 0) 到 (100 + 96√3, 96)）：角色控制器走上去一直贴着坡，走到顶上。
    #[test]
    fn walks_up_a_ramp() {
        let mut app = app();
        let mut scene = level(None);
        let (a, b) = (Vec2::new(100.0, 0.0), Vec2::new(100.0 + 96.0 * 3f32.sqrt(), 96.0));
        let d = b - a;
        let n = Vec2::new(-d.y, d.x).normalize();
        let center = (a + b) / 2.0 - n * 8.0;
        scene["sprites"].as_array_mut().unwrap().push(json!({ "id": "ramp", "x": center.x, "y": center.y, "w": d.length(), "h": 16,
            "rotation": d.y.atan2(d.x).to_degrees(), "body": { "kind": "fixed", "friction": 0 } }));
        app.prop("scene", Some(&scene));
        run(&mut app, 20);
        key(&mut app, "ArrowRight", true);
        let mut highest: f32 = 0.0;
        for _ in 0..90 {
            run(&mut app, 1);
            highest = highest.max(player(&mut app).0.y);
        }
        assert!(highest > 96.0 + 15.0, "走到坡顶 {highest}");
    }

    /// 右上方的幽灵：角色走近了才追，飞过来撞到角色，角色回到起点。
    #[test]
    fn a_ghost_chases_once_you_come_close_and_knocks_you_back() {
        let mut app = app();
        let mut scene = level(None);
        let ghost = json!({ "id": format!("{ENEMY}ghost-0"), "x": 400, "y": 60, "w": 22, "h": 22, "body": { "kind": "kinematic", "friction": 0 },
            "chase": { "target": PLAYER, "speed": 150, "sight": 250 } });
        scene["sprites"].as_array_mut().unwrap().push(ghost);
        app.prop("scene", Some(&scene));
        assert!(named(&run(&mut app, 30), "chase").is_empty(), "离得远，看不见");
        key(&mut app, "ArrowRight", true);
        let seen = (0..120).find_map(|_| named(&run(&mut app, 1), "chase").first().map(|e| (*e).clone())).expect("走近了");
        assert_eq!((seen["id"].as_str(), seen["state"].as_str()), (Some("enemy-ghost-0"), Some("chase")));
        key(&mut app, "ArrowRight", false);
        let x = player(&mut app).0.x;
        assert!(x > 100.0, "走了一段才看见 {x}");
        let respawn = (0..180).find(|_| !named(&run(&mut app, 1), "respawn").is_empty());
        assert!(respawn.is_some(), "幽灵飞过来撞到了角色");
    }

    /// 和 levels.ts 的 towerAt 一样摆的一摞 3D 箱子（边长 14、留 1 的缝）：摞着自己不倒；角色跑过 sensor 时发 collide，
    /// 游戏推一下就散开。
    #[test]
    fn a_tower_of_3d_crates_stands_and_scatters_when_pushed() {
        let mut app = app();
        let mut scene = level(None);
        let (x, s) = (200.0, 14.0);
        let mut models = vec![json!({ "id": "floor", "model": "asset:/@platformer/crate.glb", "x": x, "y": -10, "z": -20, "scale": 0.001,
            "body": { "kind": "fixed", "size": [384, 20, 192] } })];
        for (n, (i, row)) in [(-1.0, 0.0), (0.0, 0.0), (1.0, 0.0), (-0.5, 1.0), (0.5, 1.0), (0.0, 2.0)].into_iter().enumerate() {
            models.push(json!({ "id": format!("box-{n}"), "model": "asset:/@platformer/crate.glb", "x": x + i * (s + 1.0), "y": s / 2.0 + row * s,
                "z": -20, "scale": s, "body": { "density": 0.5 } }));
        }
        scene["physics3d"] = json!({ "gravity": [0, -1800, 0] });
        scene["models"] = json!(models);
        scene["sprites"].as_array_mut().unwrap()[0]["body"]["collide"] = json!(true);
        scene["sprites"].as_array_mut().unwrap().push(json!({ "id": "tower-trigger", "x": x - 3.0 * s, "y": 32, "w": 8, "h": 64, "body": { "kind": "fixed", "sensor": true } }));
        app.prop("scene", Some(&scene));
        let boxes = |app: &mut App| -> Vec<nge_ecs::glam::Vec3> {
            let world = app.world_mut();
            let mut found: Vec<(String, nge_ecs::glam::Vec3)> = world.query::<(&SceneId, &nge_render3d::Transform3d)>().iter(world)
                .filter(|(id, _)| id.0.starts_with("box-")).map(|(id, t)| (id.0.clone(), t.translation)).collect();
            found.sort_by(|a, b| a.0.cmp(&b.0));
            found.into_iter().map(|(_, p)| p).collect()
        };
        run(&mut app, 2);
        let stacked = boxes(&mut app);
        run(&mut app, 120);
        for (a, b) in stacked.iter().zip(boxes(&mut app)) {
            assert!(a.distance(b) < 1.0, "摞着不倒 {a} → {b}");
        }
        key(&mut app, "ArrowRight", true);
        let hit = (0..120).find_map(|_| {
            named(&run(&mut app, 1), "collide").into_iter().find(|e| e["started"] == json!(true) && (e["a"] == "tower-trigger" || e["b"] == "tower-trigger")).cloned()
        });
        assert!(hit.is_some(), "角色跑过 sensor");
        for n in 0..6 {
            app.command("push", &json!({ "id": format!("box-{n}"), "x": 160 + n * 25, "y": 120 + n * 40, "z": if n % 2 == 1 { 60 } else { -60 } }));
        }
        run(&mut app, 90);
        let after = boxes(&mut app);
        let spread = after.iter().map(|p| p.x).fold(f32::MIN, f32::max) - after.iter().map(|p| p.x).fold(f32::MAX, f32::min);
        assert!(spread > 60.0, "散开了 {after:?}");
        assert!(after.iter().all(|p| p.y > 0.0 && p.y < 60.0), "落在地上 {after:?}");
    }

    fn named<'a>(events: &'a [(String, Value)], name: &str) -> Vec<&'a Value> {
        events.iter().filter(|(n, _)| n == name).map(|(_, d)| d).collect()
    }

    #[test]
    fn stomping_an_enemy_bounces_once_until_restart() {
        let mut app = app();
        let mut scene = with_enemy(level(None), 0.0);
        scene["sprites"][0]["y"] = json!(120);
        app.prop("scene", Some(&scene));
        let events = run(&mut app, 20);
        let stomps = named(&events, "stomp");
        assert!(stomps.len() == 1 && stomps[0]["id"] == "enemy-0" && stomps[0]["y"].as_f64().unwrap() > 30.0, "{events:?}");
        assert!(named(&events, "respawn").is_empty(), "踩到不算被撞");
        let top = (0..30).map(|_| { run(&mut app, 1); player(&mut app).0.y }).fold(f32::MIN, f32::max);
        // 敌人顶边 24，角色站上去中心在 40；没按跳弹一半（≈ 28）
        assert!(top > 60.0, "弹起来了 {top}");
        // TSX 还没去掉它的刚体：落回去也不再算一次
        let events = run(&mut app, 60);
        assert!(named(&events, "stomp").is_empty() && named(&events, "respawn").is_empty(), "{events:?}");
        assert_eq!(voices(&mut app), 1, "踩到的音效");

        key(&mut app, "KeyR", true);
        run(&mut app, 1);
        key(&mut app, "KeyR", false);
        let events = run(&mut app, 30);
        assert_eq!(named(&events, "stomp").len(), 1, "重新开始后又能踩 {events:?}");
    }

    #[test]
    fn holding_jump_on_a_stomp_bounces_higher() {
        let mut app = app();
        let mut scene = with_enemy(level(None), 0.0);
        scene["sprites"][0]["y"] = json!(120);
        app.prop("scene", Some(&scene));
        key(&mut app, "Space", true);
        let top = (0..60).map(|_| { run(&mut app, 1); player(&mut app).0.y }).skip(20).fold(f32::MIN, f32::max);
        // 站在敌人上中心在 40，按着跳弹整跳（≈ 114）
        assert!(top > 130.0, "按着跳弹得高 {top}");
    }

    #[test]
    fn touching_an_enemy_from_the_side_sends_you_back() {
        let mut app = app();
        app.prop("scene", Some(&with_enemy(level(None), 100.0)));
        run(&mut app, 20);
        let start = player(&mut app).0;
        key(&mut app, "ArrowRight", true);
        let mut frames = Vec::new();
        while frames.len() < 60 && frames.last().is_none_or(|e: &Vec<_>| named(e, "respawn").is_empty()) {
            frames.push(run(&mut app, 1));
        }
        let hit = frames.len() - 1;
        // 从 0 走到 100 的敌人跟前（中心相距 24）大约要 18 帧
        assert!(hit > 12, "走到跟前才撞上，第 {hit} 帧");
        assert_eq!(named(&frames[hit], "respawn"), [&json!({ "falls": 1 })]);
        assert!(frames.iter().all(|e| named(e, "stomp").is_empty()));
        key(&mut app, "ArrowRight", false);
        assert!(named(&run(&mut app, 30), "respawn").is_empty(), "松开以后不再撞");
        assert!((player(&mut app).0 - start).length() < 20.0, "回到起点附近 {:?}", player(&mut app));
    }

    /// 这一帧调试绘制的（位置，大小）。
    fn overlays(app: &mut App) -> Vec<(Vec2, Vec2)> {
        let instances = app.world().resource::<nge_render2d::SpriteInstances>();
        instances.iter().filter(|(_, i)| i.z == nge_render2d::GIZMO_Z).map(|(_, i)| (i.position, i.size)).collect()
    }

    #[test]
    fn p_toggles_the_perf_panel() {
        let mut app = app();
        app.prop("scene", Some(&level(None)));
        run(&mut app, 2);
        let on = |app: &App| app.world().resource::<nge_text::PerfOverlay>().0;
        assert!(!on(&app));
        key(&mut app, "KeyP", true);
        run(&mut app, 1);
        key(&mut app, "KeyP", false);
        run(&mut app, 3);
        assert!(on(&app), "按一次打开，场景每帧同步也不会关掉");
        key(&mut app, "KeyP", true);
        run(&mut app, 1);
        assert!(!on(&app), "再按一次关掉");
    }

    #[test]
    fn q_toggles_the_debug_view_of_queries() {
        let mut app = app();
        app.prop("scene", Some(&with_enemy(level(None), 60.0)));
        run(&mut app, 20);
        assert!(overlays(&mut app).is_empty());
        key(&mut app, "KeyQ", true);
        run(&mut app, 1);
        key(&mut app, "KeyQ", false);
        run(&mut app, 1);
        let shown = overlays(&mut app);
        // 往下的射线碰到地面：交点（8 × 8）在 y = 0
        assert!(shown.iter().any(|(at, size)| *size == Vec2::splat(8.0) && at.y.abs() < 1.0), "{shown:?}");
        // 框里的敌人被标出来（10 × 10，在敌人身上）
        assert!(shown.iter().any(|(at, size)| *size == Vec2::splat(10.0) && (*at - Vec2::new(60.0, 12.0)).length() < 1.0), "{shown:?}");
        // 敌人（24 × 24）的碰撞体轮廓：四条边落在它的边上
        for side in [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::NEG_Y] {
            let edge = Vec2::new(60.0, 12.0) + side * 12.0;
            assert!(shown.iter().any(|(at, _)| (*at - edge).length() < 1.0), "{edge} {shown:?}");
        }
        key(&mut app, "KeyQ", true);
        run(&mut app, 2);
        assert!(overlays(&mut app).is_empty(), "再按一次关掉");
    }

    /// game.tsx 的金币是 sensor、角色开了 collide：走过去时发 collide，金币不挡路。
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

    /// effects.ts 里自己写的着色器都编得过：按「名字: `代码`」取出来，`fn effect` 的按精灵着色器、`fn post` 的按后处理校验。
    #[test]
    fn demo_shaders_compile() {
        let ts = include_str!("../effects.ts");
        let start = ts.find("export const SHADERS").expect("有 SHADERS");
        let end = start + ts[start..].find("\n};").expect("SHADERS 结束");
        let mut found = 0;
        for chunk in ts[start..end].split("`").skip(1).step_by(2) {
            let checked = if chunk.contains("fn post(") { nge_render2d::Shaders::check_post(chunk) } else { nge_render2d::Shaders::check_sprite(chunk) };
            if let Err(e) = checked {
                panic!("{chunk}\n编不过：{e}");
            }
            found += 1;
        }
        assert_eq!(found, 3, "shimmer、starfield、blur");
    }

    /// 终点的旗子带蒙皮和 wave 动画：播起来布的末端前后摆。
    #[test]
    fn flag_waves() {
        use nge_render3d::{Animator, Assets3d, Image, Model3d, Pose, Transform3d};

        let mut app = App::new();
        app.add_plugins(Render3dPlugin);
        let model = app
            .world_mut()
            .resource_scope(|world, mut assets: Mut<Assets3d>| assets.load_gltf(&mut world.resource_mut::<Assets<Image>>(), include_bytes!("../assets/flag.glb")))
            .unwrap();
        let loaded = app.world().resource::<Assets3d>().model(model).unwrap();
        assert_eq!((loaded.parts[0].skin, loaded.skeleton.skins[0].joints.len()), (Some(0), 4));
        let flag = app.world_mut().spawn((Model3d(model), Transform3d::default(), Animator::new("wave"))).id();
        let tip: Vec<f32> = (0..8)
            .map(|_| {
                app.tick(VIEWPORT, 0.125);
                // 节点按父在前展开：骨头 0–3，最后是布
                app.world().get::<Pose>(flag).unwrap().nodes[3].w_axis.z
            })
            .collect();
        let (lo, hi) = tip.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &z| (lo.min(z), hi.max(z)));
        assert!(hi - lo > 0.1, "{tip:?}");
    }

    /// 幽灵果冻：半透明材质、一个变形目标；idle 不动，从 idle 淡到 squish 时权重慢慢起来。
    #[test]
    fn jelly_is_translucent_and_fades_into_a_squish() {
        use nge_render3d::{Animator, Assets3d, Image, Model3d, Pose, Transform3d};

        let uris: Vec<_> = assets!().iter().map(|f| f.uri).collect();
        let ts = [include_str!("../game.tsx"), include_str!("../levels.ts")].concat();
        assert!(uris.contains(&"asset:/@platformer/jelly.glb") && ts.contains("asset:/@platformer/jelly.glb"));
        let mut app = App::new();
        app.add_plugins(Render3dPlugin);
        let model = app
            .world_mut()
            .resource_scope(|world, mut assets: Mut<Assets3d>| assets.load_gltf(&mut world.resource_mut::<Assets<Image>>(), include_bytes!("../assets/jelly.glb")))
            .unwrap();
        let assets = app.world().resource::<Assets3d>();
        let loaded = assets.model(model).unwrap();
        let names: Vec<&str> = loaded.animations.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["idle", "squish"]);
        let part = &loaded.parts[0];
        assert!(assets.material(part.material).blend && assets.material(part.material).base_color.w < 1.0, "半透明");
        assert_eq!(assets.mesh(part.mesh).unwrap().targets.len(), 1);

        let jelly = app.world_mut().spawn((Model3d(model), Transform3d::default(), Animator::new("idle"))).id();
        let weight = |app: &mut App| {
            app.tick(VIEWPORT, 0.05);
            app.world().get::<Pose>(jelly).unwrap().weights[0][0]
        };
        assert_eq!((weight(&mut app), weight(&mut app)), (0.0, 0.0), "idle 不动");
        app.world_mut().get_mut::<Animator>(jelly).unwrap().play("squish", 0.25);
        let fading: Vec<f32> = (0..4).map(|_| weight(&mut app)).collect();
        let full: Vec<f32> = (0..3).map(|_| weight(&mut app)).collect();
        // 淡入时只占一部分，淡完以后按 squish 本身的曲线（0.3 秒压到底）
        assert!(fading.iter().all(|&w| w < 0.6) && fading[3] > 0.0, "{fading:?}");
        assert!(full.iter().any(|&w| w > 0.8), "{full:?}");
    }
}
