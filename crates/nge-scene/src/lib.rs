//! 声明式场景：TSX 通过 `scene` prop 描述精灵、文字、粒子、3D 模型和灯光，这里按 id 增量同步成 ECS 实体；
//! 一次性的效果（炸粒子、飘字、推刚体、音效、补间、震屏）走 `burst` / `floatText` / `push` / `sound` / `tween` / `shake` command。
//!
//! 节点内容没变的实体不会被改动，所以 Rust 系统可以按 [`SceneId`] 找到实体继续驱动它；
//! 节点一旦在 TSX 里改了，就以 TSX 声明的为准整体覆盖。

mod ai;
mod anim;
mod anim_graph;
mod attach;
mod audio;
mod behave;
mod behavior;
mod camera;
#[cfg(not(all(feature = "physics3d", feature = "ai", feature = "net")))]
mod disabled;
mod effects;
mod joints;
mod joints3d;
mod lenient;
mod level;
mod match2d;
mod net;
mod particles3d;
mod nodes;
mod physics;
mod physics3d;
mod protocol;
#[cfg_attr(not(feature = "physics3d"), allow(dead_code))]
mod query3d;
mod resolve;
mod sync;
mod sync_nodes;
mod tile_slopes;
mod tilemap;
#[cfg(test)]
mod tilemap_tests;
mod world_ui;

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use nge_ecs::{App, Plugin, PostUpdate, PreUpdate, Update};
use nge_audio::AudioPlugin;
use nge_particles::ParticlePlugin;
use nge_physics2d::{PhysicsPlugin, PhysicsStepped};
use nge_render2d::{Assets, AtlasId, Atlases, Camera2d, Grid, Image, PointerWorld, TextureId};
use nge_render3d::{Assets3d, Camera3d, Transform3d};
use nge_text::TextPlugin;

pub use ai::{CHASE, ChaseEvent, ChaseNode};
pub use behavior::{BEHAVIOR, BLACKBOARD, BehaviorEvent, BehaviorNode, BlackboardSet, BtNode};
pub use world_ui::{BarNode, BubbleNode, TYPED, Typed};
pub use anim_graph::{ANIM_EVENT, ANIM_SET, AnimEventOut, AnimParams, AnimSet, sprite_frame};
pub use anim::{ANIM_END, AnimEnd, AnimNode, Ease, FrameAnimation, TWEEN, TWEEN_END, Tween, TweenCommand, TweenEnd, TweenProps, Tweens};
pub use audio::{AudioNode, BusName, BusNode, BusesNode, PlaySound, SOUND};
pub use behave::{Spin, Tappable};
pub use camera::{SHAKE, Shake};
pub use effects::{Light2dNode, LightingNode, MaskedNode, PostNode};
pub use joints::{JointKindName, JointNode, MotorNode};
pub use joints3d::{Joint3dKindName, Joint3dNode};
pub use level::{LEVEL_START, LevelStart};
pub use nodes::{AmbientNode, AnchorName, BURST, Burst, Camera3dNode, EmitterNode, FLOAT_TEXT, FloatText, LightKind, LightNode, ModelNode, Particles, TextNode};
pub use net::{NetNode, REMOTE_PREFIX, parse_remote};
pub use physics::{BodyKind, BodyNode, COLLIDE, Collide, DROP_THROUGH, DROPPED, DropThrough, Dropped, PUSH, PhysicsNode, Push, ShapeName};
pub use physics3d::{Body3dKind, ModelBody, Physics3dNode, Shape3dName};
pub use query3d::{Hit3d, QUERY3D, QUERY3D_RESULT, Query3d, Query3dCommand, Query3dKind, Query3dResult, Shape3dQuery};
pub use protocol::{Bounds, CameraNode, Color, SCENE, Scene, SpriteNode, TAP, Tap};
pub use sync::SceneId;
pub use tilemap::TilemapNode;

/// 注册 `scene` prop、`tap` 等事件、`burst` / `floatText` / `push` / `sound` / `tween` / `shake` command 和命名纹理。
/// 要在 `Render3dPlugin`、`Render2dPlugin` 之后添加（先 3D 后 2D，否则模型会盖住精灵）；没加 `ParticlePlugin` / `TextPlugin` / `PhysicsPlugin` / `AudioPlugin` 时自己补上。
///
/// 插件自己放一个带 `Camera2d` 和 `Camera3d` 的相机实体，游戏不要再放别的相机（渲染只用第一个）。
#[derive(Default)]
pub struct ScenePlugin {
    textures: Vec<(String, Vec<u8>, Option<Grid>)>,
}

impl ScenePlugin {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一张 PNG，节点用 `texture: name` 引用。
    pub fn texture(mut self, name: impl Into<String>, png: &[u8]) -> Self {
        self.textures.push((name.into(), png.to_vec(), None));
        self
    }

    /// 注册一张网格图集，节点用 `texture: name, frame: n` 引用。
    pub fn atlas(mut self, name: impl Into<String>, png: &[u8], grid: Grid) -> Self {
        self.textures.push((name.into(), png.to_vec(), Some(grid)));
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum NamedTexture {
    Texture(TextureId),
    Atlas(AtlasId),
}

#[derive(Resource, Default)]
pub(crate) struct SceneTextures(HashMap<String, NamedTexture>);

/// 场景自己维护的相机。
#[derive(Component)]
pub(crate) struct SceneCamera;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        let mut textures = SceneTextures::default();
        let world = app.world_mut();
        assert!(world.contains_resource::<Atlases>(), "ScenePlugin 要在 Render2dPlugin 之后添加");
        assert!(world.contains_resource::<Assets3d>(), "ScenePlugin 要在 Render3dPlugin 之后添加");
        for (name, png, grid) in &self.textures {
            match world.resource_mut::<Assets<Image>>().load_png(png) {
                Ok(id) => {
                    let named = grid.map_or(NamedTexture::Texture(id), |g| NamedTexture::Atlas(world.resource_mut::<Atlases>().add(id, g)));
                    if textures.0.insert(name.clone(), named).is_some() {
                        log::warn!("纹理名 {name} 重复注册，用后一次");
                    }
                }
                Err(e) => log::error!("纹理 {name} 加载失败: {e}"),
            }
        }
        app.world_mut().spawn((Camera2d::default(), Camera3d::default(), Transform3d::default(), SceneCamera));
        if !app.is_plugin_added::<nge_particles::Particles3dPlugin>() {
            app.add_plugins(nge_particles::Particles3dPlugin);
        }
        if !app.is_plugin_added::<ParticlePlugin>() {
            app.add_plugins(ParticlePlugin);
        }
        if !app.is_plugin_added::<TextPlugin>() {
            app.add_plugins(TextPlugin);
        }
        if !app.is_plugin_added::<PhysicsPlugin>() {
            app.add_plugins(PhysicsPlugin::default());
        }
        #[cfg(feature = "physics3d")]
        physics3d::register(app);
        #[cfg(feature = "ai")]
        ai::register(app);
        #[cfg(feature = "ai")]
        behavior::register(app);
        #[cfg(feature = "net")]
        net::register(app);
        #[cfg(not(all(feature = "physics3d", feature = "ai", feature = "net")))]
        app.add_systems(PreUpdate, disabled::warn_disabled.run_if(resource_changed::<Scene>));
        if !app.is_plugin_added::<AudioPlugin>() {
            app.add_plugins(AudioPlugin::default());
        }
        let synced = (
            sync::sync_sprites,
            sync::sync_time_scale,
            sync::sync_perf,
            tilemap::sync_tilemaps,
            camera::sync_camera,
            sync_nodes::sync_texts,
            sync_nodes::sync_emitters,
            sync_nodes::sync_models,
            particles3d::sync_emitters3d,
            sync_nodes::sync_lights,
            sync_nodes::sync_camera3d,
            physics::sync_gravity,
            joints::sync_joints,
            audio::sync_audio,
            effects::sync_effects,
            world_ui::sync_bars,
            world_ui::sync_bubbles,
        );
        app.insert_resource(textures)
            .init_resource::<resolve::UriAtlases>()
            .init_resource::<camera::CameraRig>()
            .add_prop(SCENE)
            .add_event(TAP)
            .add_command(BURST)
            .add_command(FLOAT_TEXT)
            .add_command(PUSH)
            .add_command(DROP_THROUGH)
            .add_command(SOUND)
            .add_command(TWEEN)
            .add_command(SHAKE)
            .add_event(ANIM_END)
            // 动画状态机（见 docs/anim-graph.md）
            .add_command(ANIM_SET)
            .add_event(ANIM_EVENT)
            .add_message::<nge_anim::AnimEvent>()
            .add_systems(Update, anim_graph::anim_set)
            // 物理之后推进状态机：参数可以用物理写回的实际速度
            .configure_sets(PostUpdate, nge_anim::Animated.after(PhysicsStepped))
            .add_systems(PostUpdate, (anim_graph::animate_sprite_graphs.in_set(nge_anim::Animated), anim_graph::forward_events.after(nge_anim::Animated)))
            .add_event(TWEEN_END)
            .add_event(COLLIDE)
            .add_event(DROPPED)
            .add_event(LEVEL_START)
            .add_event(CHASE)
            .add_event(TYPED)
            .add_systems(Update, world_ui::type_text)
            // 跟随插值过的位置；屏幕气泡用这一帧的相机
            .add_systems(PostUpdate, (world_ui::place_bars, world_ui::place_bubbles).after(PhysicsStepped).after(camera::camera_rig))
            .add_message::<LevelStart>()
            .add_systems(PreUpdate, level::switch_level.run_if(resource_changed::<Scene>).before(sync::sync_sprites))
            .add_systems(PreUpdate, synced.run_if(resource_changed::<Scene>).after(level::switch_level).before(PointerWorld))
            .add_systems(PreUpdate, match2d::match_2d.after(sync_nodes::sync_camera3d).after(camera::sync_camera).before(PointerWorld))
            .add_systems(Update, (behave::spin, behave::tap, behave::burst, behave::float_text, physics::push, physics::drop_through, audio::play_sound, anim::animate))
            // 补间最后写，盖过 spin / push 这一帧的改动
            .add_systems(Update, (anim::start_tweens, anim::tween).chain().after(behave::spin).after(physics::push))
            // 精灵和关节都同步完了再连：关节可以在精灵之后才出现，反过来也行
            .add_systems(PostUpdate, joints::resolve.before(PhysicsStepped))
            .add_systems(PostUpdate, attach::attach.after(PhysicsStepped).before(behave::follow))
            // 拖尾读这一帧最后的位置：物理、跟随都改完以后
            .configure_sets(
                PostUpdate,
                nge_particles::Particles3dSimulated.after(PhysicsStepped).after(attach::attach).after(behave::follow),
            )
            .add_systems(PostUpdate, effects::lighting.after(PhysicsStepped).after(behave::follow).after(camera::camera_rig))
            .add_systems(PostUpdate, (behave::follow, physics::collide.after(PhysicsStepped), audio::listen.after(camera::camera_rig).before(nge_audio::AudioUpdated), camera::camera_rig.after(PhysicsStepped), match2d::match_2d.after(camera::camera_rig)));
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use nge_core::Viewport;
    use nge_ecs::App;
    use nge_render2d::{Grid, Render2dPlugin};
    use nge_render3d::Render3dPlugin;
    use serde_json::Value as JsonValue;

    use crate::ScenePlugin;

    pub const BEEP: &str = "asset:/@t/beep.wav";
    pub const VIEWPORT: Viewport = Viewport { size: [100.0, 100.0], pixels: [100, 100] };

    /// 4×2 的图集，每格 8×8。
    pub fn app() -> App {
        build(|_| {})
    }

    /// 先加 `plugin`（比如带假连接的联网插件）再加场景插件。
    #[allow(dead_code)]
    pub fn app_with(plugin: impl nge_ecs::Plugin) -> App {
        build(move |app| {
            app.add_plugins(plugin);
        })
    }

    fn build(extra: impl FnOnce(&mut App)) -> App {
        let mut png = Vec::new();
        image::RgbaImage::new(32, 16).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let source = nge_assets::MemorySource::default().with(BEEP, nge_audio::testing::wav(1.0));
        let mut app = App::new();
        app.add_plugins(nge_assets::AssetPlugin::new().source(std::sync::Arc::new(source)))
            .add_plugins(nge_audio::AudioPlugin::mock())
            .add_plugins(Render3dPlugin)
            .add_plugins(Render2dPlugin);
        extra(&mut app);
        app.add_plugins(ScenePlugin::new().texture("plain", &png).atlas("grid", &png, Grid { cols: 4, rows: 2 }));
        app
    }

    /// 边长 2 的立方体模型（中心在原点）。
    pub fn cube_model(world: &mut bevy_ecs::world::World) -> nge_render3d::ModelId {
        use nge_render3d::{Assets3d, MaterialId, Mesh, Model, Part};
        let mut assets = world.resource_mut::<Assets3d>();
        let mesh = assets.add_mesh(Mesh::cube(2.0)).unwrap();
        assets.add_model(Model { parts: vec![Part::new(mesh, MaterialId::DEFAULT, glam::Mat4::IDENTITY)], ..Default::default() }).unwrap()
    }

    pub fn frame(app: &mut App, scene: Option<JsonValue>) -> Vec<(String, JsonValue)> {
        if let Some(scene) = scene {
            app.prop("scene", Some(&scene));
        }
        app.tick(VIEWPORT, 0.5)
    }
}
