//! 场景：glTF 箱子在地面上自转，拖动画面绕着看。

use nge_ecs::prelude::*;
use nge_render3d::prelude::*;
use serde::{Deserialize, Serialize};

/// prop `paused`：停止自转。
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Paused(pub bool);

pub const PAUSED: PropKey<Paused> = PropKey::new("paused");

/// 绕 `TARGET` 转的相机：`yaw` 绕 y 轴，`pitch` 是仰角（弧度）。
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    /// 正在拖动的手指和它上一帧的位置。
    drag: Option<(u32, Vec2)>,
}

impl Default for Orbit {
    fn default() -> Self {
        Self { yaw: 0.6, pitch: 0.45, distance: 4.5, drag: None }
    }
}

const TARGET: Vec3 = Vec3::new(0.0, 0.6, 0.0);
/// 每拖动一个 point 转多少弧度。
const DRAG_SPEED: f32 = 0.01;
const PITCH_RANGE: (f32, f32) = (0.05, 1.4);
/// 自转速度（弧度 / 秒）。
const SPIN_SPEED: f32 = 0.6;

impl Orbit {
    pub fn eye(&self) -> Transform3d {
        let dir = Vec3::new(self.pitch.cos() * self.yaw.sin(), self.pitch.sin(), self.pitch.cos() * self.yaw.cos());
        Transform3d { translation: TARGET + dir * self.distance, ..Default::default() }.looking_at(TARGET, Vec3::Y)
    }
}

#[derive(Component)]
struct Spinner;

pub fn app() -> App {
    let mut app = App::new();
    app.add_plugins(Render3dPlugin)
        .add_prop(PAUSED)
        .init_resource::<Orbit>()
        .insert_resource(ClearColor(nge_core::wgpu::Color { r: 0.02, g: 0.03, b: 0.06, a: 1.0 }))
        .add_systems(Startup, setup)
        .add_systems(Update, (orbit, spin));
    app
}

fn setup(mut commands: Commands, mut assets: ResMut<Assets3d>) {
    let model = assets.load_gltf(include_bytes!("../assets/crate.glb")).expect("内置模型");
    let ground = assets.add_mesh(Mesh::plane(8.0)).expect("内置网格");
    let grass = assets.add_material(Material { base_color: Vec4::new(0.2, 0.35, 0.2, 1.0), roughness: 0.9, ..Default::default() });
    commands.spawn((Model3d(model), Spinner, Transform3d::from_xyz(0.0, 0.5, 0.0)));
    commands.spawn((Mesh3d { mesh: ground, material: grass }, Transform3d::default()));
    commands.spawn((Camera3d::default(), Orbit::default().eye()));
    commands.spawn(DirectionalLight { direction: Vec3::new(-1.0, -2.0, -1.5), ..Default::default() });
    commands.spawn((PointLight { color: Vec3::new(1.0, 0.8, 0.5), intensity: 6.0, range: 8.0 }, Transform3d::from_xyz(-1.5, 1.5, 1.5)));
}

/// 一根手指拖动：左右转 yaw，上下转 pitch。
fn orbit(input: Res<Input>, mut orbit: ResMut<Orbit>, mut camera: Query<&mut Transform3d, With<Camera3d>>) {
    let drag = match orbit.drag {
        Some((id, last)) => input.pointers_down().find(|p| p.id == id).map(|p| (id, last, p.screen)),
        None => input.pointers_pressed().next().map(|p| (p.id, p.screen, p.screen)),
    };
    let Some((id, last, now)) = drag else {
        orbit.drag = None;
        return;
    };
    let delta = now - last;
    orbit.drag = Some((id, now));
    if delta == Vec2::ZERO {
        return;
    }
    orbit.yaw -= delta.x * DRAG_SPEED;
    orbit.pitch = (orbit.pitch + delta.y * DRAG_SPEED).clamp(PITCH_RANGE.0, PITCH_RANGE.1);
    for mut t in &mut camera {
        *t = orbit.eye();
    }
}

fn spin(time: Res<Time>, paused: Res<Paused>, mut q: Query<&mut Transform3d, With<Spinner>>) {
    if paused.0 {
        return;
    }
    for mut t in &mut q {
        t.rotation = (Quat::from_rotation_y(SPIN_SPEED * time.delta) * t.rotation).normalize();
    }
}

#[cfg(test)]
mod tests {
    use nge_core::Viewport;
    use serde_json::json;

    use super::*;

    const VIEWPORT: Viewport = Viewport { size: [300.0, 300.0], pixels: [300, 300] };

    fn spinner_rotation(app: &mut App) -> Quat {
        let world = app.world_mut();
        world.query_filtered::<&Transform3d, With<Spinner>>().single(world).unwrap().rotation
    }

    fn camera(app: &mut App) -> Transform3d {
        let world = app.world_mut();
        *world.query_filtered::<&Transform3d, With<Camera3d>>().single(world).unwrap()
    }

    #[test]
    fn orbit_eye_looks_at_target_from_distance() {
        let o = Orbit::default();
        let eye = o.eye();
        assert!(((eye.translation - TARGET).length() - o.distance).abs() < 1e-4);
        let forward = eye.rotation * Vec3::NEG_Z;
        assert!((forward - (TARGET - eye.translation).normalize()).length() < 1e-4);
    }

    #[test]
    fn spins_until_paused() {
        let mut app = app();
        app.tick(VIEWPORT, 0.5);
        let a = spinner_rotation(&mut app);
        assert!(a.angle_between(Quat::IDENTITY) > 0.1);
        app.prop("paused", Some(&json!(true)));
        app.tick(VIEWPORT, 0.5);
        assert_eq!(spinner_rotation(&mut app), a);
    }

    #[test]
    fn dragging_orbits_the_camera_and_clamps_pitch() {
        let mut app = app();
        app.tick(VIEWPORT, 0.016);
        let before = camera(&mut app);
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 100, "y": 100 }));
        app.tick(VIEWPORT, 0.016);
        app.command("touch", &json!({ "phase": "move", "id": 1, "x": 150, "y": 100 }));
        app.tick(VIEWPORT, 0.016);
        let o = *app.world().resource::<Orbit>();
        assert!((o.yaw - (Orbit::default().yaw - 50.0 * DRAG_SPEED)).abs() < 1e-5, "向右拖，相机向左绕 {}", o.yaw);
        assert_ne!(camera(&mut app), before);

        // 一直往下拖：仰角卡在上限
        app.command("touch", &json!({ "phase": "move", "id": 1, "x": 150, "y": 10000 }));
        app.tick(VIEWPORT, 0.016);
        assert_eq!(app.world().resource::<Orbit>().pitch, PITCH_RANGE.1);

        // 松手后再动不影响
        app.command("touch", &json!({ "phase": "end", "id": 1, "x": 150, "y": 10000 }));
        app.tick(VIEWPORT, 0.016);
        let after = *app.world().resource::<Orbit>();
        app.command("touch", &json!({ "phase": "move", "id": 1, "x": 0, "y": 0 }));
        app.tick(VIEWPORT, 0.016);
        assert_eq!(*app.world().resource::<Orbit>(), after);
    }
}
