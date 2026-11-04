//! 游戏逻辑（ECS，渲染线程）：手指左右拖动飞船（Mac 上也可以用 ← → / A D），躲开下落的陨石。

use nge_ecs::prelude::*;
use nge_render2d::prelude::*;
use nge_render2d::AtlasId;

use crate::protocol::{CONTROL, Control, DIFFICULTY, Difficulty, GAME_OVER, GameOver, HUD, Hud};

const PLAYER_SIZE: f32 = 44.0;
const PLAYER_RADIUS: f32 = 16.0;
/// 飞船离视图底边的距离。
const PLAYER_BOTTOM: f32 = 70.0;
/// 飞船追手指的速度（越大越跟手）。
const STEER: f32 = 14.0;
/// 按住方向键时目标位置移动的速度（point / s）。
const KEY_SPEED: f32 = 420.0;
const LEFT_KEYS: [&str; 2] = ["ArrowLeft", "KeyA"];
const RIGHT_KEYS: [&str; 2] = ["ArrowRight", "KeyD"];
const INVULNERABLE_SECS: f32 = 1.2;
const POINTS_PER_SEC: f32 = 10.0;
/// 每过这么多秒，陨石速度 +100%。
const RAMP_SECS: f32 = 60.0;
const PLAYER_ATLAS_INDEX: u32 = 10;

impl Difficulty {
    fn spawn_every(self) -> f32 {
        match self {
            Difficulty::Normal => 0.6,
            Difficulty::Hard => 0.3,
        }
    }

    fn speed(self) -> f32 {
        match self {
            Difficulty::Normal => 1.0,
            Difficulty::Hard => 1.6,
        }
    }
}

#[derive(Component)]
struct Player {
    target_x: f32,
}

#[derive(Component)]
struct Rock {
    radius: f32,
    velocity: Vec2,
    spin: f32,
}

/// 一局内的计时器。
#[derive(Resource, Default)]
struct Run {
    elapsed: f32,
    spawn_timer: f32,
    invulnerable: f32,
}

#[derive(Resource)]
struct Ships(AtlasId);

#[derive(Resource)]
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32
    }

    fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.next()
    }
}

pub fn app() -> App {
    let mut app = App::new();
    app.add_plugins(Render2dPlugin)
        .add_prop(DIFFICULTY)
        .add_command(CONTROL)
        .insert_resource(Hud::default())
        .add_state(HUD)
        .add_event(GAME_OVER)
        .insert_resource(Run::default())
        .insert_resource(Rng(0x2545_F491))
        .add_systems(Startup, setup)
        .add_systems(Update, control)
        .add_systems(
            Update,
            // 逐个判断（集合条件每帧只判一次）：collide 结束游戏后，同帧的 score 不再加分
            (steer, spawn_rocks, move_rocks, collide, score, blink).chain().after(control).distributive_run_if(playing),
        );
    app
}

fn playing(hud: Res<Hud>) -> bool {
    !hud.paused && !hud.over
}

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>, mut atlases: ResMut<Atlases>) {
    let texture = images.load_png(include_bytes!("../assets/ships.png")).expect("内置图集");
    let atlas = atlases.add(texture, Grid { cols: 4, rows: 4 });
    commands.insert_resource(ClearColor(nge_core::wgpu::Color { r: 0.01, g: 0.01, b: 0.03, a: 1.0 }));
    commands.spawn(Camera2d::default());
    commands.spawn((
        Player { target_x: 0.0 },
        Sprite::from_atlas(atlas, PLAYER_ATLAS_INDEX).with_size(Vec2::splat(PLAYER_SIZE)),
        // 图集里的形状尖头朝下，转半圈让它朝上
        Transform2d { rotation: std::f32::consts::PI, z: 10.0, ..Default::default() },
    ));
    commands.insert_resource(Ships(atlas));
}

fn control(
    mut commands: Commands,
    mut reader: MessageReader<Control>,
    mut hud: ResMut<Hud>,
    mut run: ResMut<Run>,
    rocks: Query<Entity, With<Rock>>,
    mut player: Query<(&mut Player, &mut Transform2d, &mut Sprite)>,
) {
    for control in reader.read() {
        match control {
            Control::Pause if !hud.over => hud.paused = true,
            Control::Resume => hud.paused = false,
            Control::Restart => {
                for rock in &rocks {
                    commands.entity(rock).despawn();
                }
                *hud = Hud { best: hud.best, ..Hud::default() };
                *run = Run::default();
                if let Ok((mut p, mut t, mut sprite)) = player.single_mut() {
                    p.target_x = 0.0;
                    t.translation.x = 0.0;
                    sprite.color.w = 1.0;
                }
            }
            Control::Pause => {}
        }
    }
}

fn steer(time: Res<Time>, screen: Res<Screen>, input: Res<Input>, mut player: Query<(&mut Player, &mut Transform2d)>) {
    let Ok((mut player, mut t)) = player.single_mut() else { return };
    let half = screen.size / 2.0;
    let limit = (half.x - PLAYER_RADIUS).max(0.0);
    if let Some(p) = input.pointers_down().next() {
        player.target_x = p.world.x.clamp(-limit, limit);
    } else {
        // 键盘没按时看手柄左摇杆，推一半走一半的速度
        let keys = input.axis(&LEFT_KEYS, &RIGHT_KEYS);
        let axis = if keys != 0.0 { keys } else { input.stick(Stick::Left).x };
        player.target_x = (player.target_x + axis * KEY_SPEED * time.delta).clamp(-limit, limit);
    }
    let follow = 1.0 - (-STEER * time.delta).exp();
    t.translation.x += (player.target_x - t.translation.x) * follow;
    t.translation.y = -half.y + PLAYER_BOTTOM;
}

#[allow(clippy::too_many_arguments)]
fn spawn_rocks(
    mut commands: Commands,
    time: Res<Time>,
    screen: Res<Screen>,
    difficulty: Res<Difficulty>,
    ships: Res<Ships>,
    mut run: ResMut<Run>,
    mut rng: ResMut<Rng>,
) {
    run.spawn_timer -= time.delta;
    if run.spawn_timer > 0.0 {
        return;
    }
    run.spawn_timer += difficulty.spawn_every();
    let half = screen.size / 2.0;
    let radius = rng.range(12.0, 30.0);
    let ramp = 1.0 + run.elapsed / RAMP_SECS;
    let speed = rng.range(120.0, 220.0) * difficulty.speed() * ramp;
    commands.spawn((
        Rock { radius, velocity: Vec2::new(rng.range(-30.0, 30.0), -speed), spin: rng.range(-3.0, 3.0) },
        Sprite::from_atlas(ships.0, (rng.next() * 8.0) as u32).with_size(Vec2::splat(radius * 2.0)),
        Transform2d::from_xy(rng.range(-half.x, half.x), half.y + radius),
    ));
}

fn move_rocks(mut commands: Commands, time: Res<Time>, screen: Res<Screen>, mut rocks: Query<(Entity, &Rock, &mut Transform2d)>) {
    let bottom = -screen.size.y / 2.0;
    for (entity, rock, mut t) in &mut rocks {
        t.translation += rock.velocity * time.delta;
        t.rotation += rock.spin * time.delta;
        if t.translation.y < bottom - rock.radius {
            commands.entity(entity).despawn();
        }
    }
}

fn collide(
    mut commands: Commands,
    time: Res<Time>,
    mut run: ResMut<Run>,
    mut hud: ResMut<Hud>,
    mut outbox: ResMut<Outbox>,
    player: Query<&Transform2d, With<Player>>,
    rocks: Query<(Entity, &Rock, &Transform2d)>,
) {
    run.invulnerable = (run.invulnerable - time.delta).max(0.0);
    let Ok(player) = player.single() else { return };
    if run.invulnerable > 0.0 {
        return;
    }
    let hit = rocks
        .iter()
        .find(|(_, rock, t)| t.translation.distance(player.translation) < rock.radius + PLAYER_RADIUS);
    let Some((entity, _, _)) = hit else { return };
    commands.entity(entity).despawn();
    run.invulnerable = INVULNERABLE_SECS;
    hud.lives = hud.lives.saturating_sub(1);
    if hud.lives == 0 {
        let new_best = hud.score > hud.best;
        hud.best = hud.best.max(hud.score);
        hud.over = true;
        outbox.send(GAME_OVER, &GameOver { score: hud.score, best: hud.best, new_best });
    }
}

fn score(time: Res<Time>, mut run: ResMut<Run>, mut hud: ResMut<Hud>) {
    run.elapsed += time.delta;
    let score = (run.elapsed * POINTS_PER_SEC) as u32;
    // 分数没变时不标记修改，避免每帧推送 HUD
    if hud.score != score {
        hud.score = score;
    }
}

/// 无敌期间闪烁。
fn blink(run: Res<Run>, mut player: Query<&mut Sprite, With<Player>>) {
    let Ok(mut sprite) = player.single_mut() else { return };
    let visible = run.invulnerable <= 0.0 || (run.invulnerable * 10.0) as u32 % 2 == 0;
    let alpha = if visible { 1.0 } else { 0.25 };
    if sprite.color.w != alpha {
        sprite.color.w = alpha;
    }
}

#[cfg(test)]
mod tests {
    use nge_core::Viewport;
    use serde_json::{Value, json};

    use super::*;
    use crate::protocol::START_LIVES;

    const DT: f32 = 1.0 / 60.0;

    fn viewport() -> Viewport {
        Viewport { size: [400.0, 700.0], pixels: [800, 1400] }
    }

    fn hud(app: &App) -> Hud {
        app.world().resource::<Hud>().clone()
    }

    fn events(out: &[(String, Value)], name: &str) -> Vec<Value> {
        out.iter().filter(|(n, _)| n == name).map(|(_, d)| d.clone()).collect()
    }

    /// 在飞船位置放一块陨石。
    fn drop_rock_on_player(app: &mut App) {
        let world = app.world_mut();
        let at = world.query_filtered::<&Transform2d, With<Player>>().single(world).unwrap().translation;
        world.spawn((Rock { radius: 20.0, velocity: Vec2::ZERO, spin: 0.0 }, Transform2d { translation: at, ..Default::default() }));
    }

    fn rock_count(app: &mut App) -> usize {
        let world = app.world_mut();
        world.query::<&Rock>().iter(world).count()
    }

    fn player_x(app: &mut App) -> f32 {
        let world = app.world_mut();
        world.query_filtered::<&Transform2d, With<Player>>().single(world).unwrap().translation.x
    }

    #[test]
    fn arrow_keys_and_wasd_steer() {
        let mut app = app();
        app.tick(viewport(), DT);
        app.command("key", &json!({ "code": "ArrowRight", "down": true }));
        for _ in 0..30 {
            app.tick(viewport(), DT);
        }
        let right = player_x(&mut app);
        assert!(right > 50.0, "{right}");
        app.command("key", &json!({ "code": "ArrowRight", "down": false }));
        app.command("key", &json!({ "code": "KeyA", "down": true }));
        for _ in 0..300 {
            app.tick(viewport(), DT);
        }
        let left = player_x(&mut app);
        let edge = -viewport().size[0] / 2.0 + PLAYER_RADIUS;
        assert!((left - edge).abs() < 1.0, "按住不放停在边上：{left}");
    }

    #[test]
    fn stick_steers_by_how_far_it_is_pushed() {
        let moved = |x: f32| {
            let mut app = app();
            app.tick(viewport(), DT);
            app.command("gamepad", &json!({ "left": [x, 0.0] }));
            for _ in 0..10 {
                app.tick(viewport(), DT);
            }
            player_x(&mut app)
        };
        let (half, full) = (moved(0.5), moved(1.0));
        assert!(half > 10.0 && full > half * 1.5, "推一半走得慢：{half} / {full}");
        assert!(moved(-1.0) < -10.0);
    }

    #[test]
    fn first_frame_pushes_hud_with_full_lives() {
        let mut app = app();
        let out = app.tick(viewport(), DT);
        assert_eq!(events(&out, "hud"), vec![json!({ "score": 0, "lives": 3, "best": 0, "paused": false, "over": false })]);
    }

    #[test]
    fn score_grows_and_pause_stops_it() {
        let mut app = app();
        for _ in 0..60 {
            app.tick(viewport(), DT);
        }
        let before = hud(&app).score;
        assert!((9..=10).contains(&before), "{before}");
        app.command("control", &json!("Pause"));
        for _ in 0..60 {
            app.tick(viewport(), DT);
        }
        assert_eq!(hud(&app).score, before);
        assert!(hud(&app).paused);
    }

    #[test]
    fn hud_is_not_pushed_every_frame() {
        let mut app = app();
        let pushes: usize = (0..60).map(|_| events(&app.tick(viewport(), DT), "hud").len()).sum();
        // 首帧 1 次 + 分数每 0.1s 变一次
        assert!(pushes <= 12, "{pushes}");
    }

    #[test]
    fn hit_costs_a_life_with_invulnerability() {
        let mut app = app();
        app.tick(viewport(), DT);
        drop_rock_on_player(&mut app);
        app.tick(viewport(), DT);
        assert_eq!(hud(&app).lives, START_LIVES - 1);
        // 无敌期内再撞不扣
        drop_rock_on_player(&mut app);
        app.tick(viewport(), DT);
        assert_eq!(hud(&app).lives, START_LIVES - 1);
    }

    #[test]
    fn game_over_then_restart_keeps_best() {
        let mut app = app();
        for _ in 0..30 {
            app.tick(viewport(), DT);
        }
        let mut over = Vec::new();
        for _ in 0..START_LIVES {
            drop_rock_on_player(&mut app);
            over.extend(events(&app.tick(viewport(), DT), "game_over"));
            app.world_mut().resource_mut::<Run>().invulnerable = 0.0;
        }
        let hud_now = hud(&app);
        assert!(hud_now.over);
        // 结束帧上 HUD 分数和结束事件一致
        assert_eq!(over[0]["score"], json!(hud_now.score));
        assert_eq!(over.len(), 1);
        assert_eq!(over[0]["best"], json!(hud_now.score));
        assert_eq!(over[0]["new_best"], json!(true));

        app.command("control", &json!("Restart"));
        app.tick(viewport(), DT);
        let after = hud(&app);
        assert_eq!((after.lives, after.over, after.best), (START_LIVES, false, hud_now.best));
        // 旧陨石（都在飞船附近）已清掉；重开当帧可能已经在顶部生成了新的
        let world = app.world_mut();
        assert!(world.query::<(&Rock, &Transform2d)>().iter(world).all(|(_, t)| t.translation.y > 0.0));
    }

    #[test]
    fn game_over_frame_does_not_add_score_even_at_tick_boundary() {
        // 每帧都跨过 0.1s 分数边界，若 score 在结束帧还跑就会多 1
        let dt = 0.1;
        let mut app = app();
        app.tick(viewport(), dt);
        app.world_mut().resource_mut::<Hud>().lives = 1;
        drop_rock_on_player(&mut app);
        let out = app.tick(viewport(), dt);
        let over = events(&out, "game_over");
        assert_eq!(over.len(), 1);
        assert_eq!(over[0]["score"], json!(hud(&app).score));
    }

    #[test]
    fn pause_ignored_after_game_over() {
        let mut app = app();
        app.tick(viewport(), DT);
        app.world_mut().resource_mut::<Hud>().over = true;
        app.command("control", &json!("Pause"));
        app.tick(viewport(), DT);
        assert!(!hud(&app).paused);
    }

    #[test]
    fn hard_spawns_more_rocks() {
        fn spawned(difficulty: Difficulty) -> usize {
            let mut app = app();
            app.prop("difficulty", Some(&serde_json::to_value(difficulty).unwrap()));
            // 不让飞船被撞，只数生成了多少（2 秒内陨石还没掉出屏幕）
            app.world_mut().resource_mut::<Run>().invulnerable = 100.0;
            for _ in 0..120 {
                app.tick(viewport(), DT);
            }
            rock_count(&mut app)
        }
        assert!(spawned(Difficulty::Hard) > spawned(Difficulty::Normal));
    }

    #[test]
    fn touch_steers_player() {
        let mut app = app();
        app.tick(viewport(), DT);
        // 视图右边缘附近按下
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 390.0, "y": 600.0 }));
        for _ in 0..60 {
            app.tick(viewport(), DT);
        }
        let world = app.world_mut();
        let x = world.query_filtered::<&Transform2d, With<Player>>().single(world).unwrap().translation.x;
        assert!(x > 150.0, "{x}");
    }
}
