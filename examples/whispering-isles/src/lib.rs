//! 风语群岛（Whispering Isles）：3D 开放世界探索示例。
//! Rust 负责 60/120Hz 的 3D 角色控制器（相机相对移动、冲刺体力、跳跃与滑翔伞）、
//! 可旋转第三人称轨道相机、浮空石台运动与低频遥测回传；TSX 负责世界场景声明、
//! 动态昼夜循环、圣坛唤醒与微光收集、天空指南针与小地图 HUD。

use bevy_ecs::prelude::*;
use nge_assets::{AssetPlugin, AssetServer, Assets, Handle};
use nge_audio::{Audio, AudioPlugin, Play, Sound};
use nge_core::EventKey;
use nge_ecs::glam::{Quat, Vec3};
use nge_ecs::{App, Input, Outbox, PostUpdate, Startup, Time, Update};
use nge_physics3d::{CharacterState3d, Physics3d, Physics3dStepped, QueryFilter, QueryShape, Velocity};
use nge_render2d::Render2dPlugin;
use nge_render3d::{Animator, Camera3d, Render3dPlugin, Shadows, Transform3d};
use nge_scene::{Camera3dRigUpdated, SceneId, ScenePlugin};
use nsh_core::prelude::assets;
use serde::Serialize;

pub const HERO: &str = "explorer";
pub const GLIDER: &str = "glider";
pub const PLAT_1: &str = "sky-plat-1";
pub const PLAT_2: &str = "sky-plat-2";

pub const TELEMETRY: EventKey<Telemetry> = EventKey::new("telemetry");
pub const INTERACT: EventKey<Interact> = EventKey::new("interact");

#[derive(Debug, Clone, Serialize)]
pub struct Telemetry {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub cam_yaw: f32,
    pub stamina: f32,
    pub max_stamina: f32,
    pub gliding: bool,
    pub grounded: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Interact {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

const WALK_SPEED: f32 = 225.0;
const SPRINT_SPEED: f32 = 365.0;
const GLIDE_SPEED: f32 = 310.0;
const JUMP_VEL: f32 = 460.0;
const GLIDE_FALL_CAP: f32 = -115.0;
const FALL_LINE: f32 = -120.0;
const COYOTE_TIME: f32 = 0.14;
const CAM_TURN_SPEED: f32 = 1.9;
const CAM_PRESETS: [(f32, f32); 3] = [(175.0, 360.0), (295.0, 600.0), (115.0, 230.0)];

#[derive(Resource, Debug)]
struct ExplorerState {
    spawn: Option<(Entity, Vec3)>,
    yaw: f32,
    cam_yaw: f32,
    cam_preset: usize,
    cam_pos: Option<Vec3>,
    stamina: f32,
    max_stamina: f32,
    airborne: f32,
    gliding: bool,
    telemetry_acc: f32,
    plat1_origin: Option<(Entity, Vec3)>,
    plat2_origin: Option<(Entity, Vec3)>,
}

impl Default for ExplorerState {
    fn default() -> Self {
        Self {
            spawn: None,
            yaw: 0.0,
            cam_yaw: 0.0,
            cam_preset: 0,
            cam_pos: None,
            stamina: 100.0,
            max_stamina: 100.0,
            airborne: 0.0,
            gliding: false,
            telemetry_acc: 0.0,
            plat1_origin: None,
            plat2_origin: None,
        }
    }
}

#[derive(Resource, Default)]
struct Sfx {
    jump: Option<Handle<Sound>>,
    glide: Option<Handle<Sound>>,
}

fn load_sfx(audio: Res<Audio>, mut server: ResMut<AssetServer>, mut sounds: ResMut<Assets<Sound>>, mut sfx: ResMut<Sfx>) {
    if audio.enabled() {
        sfx.jump = Some(server.load(&mut sounds, "asset:/@whispering-isles/jump.wav"));
        sfx.glide = Some(server.load(&mut sounds, "asset:/@whispering-isles/glide.wav"));
    }
}

fn play_sfx(audio: &mut Audio, handle: &Option<Handle<Sound>>, volume: f32) {
    if let Some(h) = handle {
        audio.play(*h, Play { volume, ..Play::default() });
    }
}

type HeroQuery<'a> = (Entity, &'a SceneId, &'a mut Transform3d, &'a mut Velocity, &'a CharacterState3d, Option<&'a mut Animator>);

#[allow(clippy::too_many_arguments)]
fn control_explorer(
    time: Res<Time>,
    input: Res<Input>,
    physics: Res<Physics3d>,
    mut st: ResMut<ExplorerState>,
    mut heroes: Query<HeroQuery, Without<Camera3d>>,
    mut outbox: ResMut<Outbox>,
    mut audio: ResMut<Audio>,
    sfx: Res<Sfx>,
) {
    let Some((entity, _, mut t, mut v, cstate, animator)) = heroes.iter_mut().find(|(_, id, ..)| id.0 == HERO) else { return };
    let dt = time.delta;
    if !(dt > 0.0) {
        return;
    }

    let (spawn, is_new) = match st.spawn {
        Some((e, at)) if e == entity => (at, false),
        _ => (st.spawn.insert((entity, t.translation)).1, true),
    };

    // 地形网格碰撞体仍在后台构建时，角色留在起点等待
    if is_new || t.translation.y < FALL_LINE || physics.pending_meshes() > 0 {
        t.translation = spawn;
        *v = Velocity::default();
        st.gliding = false;
        return;
    }

    // 旋转第三人称视角（Q/E 或 J/L 或手柄肩键）
    let turn_left = input.key_down("KeyQ") || input.key_down("KeyJ") || input.key_down("GamepadLeftShoulder");
    let turn_right = input.key_down("KeyE") || input.key_down("KeyL") || input.key_down("GamepadRightShoulder");
    let turn = f32::from(turn_left) - f32::from(turn_right);
    st.cam_yaw += turn * CAM_TURN_SPEED * dt;

    if input.key_pressed("KeyV") {
        st.cam_preset = (st.cam_preset + 1) % CAM_PRESETS.len();
    }

    // 交互键（F / Enter / 手柄 X）
    if input.key_pressed("KeyF") || input.key_pressed("Enter") || input.key_pressed("GamepadX") {
        outbox.send(INTERACT, &Interact { x: t.translation.x, y: t.translation.y, z: t.translation.z });
    }

    let held = |keys: &[&str]| keys.iter().any(|k| input.key_down(k));
    let raw_x = f32::from(held(&["KeyD", "ArrowRight", "GamepadRight", "GamepadLeftStickRight"]))
        - f32::from(held(&["KeyA", "ArrowLeft", "GamepadLeft", "GamepadLeftStickLeft"]));
    let raw_z = f32::from(held(&["KeyS", "ArrowDown", "GamepadDown", "GamepadLeftStickDown"]))
        - f32::from(held(&["KeyW", "ArrowUp", "GamepadUp", "GamepadLeftStickUp"]));

    // 把输入转换到相机朝向的世界 XZ 平面
    let forward = Vec3::new(-st.cam_yaw.sin(), 0.0, -st.cam_yaw.cos());
    let right = Vec3::new(st.cam_yaw.cos(), 0.0, -st.cam_yaw.sin());
    let move_dir = (right * raw_x - forward * raw_z).normalize_or_zero();

    if cstate.grounded {
        st.airborne = 0.0;
        st.gliding = false;
    } else {
        st.airborne += dt;
    }

    let sprinting = (input.key_down("ShiftLeft") || input.key_down("ShiftRight") || input.key_down("GamepadB")) && move_dir != Vec3::ZERO && st.stamina > 1.0 && !st.gliding;
    let jump_pressed = input.key_pressed("Space") || input.key_pressed("GamepadA");
    let jump_held = input.key_down("Space") || input.key_down("GamepadA");

    let mut vy = v.linear.y;
    if jump_pressed && st.airborne <= COYOTE_TIME {
        vy = JUMP_VEL;
        st.airborne = COYOTE_TIME + 1.0;
        play_sfx(&mut audio, &sfx.jump, 0.45);
    } else if !cstate.grounded && jump_held && vy < 0.0 && st.stamina > 2.0 {
        if !st.gliding {
            st.gliding = true;
            play_sfx(&mut audio, &sfx.glide, 0.45);
        }
        if vy < GLIDE_FALL_CAP {
            vy = GLIDE_FALL_CAP;
        }
    } else if !jump_held || st.stamina <= 0.5 {
        st.gliding = false;
    }

    // 体力消耗与恢复
    if st.gliding {
        st.stamina = (st.stamina - 14.0 * dt).max(0.0);
    } else if sprinting {
        st.stamina = (st.stamina - 24.0 * dt).max(0.0);
    } else if cstate.grounded {
        st.stamina = (st.stamina + 34.0 * dt).min(st.max_stamina);
    }

    let speed = if st.gliding {
        GLIDE_SPEED
    } else if sprinting {
        SPRINT_SPEED
    } else {
        WALK_SPEED
    };
    let horiz = move_dir * speed;
    v.linear = Vec3::new(horiz.x, vy, horiz.z);

    // 角色平滑转向移动方向（模型正面朝 -Z）
    if move_dir != Vec3::ZERO {
        let target_yaw = (-move_dir.x).atan2(-move_dir.z);
        let mut diff = target_yaw - st.yaw;
        while diff > std::f32::consts::PI {
            diff -= std::f32::consts::TAU;
        }
        while diff < -std::f32::consts::PI {
            diff += std::f32::consts::TAU;
        }
        st.yaw += diff * (12.0 * dt).min(1.0);
    }
    t.rotation = Quat::from_rotation_y(st.yaw);

    // 切换角色待机 / 行走动画（带平滑淡入淡出）
    if let Some(mut anim) = animator {
        let moving = move_dir != Vec3::ZERO && cstate.grounded;
        let clip = if moving { "walk" } else { "idle" };
        if anim.clip != clip {
            anim.play(clip, 0.16);
        }
        anim.speed = if moving && sprinting { 1.55 } else { 1.0 };
    }

    // 定期向 TSX 回传位置与状态（指南针、小地图、体力条与圣坛交互提示）
    st.telemetry_acc += dt;
    if st.telemetry_acc >= 0.06 {
        st.telemetry_acc = 0.0;
        outbox.send(
            TELEMETRY,
            &Telemetry {
                x: t.translation.x,
                y: t.translation.y,
                z: t.translation.z,
                yaw: st.yaw,
                cam_yaw: st.cam_yaw,
                stamina: st.stamina,
                max_stamina: st.max_stamina,
                gliding: st.gliding,
                grounded: cstate.grounded,
            },
        );
    }
}

/// 浮空石台沿垂直与水平轨道往返，载着玩家登上星穹峰顶。
fn move_platforms(time: Res<Time>, mut st: ResMut<ExplorerState>, mut q: Query<(Entity, &SceneId, &mut Transform3d), Without<Camera3d>>) {
    let elapsed = time.elapsed as f32;
    for (entity, id, mut t) in &mut q {
        if id.0 == PLAT_1 {
            let origin = match st.plat1_origin {
                Some((e, at)) if e == entity => at,
                _ => st.plat1_origin.insert((entity, t.translation)).1,
            };
            let p = (elapsed / 7.0) * std::f32::consts::TAU;
            t.translation = origin + Vec3::new(0.0, 32.0 * p.sin(), 60.0 * p.cos());
        } else if id.0 == PLAT_2 {
            let origin = match st.plat2_origin {
                Some((e, at)) if e == entity => at,
                _ => st.plat2_origin.insert((entity, t.translation)).1,
            };
            let p = (elapsed / 8.0) * std::f32::consts::TAU;
            t.translation = origin + Vec3::new(55.0 * p.sin(), 40.0 * p.cos(), 0.0);
        }
    }
}

/// 物理步进后：让滑翔伞贴在角色头顶，并平滑更新带防穿墙检测的第三人称轨道相机（排在 Camera3dRigUpdated 之前，自动叠加 3D 震屏）。
fn follow_camera_and_glider(
    time: Res<Time>,
    physics: Res<Physics3d>,
    mut st: ResMut<ExplorerState>,
    mut transforms: Query<(Entity, &SceneId, &mut Transform3d), Without<Camera3d>>,
    mut cameras: Query<(&mut Camera3d, &mut Transform3d)>,
) {
    let Some((hero_entity, hero_pos, hero_rot)) = transforms.iter().find(|(_, id, _)| id.0 == HERO).map(|(e, _, t)| (e, t.translation, t.rotation)) else { return };

    if let Some((_, _, mut gt)) = transforms.iter_mut().find(|(_, id, _)| id.0 == GLIDER) {
        if st.gliding {
            gt.translation = hero_pos + Vec3::Y * 32.0;
            gt.rotation = hero_rot;
        } else {
            gt.translation = Vec3::new(0.0, -600.0, 0.0);
        }
    }

    let look_at = hero_pos + Vec3::Y * 26.0;
    let (height, dist) = CAM_PRESETS[st.cam_preset];
    let raw_offset = Vec3::new(st.cam_yaw.sin() * dist, height - 26.0, st.cam_yaw.cos() * dist);
    let mut arm_len = raw_offset.length();
    let arm_dir = raw_offset / arm_len.max(1e-4);
    if let Some(hit) = physics.cast_shape(QueryShape::Ball(5.0), look_at, Quat::IDENTITY, arm_dir, arm_len, QueryFilter::default().exclude(hero_entity)) {
        arm_len = (hit.distance - 3.0).max(24.0);
    }
    let desired = look_at + arm_dir * arm_len;
    let alpha = 1.0 - (-12.0 * time.delta).exp();
    let cam_pos = match st.cam_pos {
        Some(prev) => prev.lerp(desired, alpha),
        None => desired,
    };
    st.cam_pos = Some(cam_pos);

    for (mut cam, mut ct) in &mut cameras {
        cam.near = 2.0;
        cam.far = 9000.0;
        *ct = Transform3d::from_xyz(cam_pos.x, cam_pos.y, cam_pos.z).looking_at(look_at, Vec3::Y);
    }
}

fn toggle_debug(input: Res<Input>, mut shadows: ResMut<Shadows>, mut perf: ResMut<nge_text::PerfOverlay>) {
    if input.key_pressed("KeyC") {
        shadows.debug_cascades = !shadows.debug_cascades;
    }
    if input.key_pressed("KeyP") {
        perf.0 = !perf.0;
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
        .add_event(TELEMETRY)
        .add_event(INTERACT)
        .init_resource::<ExplorerState>()
        .init_resource::<Sfx>()
        .add_systems(Startup, load_sfx)
        .add_systems(Update, (control_explorer, move_platforms, toggle_debug).chain())
        .add_systems(PostUpdate, follow_camera_and_glider.after(Physics3dStepped).before(Camera3dRigUpdated));
    app
}

pub fn factory() -> nge_core::GameFactory {
    nge_ecs::factory(app)
}

#[nsh_core::prelude::native_module("whispering-isles/native")]
fn native(registry: &mut nsh_core::prelude::Registry) {
    registry.assets(assets!());
    #[cfg(target_arch = "wasm32")]
    nge_platform::web::register(factory(), assets!().iter().map(|f| f.uri));
    #[cfg(target_os = "android")]
    nge_platform::android::register(factory());
}

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

#[cfg(test)]
mod tests {
    use nge_core::Viewport;
    use nge_render3d::{Assets3d, Image};
    use serde_json::json;

    use super::*;

    const VIEWPORT: Viewport = Viewport { size: [1024.0, 640.0], pixels: [1024, 640] };
    const GLBS: [&str; 16] = [
        "terrain", "water", "explorer", "staff", "wisp", "glider", "pine", "ruin_arch", "ruin_pillar",
        "ruin_gate", "shrine_dormant", "shrine_awake", "relic", "campfire", "platform", "boulder",
    ];
    const WAVS: [&str; 5] = ["jump", "glide", "shrine", "relic", "time"];

    #[test]
    fn assets_are_bundled() {
        let uris: Vec<_> = assets!().iter().map(|f| f.uri).collect();
        for name in GLBS {
            let uri = format!("asset:/@whispering-isles/{name}.glb");
            assert!(uris.contains(&uri.as_str()), "缺少 3D 模型 {uri}");
        }
        for name in WAVS {
            let uri = format!("asset:/@whispering-isles/{name}.wav");
            assert!(uris.contains(&uri.as_str()), "缺少音效 {uri}");
        }
        assert!(uris.contains(&"asset:/@whispering-isles/crate.glb"));
    }

    #[test]
    fn key_3d_models_load() {
        let mut app = App::new();
        app.add_plugins(Render3dPlugin);
        let bytes: [&[u8]; 5] = [
            include_bytes!("../assets/terrain.glb"),
            include_bytes!("../assets/explorer.glb"),
            include_bytes!("../assets/glider.glb"),
            include_bytes!("../assets/shrine_awake.glb"),
            include_bytes!("../assets/ruin_gate.glb"),
        ];
        for b in bytes {
            app.world_mut()
                .resource_scope(|world, mut assets: Mut<Assets3d>| {
                    assets.load_gltf(&mut world.resource_mut::<Assets<Image>>(), b)
                })
                .expect("GLB 模型加载失败");
        }
    }

    #[test]
    fn world_scene_ticks_and_emits_telemetry() {
        let mut app = build(AudioPlugin::mock());
        let scene = json!({
            "background": "#5aa0e0",
            "camera3d": { "x": 0, "y": 240, "z": 620, "target": [0, 30, 260], "fov": 55, "shadowCascades": 3, "shadowDistance": 1800 },
            "sky": { "top": "#2f6ec4", "horizon": "#cde4f7", "ground": "#465842", "sunSize": 2.2 },
            "fog": { "start": 900, "end": 3600, "color": "#cde4f7", "sky": 0.65, "max": 0.85 },
            "physics3d": { "gravity": [0, -981, 0] },
            "models": [
                { "id": "floor", "model": "asset:/@whispering-isles/platform.glb", "x": 0, "y": -10, "z": 285, "body": { "kind": "fixed", "shape": "box", "size": [400, 20, 400] } },
                { "id": HERO, "model": "asset:/@whispering-isles/explorer.glb", "x": 0, "y": 30, "z": 285, "body": { "kind": "character", "shape": "capsule", "r": 12, "h": 24, "slope": 48, "step": 22, "snap": 10 } },
                { "id": GLIDER, "model": "asset:/@whispering-isles/glider.glb", "x": 0, "y": -600, "z": 0 }
            ]
        });
        app.prop("scene", Some(&scene));
        let mut saw_telemetry = false;
        for _ in 0..15 {
            let events = app.tick(VIEWPORT, 1.0 / 60.0);
            if events.iter().any(|(n, _)| n == "telemetry") {
                saw_telemetry = true;
            }
        }
        assert!(saw_telemetry, "应回传角色遥测给 TSX");
    }
}
