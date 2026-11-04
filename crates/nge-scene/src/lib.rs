//! 声明式场景：TSX 通过 `scene` prop 描述精灵、文字、粒子、3D 模型和灯光，这里按 id 增量同步成 ECS 实体；
//! 一次性的效果（炸粒子、飘字）走 `burst` / `floatText` command。
//!
//! 节点内容没变的实体不会被改动，所以 Rust 系统可以按 [`SceneId`] 找到实体继续驱动它；
//! 节点一旦在 TSX 里改了，就以 TSX 声明的为准整体覆盖。

mod behave;
mod nodes;
mod protocol;
mod resolve;
mod sync;
mod sync_nodes;

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use nge_ecs::{App, Plugin, PostUpdate, PreUpdate, Update};
use nge_particles::ParticlePlugin;
use nge_render2d::{Assets, AtlasId, Atlases, Camera2d, Grid, Image, PointerWorld, TextureId};
use nge_render3d::{Assets3d, Camera3d, Transform3d};
use nge_text::TextPlugin;

pub use behave::{Spin, Tappable};
pub use nodes::{AmbientNode, AnchorName, BURST, Burst, Camera3dNode, EmitterNode, FLOAT_TEXT, FloatText, LightKind, LightNode, ModelNode, Particles, TextNode};
pub use protocol::{CameraNode, Color, SCENE, Scene, SpriteNode, TAP, Tap};
pub use sync::SceneId;

/// 注册 `scene` prop、`tap` 事件、`burst` / `floatText` command 和命名纹理。
/// 要在 `Render3dPlugin`、`Render2dPlugin` 之后添加（先 3D 后 2D，否则模型会盖住精灵）；没加 `ParticlePlugin` / `TextPlugin` 时自己补上。
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
        if !app.is_plugin_added::<ParticlePlugin>() {
            app.add_plugins(ParticlePlugin);
        }
        if !app.is_plugin_added::<TextPlugin>() {
            app.add_plugins(TextPlugin);
        }
        let synced = (
            sync::sync_sprites,
            sync::sync_camera,
            sync_nodes::sync_texts,
            sync_nodes::sync_emitters,
            sync_nodes::sync_models,
            sync_nodes::sync_lights,
            sync_nodes::sync_camera3d,
        );
        app.insert_resource(textures)
            .init_resource::<resolve::UriAtlases>()
            .add_prop(SCENE)
            .add_event(TAP)
            .add_command(BURST)
            .add_command(FLOAT_TEXT)
            .add_systems(PreUpdate, synced.run_if(resource_changed::<Scene>).before(PointerWorld))
            .add_systems(Update, (behave::spin, behave::tap, behave::burst, behave::float_text))
            .add_systems(PostUpdate, behave::follow);
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

    pub const VIEWPORT: Viewport = Viewport { size: [100.0, 100.0], pixels: [100, 100] };

    /// 4×2 的图集，每格 8×8。
    pub fn app() -> App {
        let mut png = Vec::new();
        image::RgbaImage::new(32, 16).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let mut app = App::new();
        app.add_plugins(Render3dPlugin)
            .add_plugins(Render2dPlugin)
            .add_plugins(ScenePlugin::new().texture("plain", &png).atlas("grid", &png, Grid { cols: 4, rows: 2 }));
        app
    }

    /// 边长 2 的立方体模型（中心在原点）。
    pub fn cube_model(world: &mut bevy_ecs::world::World) -> nge_render3d::ModelId {
        use nge_render3d::{Assets3d, MaterialId, Mesh, Model, Part};
        let mut assets = world.resource_mut::<Assets3d>();
        let mesh = assets.add_mesh(Mesh::cube(2.0)).unwrap();
        assets.add_model(Model { parts: vec![Part { mesh, material: MaterialId::DEFAULT, local: glam::Mat4::IDENTITY }] }).unwrap()
    }

    pub fn frame(app: &mut App, scene: Option<JsonValue>) -> Vec<(String, JsonValue)> {
        if let Some(scene) = scene {
            app.prop("scene", Some(&scene));
        }
        app.tick(VIEWPORT, 0.5)
    }
}
