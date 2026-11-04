//! 场景：左边 glTF 箱子、右边贴着 tile.png 的立方体，左下角同一张 tile.png 的 2D 精灵；点击炸粒子、飘字。

use std::f32::consts::{FRAC_PI_2, PI};

use nge_assets::{AssetPlugin, AssetServer};
use nge_ecs::prelude::*;
use nge_particles::prelude::*;
use nge_render2d::{Camera2d, Render2dPlugin, Sprite, SpriteImage, TextureId, Transform2d};
use nge_render3d::Gltf;
use nge_render3d::prelude::*;
use nge_text::prelude::*;
use serde::{Deserialize, Serialize};

pub const TILE: &str = "asset:/@effects/tile.png";
pub const SPARK: &str = "asset:/@effects/spark.png";
pub const FONT: &str = "asset:/@effects/Inter-Bold.ttf";
pub const CRATE: &str = "asset:/@effects/crate.glb";

/// 发给 HUD 的状态。
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// 还有资源没加载完。
    pub loading: bool,
    pub particles: u32,
}

/// prop `stress`：开一个 10k 粒子的发射器。
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Stress(pub bool);

pub const STATUS: StateKey<Status> = StateKey::new("status");
pub const STRESS: PropKey<Stress> = PropKey::new("stress");

/// 压力测试发射器的发射速度：寿命 1 秒左右，存活数被上限卡在 10k。
const STRESS_RATE: f32 = 12_000.0;
const BURST: u32 = 300;
const FLOAT_SECONDS: f32 = 1.0;
const FLOAT_SPEED: f32 = 60.0;
const SPIN_SPEED: f32 = 0.6;
const BADGE_SIZE: f32 = 72.0;
const BADGE_MARGIN: f32 = 16.0;

#[derive(Resource, Clone, Copy)]
struct Handles {
    spark: TextureId,
    font: FontId,
}

#[derive(Component)]
struct Spinner;

/// 左下角的 2D 精灵（和立方体同一张贴图）。
#[derive(Component)]
struct Badge;

#[derive(Component)]
struct StressEmitter;

/// 往上飘、渐渐消失的文字。
#[derive(Component, Default)]
struct Floater {
    age: f32,
}

/// 点击次数，用来编飘字。
#[derive(Resource, Default)]
struct Taps(u32);

/// `assets` 是 `assets!()` 列出的 (uri, 源文件路径)，`cargo run` 时从源文件读。
pub fn app<'a>(assets: impl IntoIterator<Item = (&'a str, &'a str)>) -> App {
    with_assets(AssetPlugin::new().files(assets))
}

pub fn with_assets(assets: AssetPlugin) -> App {
    let mut app = App::new();
    app.add_plugins(assets)
        .add_plugins(Render3dPlugin)
        .add_plugins(Render2dPlugin)
        .add_plugins(ParticlePlugin)
        .add_plugins(TextPlugin)
        .add_prop(STRESS)
        .insert_resource(Status { loading: true, particles: 0 })
        .add_state(STATUS)
        .init_resource::<Taps>()
        .insert_resource(ClearColor(nge_core::wgpu::Color { r: 0.02, g: 0.03, b: 0.06, a: 1.0 }))
        .add_systems(Startup, setup)
        .add_systems(Update, (tap, float_text, spin, place_badge, stress))
        .add_systems(PostUpdate, status.after(nge_particles::ParticlesSimulated));
    app
}

fn setup(world: &mut World) {
    let (tile, spark, font, model) = world.resource_scope(|world, mut server: Mut<AssetServer>| {
        let tile = server.load(&mut world.resource_mut::<Assets<Image>>(), TILE);
        let spark = server.load(&mut world.resource_mut::<Assets<Image>>(), SPARK);
        let font = server.load(&mut world.resource_mut::<Assets<Font>>(), FONT);
        let model = server.load(&mut world.resource_mut::<Assets<Gltf>>(), CRATE);
        (tile, spark, font, model)
    });
    world.insert_resource(Handles { spark, font });

    let mut assets = world.resource_mut::<Assets3d>();
    let cube = assets.add_mesh(Mesh::cube(1.0)).expect("内置网格");
    let tiled = assets.add_material(Material { base_color_texture: Some(tile), roughness: 0.6, ..Default::default() });

    world.spawn((GltfModel(model), Spinner, Transform3d::from_xyz(-1.0, 0.0, 0.0)));
    world.spawn((Mesh3d { mesh: cube, material: tiled }, Spinner, Transform3d::from_xyz(1.0, 0.0, 0.0)));
    world.spawn((Camera3d::default(), Transform3d::from_xyz(0.0, 1.2, 4.0).looking_at(Vec3::ZERO, Vec3::Y)));
    world.spawn(DirectionalLight { direction: Vec3::new(-1.0, -2.0, -1.5), ..Default::default() });

    world.spawn(Camera2d::default());
    world.spawn((Badge, Sprite::from_texture(tile).with_size(Vec2::splat(BADGE_SIZE)), Transform2d::default().with_z(1.0)));
    world.spawn((
        Text2d::new("tap to burst").with_font(font).with_size(18.0).with_anchor(Anchor::Top),
        Transform2d::default(),
        ScreenText,
        Hint,
    ));
}

/// 顶部居中的提示（屏幕坐标）。
#[derive(Component)]
struct Hint;

fn place_badge(screen: Res<Screen>, mut badge: Query<&mut Transform2d, With<Badge>>, mut hint: Query<&mut Transform2d, (With<Hint>, Without<Badge>)>) {
    let half = screen.size / 2.0;
    for mut t in &mut badge {
        t.translation = Vec2::new(-half.x, -half.y) + Vec2::splat(BADGE_MARGIN + BADGE_SIZE / 2.0);
    }
    for mut t in &mut hint {
        t.translation = Vec2::new(half.x, BADGE_MARGIN);
    }
}

fn spin(time: Res<Time>, mut q: Query<&mut Transform3d, With<Spinner>>, mut badge: Query<&mut Transform2d, With<Badge>>) {
    for mut t in &mut q {
        t.rotation = (Quat::from_rotation_y(SPIN_SPEED * time.delta) * t.rotation).normalize();
    }
    // 2D 精灵跟着转，方便和立方体对照
    let angle = (time.elapsed as f32 * SPIN_SPEED).rem_euclid(2.0 * PI);
    for mut t in &mut badge {
        t.rotation = angle;
    }
}

fn tap(mut commands: Commands, input: Res<Input>, handles: Res<Handles>, mut taps: ResMut<Taps>) {
    for p in input.pointers_pressed() {
        taps.0 += 1;
        let emitter = ParticleEmitter {
            lifetime: (0.5, 1.0),
            speed: (80.0, 260.0),
            gravity: Vec2::new(0.0, -300.0),
            size: (14.0, 2.0),
            color: (Vec4::new(1.0, 0.75, 0.3, 1.0), Vec4::new(1.0, 0.2, 0.05, 0.0)),
            texture: SpriteImage::Texture(handles.spark),
            ..ParticleEmitter::burst(BURST)
        };
        commands.spawn((emitter, Transform2d::from_xy(p.world.x, p.world.y).with_z(10.0)));
        commands.spawn((
            Floater::default(),
            Text2d::new(format!("BOOM x{}", taps.0)).with_font(handles.font).with_size(24.0),
            Transform2d::from_xy(p.world.x, p.world.y + 24.0),
        ));
    }
}

fn float_text(mut commands: Commands, time: Res<Time>, mut q: Query<(Entity, &mut Floater, &mut Transform2d, &mut Text2d)>) {
    for (entity, mut floater, mut t, mut text) in &mut q {
        floater.age += time.delta;
        if floater.age >= FLOAT_SECONDS {
            commands.entity(entity).despawn();
            continue;
        }
        t.translation.y += FLOAT_SPEED * time.delta;
        text.color.w = 1.0 - floater.age / FLOAT_SECONDS;
    }
}

fn stress(mut commands: Commands, stress: Res<Stress>, handles: Res<Handles>, emitters: Query<Entity, With<StressEmitter>>) {
    match (stress.0, emitters.iter().next()) {
        (true, None) => {
            let emitter = ParticleEmitter {
                lifetime: (0.8, 1.2),
                speed: (60.0, 320.0),
                angle: (FRAC_PI_2 - 0.6, FRAC_PI_2 + 0.6),
                gravity: Vec2::new(0.0, -200.0),
                size: (6.0, 1.0),
                color: (Vec4::new(0.4, 0.8, 1.0, 1.0), Vec4::new(0.2, 0.3, 1.0, 0.0)),
                texture: SpriteImage::Texture(handles.spark),
                ..ParticleEmitter::continuous(STRESS_RATE)
            };
            commands.spawn((StressEmitter, emitter, Transform2d::default().with_z(5.0)));
        }
        (false, Some(entity)) => commands.entity(entity).despawn(),
        _ => {}
    }
}

fn status(server: Res<AssetServer>, particles: Query<&Particles>, mut status: ResMut<Status>) {
    let next = Status { loading: server.pending() > 0, particles: particles.iter().map(Particles::len).sum::<usize>() as u32 };
    if *status != next {
        *status = next;
    }
}

#[cfg(test)]
mod tests {
    use nge_core::Viewport;
    use nge_render3d::Model3d;
    use serde_json::json;

    use super::*;

    const VIEWPORT: Viewport = Viewport { size: [400.0, 300.0], pixels: [400, 300] };

    fn files() -> Vec<(String, String)> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");
        ["tile.png", "spark.png", "Inter-Bold.ttf", "crate.glb"]
            .into_iter()
            .map(|f| (format!("asset:/@effects/{f}"), format!("{dir}/{f}")))
            .collect()
    }

    fn loaded_app() -> App {
        let files = files();
        let mut app = app(files.iter().map(|(u, p)| (u.as_str(), p.as_str())));
        for _ in 0..500 {
            app.tick(VIEWPORT, 0.016);
            if !app.world().resource::<Status>().loading {
                return app;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("资源一直没加载完");
    }

    #[test]
    fn loads_everything_and_shares_the_tile() {
        let mut app = loaded_app();
        let world = app.world_mut();
        let handles = *world.resource::<Handles>();
        // 同一个 uri 再加载一次拿到同一个句柄
        let tile = world.resource_scope(|world, mut server: Mut<AssetServer>| server.load(&mut world.resource_mut::<Assets<Image>>(), TILE));
        assert!(world.resource::<Assets<Image>>().get(tile).is_some());
        assert!(world.resource::<Assets<Font>>().get(handles.font).is_some());
        let badge = world.query_filtered::<&Sprite, With<Badge>>().single(world).unwrap().image;
        assert_eq!(badge, SpriteImage::Texture(tile));
        let materials: Vec<_> = world.query::<&Mesh3d>().iter(world).map(|m| m.material).collect();
        let assets = world.resource::<Assets3d>();
        assert!(materials.iter().any(|&m| assets.material(m).base_color_texture == Some(tile)), "立方体用同一张贴图");
        assert_eq!(world.query_filtered::<(), (With<GltfModel>, With<Model3d>)>().iter(world).count(), 1, "glTF 展开成模型");
    }

    #[test]
    fn badge_spins_with_the_cube() {
        let mut app = loaded_app();
        for _ in 0..30 {
            app.tick(VIEWPORT, 0.016);
        }
        let world = app.world_mut();
        let badge = *world.query_filtered::<&Transform2d, With<Badge>>().single(world).unwrap();
        assert!(badge.rotation > 0.1, "{}", badge.rotation);
        assert_eq!(badge.translation, Vec2::new(-200.0, -150.0) + Vec2::splat(BADGE_MARGIN + BADGE_SIZE / 2.0));
    }

    #[test]
    fn tap_bursts_particles_and_floats_text() {
        let mut app = loaded_app();
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 200, "y": 150 }));
        app.tick(VIEWPORT, 0.016);
        assert_eq!(app.world().resource::<Status>().particles, BURST);
        let world = app.world_mut();
        let (text, t) = world.query_filtered::<(&Text2d, &Transform2d), With<Floater>>().single(world).unwrap();
        assert_eq!(text.text, "BOOM x1");
        assert_eq!(t.translation, Vec2::new(0.0, 24.0), "视图中心是世界原点");

        app.command("touch", &json!({ "phase": "end", "id": 1, "x": 200, "y": 150 }));
        for _ in 0..30 {
            app.tick(VIEWPORT, 0.05);
        }
        let world = app.world_mut();
        assert_eq!(world.query::<&Floater>().iter(world).count(), 0, "飘字一秒后消失");
        assert_eq!(world.query::<&ParticleEmitter>().iter(world).count(), 0, "爆炸发射器死完后删除");
        assert_eq!(app.world().resource::<Status>().particles, 0);
    }

    #[test]
    fn stress_keeps_ten_thousand_particles() {
        let mut app = loaded_app();
        app.prop("stress", Some(&json!(true)));
        for _ in 0..60 {
            app.tick(VIEWPORT, 0.016);
        }
        assert_eq!(app.world().resource::<Status>().particles, nge_particles::DEFAULT_MAX_PARTICLES as u32);
        app.prop("stress", Some(&json!(false)));
        app.tick(VIEWPORT, 0.016);
        app.tick(VIEWPORT, 0.016);
        assert_eq!(app.world().resource::<Status>().particles, 0);
    }
}
