//! 3D 试验场（按 G 进出，场景在 lab.ts / lab.json，见 docs/gameplay3d.md）：WASD / 方向键走、空格跳、掉出去回到起点；
//! 相机跟在角色后上方；平台按时间来回挪；调试视图开着时角色往下打一条射线、往前扫一个球；C 开关级联染色（见 docs/render3d-quality.md）。

use bevy_ecs::prelude::*;
use nge_audio::{Audio, Bus, EffectSpec};
use nge_ecs::glam::{Quat, Vec3};
use nge_ecs::{Input, Time};
use nge_physics3d::{CharacterState3d, Physics3d, Physics3dDebug, QueryFilter, QueryShape, Velocity};
use nge_render3d::{Camera3d, Shadows, Transform3d};
use nge_scene::SceneId;

/// 试验场的角色、平台和绕圈发声体（lab.json 里的 id）。
pub const LAB_HERO: &str = "lab-hero";
pub const LAB_PLATFORM: &str = "lab-platform";
pub const LAB_ORBITER: &str = "lab-orbiter";

/// 开关级联染色的键。
const CASCADE_KEY: &str = "KeyC";
/// 开关水下效果器链的键（见 docs/audio-3d.md）。
const UNDERWATER_KEY: &str = "KeyU";
/// 走的速度（point / s），起跳速度（重力 981 时跳 100 多高）。
const SPEED: f32 = 220.0;
const JUMP: f32 = 450.0;
/// 掉到这以下回到起点。
const FALL_LINE: f32 = -300.0;
/// 相机在角色后上方多远。
const CAMERA_BACK: Vec3 = Vec3::new(0.0, 220.0, 420.0);
/// 平台左右挪多远、来回一趟几秒。
const PLATFORM_REACH: f32 = 150.0;
const PLATFORM_PERIOD: f32 = 6.0;
/// 绕圈发声体的圆心、半径与一圈秒数。
const ORBIT_CENTER: Vec3 = Vec3::new(0.0, 90.0, -80.0);
const ORBIT_RADIUS: f32 = 260.0;
const ORBIT_PERIOD: f32 = 5.0;
/// 调试时往下打多远、往前扫多远、扫的球多大。
const PROBE_DOWN: f32 = 200.0;
const PROBE_FORWARD: f32 = 120.0;
const PROBE_BALL: f32 = 10.0;

/// 角色的起点（第一次见到时的位置）、朝哪边（停下来时保持）、水下效果器开关。
#[derive(Resource, Debug)]
pub(crate) struct Lab {
    spawn: Option<(Entity, Vec3)>,
    facing: Vec3,
    platform: Option<(Entity, Vec3)>,
    underwater: bool,
}

impl Default for Lab {
    fn default() -> Self {
        Self { spawn: None, facing: Vec3::NEG_Z, platform: None, underwater: false }
    }
}

type Hero<'a> = (Entity, &'a SceneId, &'a mut Transform3d, &'a mut Velocity, &'a CharacterState3d);

/// 键盘走、跳，掉下去回到起点。台阶、斜坡这些网格碰撞体还没生成时角色停在起点（地面是盒子，第一帧就有）。
pub(crate) fn walk(time: Res<Time>, input: Res<Input>, physics: Res<Physics3d>, mut lab: ResMut<Lab>, mut heroes: Query<Hero, Without<Camera3d>>) {
    let Some((entity, _, mut t, mut v, state)) = heroes.iter_mut().find(|(_, id, ..)| id.0 == LAB_HERO) else { return };
    if !(time.delta > 0.0) {
        return;
    }
    // 新的场景（重新进试验场）是新的实体：重新记起点
    let (spawn, new) = match lab.spawn {
        Some((e, at)) if e == entity => (at, false),
        _ => (lab.spawn.insert((entity, t.translation)).1, true),
    };
    // 刚进场的这一帧物理还没数过要等的网格碰撞体：也先停着
    if new || t.translation.y < FALL_LINE || physics.pending_meshes() > 0 {
        t.translation = spawn;
        *v = Velocity::default();
        return;
    }
    let held = |keys: [&str; 2]| keys.iter().any(|k| input.key_down(k));
    let x = f32::from(held(["KeyD", "ArrowRight"])) - f32::from(held(["KeyA", "ArrowLeft"]));
    let z = f32::from(held(["KeyS", "ArrowDown"])) - f32::from(held(["KeyW", "ArrowUp"]));
    let walk = Vec3::new(x, 0.0, z).normalize_or_zero() * SPEED;
    if walk != Vec3::ZERO {
        lab.facing = walk.normalize();
    }
    // 竖直速度归引擎管（重力），着地时按空格往上跳
    let y = if state.grounded && input.key_pressed("Space") { JUMP } else { v.linear.y };
    v.linear = Vec3::new(walk.x, y, walk.z);
}

/// 平台绕着第一次见到的位置左右挪。
pub(crate) fn move_platform(time: Res<Time>, mut lab: ResMut<Lab>, mut platforms: Query<(Entity, &SceneId, &mut Transform3d), Without<Camera3d>>) {
    let Some((entity, _, mut t)) = platforms.iter_mut().find(|(_, id, _)| id.0 == LAB_PLATFORM) else { return };
    let origin = match lab.platform {
        Some((e, at)) if e == entity => at,
        _ => lab.platform.insert((entity, t.translation)).1,
    };
    let phase = (time.elapsed as f32 / PLATFORM_PERIOD) * std::f32::consts::TAU;
    t.translation = origin + Vec3::X * PLATFORM_REACH * phase.sin();
}

/// 相机跟在角色后上方看着它（物理走完以后摆，和这一帧的位置一致）。
pub(crate) fn follow(heroes: Query<(&SceneId, &Transform3d), Without<Camera3d>>, mut cameras: Query<(&mut Camera3d, &mut Transform3d)>) {
    let Some((_, hero)) = heroes.iter().find(|(id, _)| id.0 == LAB_HERO) else { return };
    let target = hero.translation;
    for (mut camera, mut t) in &mut cameras {
        // 单位是 point：默认的近远平面太小
        camera.near = 1.0;
        camera.far = 5000.0;
        *t = Transform3d::from_xyz(target.x + CAMERA_BACK.x, target.y + CAMERA_BACK.y, target.z + CAMERA_BACK.z).looking_at(target, Vec3::Y);
    }
}

/// 调试视图开着时：往下打一条射线、往前扫一个球（`Physics3dDebug` 把它们画出来）。
pub(crate) fn probe(debug: Res<Physics3dDebug>, physics: Res<Physics3d>, lab: Res<Lab>, heroes: Query<(Entity, &SceneId, &Transform3d)>) {
    if !debug.enabled {
        return;
    }
    let Some((entity, _, t)) = heroes.iter().find(|(_, id, _)| id.0 == LAB_HERO) else { return };
    let me = QueryFilter::default().exclude(entity);
    physics.raycast(t.translation, Vec3::NEG_Y, PROBE_DOWN, me);
    physics.cast_shape(QueryShape::Ball(PROBE_BALL), t.translation, Quat::IDENTITY, lab.facing, PROBE_FORWARD, me);
}

/// 绕圈发声体按时间绕场地中心转圈（给 3D 左右声像与多普勒演示用，见 docs/audio-3d.md）。
pub(crate) fn orbit_sound_source(time: Res<Time>, mut orbiters: Query<(&SceneId, &mut Transform3d), Without<Camera3d>>) {
    let Some((_, mut t)) = orbiters.iter_mut().find(|(id, _)| id.0 == LAB_ORBITER) else { return };
    let angle = (time.elapsed as f32 / ORBIT_PERIOD) * std::f32::consts::TAU;
    t.translation = ORBIT_CENTER + Vec3::new(angle.cos() * ORBIT_RADIUS, 0.0, angle.sin() * ORBIT_RADIUS);
}

/// 试验场里按 U 开关「水下」效果器链（低通 500 Hz 加混响），离开试验场时恢复。
pub(crate) fn underwater_effects(input: Res<Input>, mut lab: ResMut<Lab>, mut audio: ResMut<Audio>, ids: Query<&SceneId>) {
    if !ids.iter().any(|id| id.0 == LAB_HERO) {
        if lab.underwater {
            lab.underwater = false;
            audio.set_effects(Bus::Sfx, &[], 0.3);
        }
        return;
    }
    if input.key_pressed(UNDERWATER_KEY) {
        lab.underwater = !lab.underwater;
        if lab.underwater {
            let chain = [
                EffectSpec::Lowpass { cutoff: 500.0, q: std::f32::consts::FRAC_1_SQRT_2 },
                EffectSpec::Reverb { room: 0.7, damping: 0.5, mix: 0.35 },
            ];
            audio.set_effects(Bus::Sfx, &chain, 0.3);
        } else {
            audio.set_effects(Bus::Sfx, &[], 0.3);
        }
    }
}

/// 试验场里按 C 开关级联染色（近到远红、绿、蓝），离开试验场时关掉。
pub(crate) fn cascade_colors(input: Res<Input>, mut shadows: ResMut<Shadows>, ids: Query<&SceneId>) {
    if !ids.iter().any(|id| id.0 == LAB_HERO) {
        if shadows.debug_cascades {
            shadows.debug_cascades = false;
        }
        return;
    }
    if input.key_pressed(CASCADE_KEY) {
        shadows.debug_cascades = !shadows.debug_cascades;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use nge_core::Viewport;
    use nge_ecs::App;
    use nge_physics3d::Collider;
    use nge_render3d::Gizmos3d;
    use serde_json::{Value, json};

    use super::*;

    const VIEWPORT: Viewport = Viewport { size: [800.0, 600.0], pixels: [800, 600] };

    /// lab.ts 拼的场景（Rust 这边按 lab.json 拼一样的）。
    fn scene() -> Value {
        let lab: Value = serde_json::from_str(include_str!("../lab.json")).unwrap();
        let mut models = vec![lab["hero"].clone()];
        models.extend(lab["models"].as_array().unwrap().iter().cloned());
        json!({ "level": "lab3d", "physics3d": { "gravity": [0, -981, 0] }, "models": models, "joints3d": lab["joints3d"] })
    }

    fn run(app: &mut App, frames: usize) {
        for _ in 0..frames {
            app.tick(VIEWPORT, 1.0 / 60.0);
        }
    }

    fn key(app: &mut App, code: &str, down: bool) {
        app.command("key", &json!({ "code": code, "down": down }));
    }

    fn find<C: Component + Clone>(app: &mut App, id: &str) -> Option<C> {
        let world = app.world_mut();
        world.query::<(&SceneId, &C)>().iter(world).find(|(s, _)| s.0 == id).map(|(_, c)| c.clone())
    }

    fn at(app: &mut App, id: &str) -> Transform3d {
        find::<Transform3d>(app, id).unwrap()
    }

    fn place(app: &mut App, id: &str, to: Vec3) {
        let world = app.world_mut();
        let mut t = world.query::<(&SceneId, &mut Transform3d)>().iter_mut(world).find(|(s, _)| s.0 == id).unwrap().1;
        t.translation = to;
    }

    /// 等异步加载（模型在后台线程读）最多多久：按墙钟算，机器忙时多跑几帧也等得到。
    const LOAD_TIMEOUT: Duration = Duration::from_secs(15);

    /// 跑一帧，让一下后台加载的线程。
    fn frame(app: &mut App) {
        run(app, 1);
        std::thread::sleep(Duration::from_millis(1));
    }

    /// 进试验场，等模型加载完、碰撞体都生成、角色站稳。地面是盒子，第一帧就有；网格碰撞体没生成完时角色停在起点：
    /// 整个过程中角色一直没往下掉（不会掉出去再回到起点）。
    fn lab() -> App {
        let mut app = crate::build(nge_audio::AudioPlugin::mock());
        app.prop("scene", Some(&scene()));
        let mut lowest = f32::INFINITY;
        let mut waited = false;
        let deadline = Instant::now() + LOAD_TIMEOUT;
        while Instant::now() < deadline {
            frame(&mut app);
            if let Some(t) = find::<Transform3d>(&mut app, LAB_HERO) {
                lowest = lowest.min(t.translation.y);
            }
            waited |= app.world().resource::<Physics3d>().pending_meshes() > 0;
            let ready = ["lab-floor", "lab-steps", "lab-ramps", "lab-door", "lab-rock"].iter().all(|id| find::<Collider>(&mut app, id).is_some());
            let standing = find::<CharacterState3d>(&mut app, LAB_HERO).is_some_and(|s| s.grounded) && (at(&mut app, LAB_HERO).translation.z - 300.0).abs() < 1.0;
            if ready && standing && app.world().resource::<Physics3d>().pending_meshes() == 0 {
                assert!(lowest > 20.0, "角色进场后一直没往下掉（最低 {lowest}）");
                assert!(waited, "确实等过网格碰撞体");
                return app;
            }
        }
        panic!("试验场没准备好：{:?}", find::<CharacterState3d>(&mut app, LAB_HERO));
    }

    #[test]
    fn the_hero_stays_at_the_start_until_the_mesh_colliders_are_built() {
        let mut app = crate::build(nge_audio::AudioPlugin::mock());
        app.prop("scene", Some(&scene()));
        key(&mut app, "KeyW", true);
        let mut frozen = 0;
        let deadline = Instant::now() + LOAD_TIMEOUT;
        while Instant::now() < deadline {
            frame(&mut app);
            if app.world().resource::<Physics3d>().pending_meshes() == 0 && frozen > 0 {
                break;
            }
            if let Some(t) = find::<Transform3d>(&mut app, LAB_HERO) {
                frozen += 1;
                assert!(t.translation.distance(Vec3::new(0.0, 25.0, 300.0)) < 0.5, "台阶、斜坡还没有碰撞体：按着 W 也不走 {}", t.translation);
            }
        }
        assert!(frozen > 0, "等过网格碰撞体");
        run(&mut app, 30);
        assert!(at(&mut app, LAB_HERO).translation.z < 290.0, "都生成以后照常走");
    }

    #[test]
    fn the_hero_walks_jumps_and_climbs_the_steps() {
        let mut app = lab();
        let start = at(&mut app, LAB_HERO).translation;
        assert!((start.y - 24.0).abs() < 2.0, "站在地上 {start}");
        key(&mut app, "KeyW", true);
        run(&mut app, 30);
        key(&mut app, "KeyW", false);
        let walked = at(&mut app, LAB_HERO).translation;
        assert!(start.z - walked.z > 90.0 && (walked.y - start.y).abs() < 2.0, "W 往 -z 走 {walked}");
        key(&mut app, "Space", true);
        run(&mut app, 1);
        key(&mut app, "Space", false);
        run(&mut app, 12);
        assert!(at(&mut app, LAB_HERO).translation.y > start.y + 50.0, "跳起来 {}", at(&mut app, LAB_HERO).translation);
        run(&mut app, 60);
        assert!(find::<CharacterState3d>(&mut app, LAB_HERO).unwrap().grounded, "落回地上");
        // 走上四级台阶（每级 16 高，角色能迈 20）
        place(&mut app, LAB_HERO, Vec3::new(-300.0, 25.0, 230.0));
        run(&mut app, 10);
        key(&mut app, "KeyW", true);
        let mut highest = 0.0f32;
        for _ in 0..80 {
            run(&mut app, 1);
            highest = highest.max(at(&mut app, LAB_HERO).translation.y);
        }
        assert!((highest - (64.0 + 24.0)).abs() < 2.0, "走上最高一级（64 高）{highest}");
    }

    #[test]
    fn the_hero_pushes_the_door_open_and_falls_back_to_the_start() {
        let mut app = lab();
        let closed = at(&mut app, "lab-door").rotation;
        key(&mut app, "KeyW", true);
        run(&mut app, 200);
        let open = at(&mut app, "lab-door").rotation.angle_between(closed).to_degrees();
        assert!(open > 20.0, "门推开了 {open}°");
        assert!(open < 101.0, "限位 ±100° {open}°");
        // 掉出去回到起点
        key(&mut app, "KeyW", false);
        place(&mut app, LAB_HERO, Vec3::new(2000.0, 25.0, 0.0));
        run(&mut app, 60);
        let back = at(&mut app, LAB_HERO).translation;
        assert!(back.distance(Vec3::new(0.0, 24.0, 300.0)) < 5.0, "回到起点 {back}");
        let state = find::<CharacterState3d>(&mut app, LAB_HERO).unwrap();
        assert!(find::<Velocity>(&mut app, LAB_HERO).unwrap().linear.length() < 1.0 && state.grounded, "回到起点后速度清零、站在地上 {state:?}");
    }

    #[test]
    fn leaving_and_coming_back_starts_fresh() {
        let mut app = lab();
        key(&mut app, "KeyW", true);
        key(&mut app, "Space", true);
        run(&mut app, 5);
        key(&mut app, "KeyW", false);
        key(&mut app, "Space", false);
        assert!(find::<Velocity>(&mut app, LAB_HERO).unwrap().linear.length() > 100.0, "走着、跳着离开");
        // 换到别的关卡再回来：实体整个重建，速度、着地状态、起点都是新的
        app.prop("scene", Some(&json!({ "level": "elsewhere" })));
        run(&mut app, 2);
        app.prop("scene", Some(&scene()));
        run(&mut app, 2);
        let t = at(&mut app, LAB_HERO).translation;
        assert!(t.distance(Vec3::new(0.0, 25.0, 300.0)) < 1.0, "从起点开始 {t}");
        assert!(find::<Velocity>(&mut app, LAB_HERO).unwrap().linear.length() < 20.0, "没带着上次的速度");
    }

    #[test]
    fn q_toggles_the_3d_debug_view_and_the_platform_moves() {
        let mut app = lab();
        let platform = at(&mut app, LAB_PLATFORM).translation;
        key(&mut app, "KeyQ", true);
        run(&mut app, 1);
        key(&mut app, "KeyQ", false);
        assert!(app.world().resource::<Physics3dDebug>().enabled);
        run(&mut app, 1);
        let lines = app.world().resource::<Gizmos3d>().depth_tested().len();
        assert!(lines > 100, "画了碰撞体的轮廓、射线、球 {lines}");
        key(&mut app, "KeyQ", true);
        run(&mut app, 1);
        key(&mut app, "KeyQ", false);
        assert!(!app.world().resource::<Physics3dDebug>().enabled);
        // 平台按正弦来回挪：看这一秒里离开起点最远多远（只比最后一帧的话，可能正好挪回原处附近）
        let mut farthest = 0.0f32;
        for _ in 0..60 {
            run(&mut app, 1);
            farthest = farthest.max(at(&mut app, LAB_PLATFORM).translation.distance(platform));
        }
        assert!(farthest > 10.0, "平台在挪 {farthest}");
    }

    #[test]
    fn c_toggles_cascade_colors_in_the_lab_only() {
        let mut app = lab();
        let press = |app: &mut App| {
            key(app, "KeyC", true);
            run(app, 1);
            key(app, "KeyC", false);
            run(app, 1);
        };
        press(&mut app);
        assert!(app.world().resource::<Shadows>().debug_cascades, "按 C 打开");
        press(&mut app);
        assert!(!app.world().resource::<Shadows>().debug_cascades, "再按关掉");
        press(&mut app);
        app.prop("scene", Some(&json!({ "level": "elsewhere" })));
        run(&mut app, 2);
        assert!(!app.world().resource::<Shadows>().debug_cascades, "离开试验场时关掉");
        press(&mut app);
        assert!(!app.world().resource::<Shadows>().debug_cascades, "试验场外按 C 没反应");
    }
}
