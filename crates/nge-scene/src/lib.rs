//! 声明式场景：TSX 通过 `scene` prop 描述精灵，这里按 id 增量同步成 ECS 实体。
//!
//! 节点内容没变的实体不会被改动，所以 Rust 系统可以按 [`SceneId`] 找到实体继续驱动它；
//! 节点一旦在 TSX 里改了，就以 TSX 声明的为准整体覆盖。

mod behave;
mod protocol;
mod sync;

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use nge_ecs::{App, Plugin, PreUpdate, Update};
use nge_render2d::{AtlasId, Assets, Camera2d, Grid, PointerWorld, TextureId};

pub use behave::{Spin, Tappable};
pub use protocol::{CameraNode, Color, SCENE, Scene, SpriteNode, TAP, Tap};
pub use sync::SceneId;

/// 注册 `scene` prop、`tap` 事件和命名纹理。要在 `Render2dPlugin` 之后添加。
///
/// 插件自己放一个 `Camera2d`，游戏不要再放别的相机（渲染只用第一个）。
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
        let mut assets = app.world_mut().get_resource_mut::<Assets>().expect("ScenePlugin 要在 Render2dPlugin 之后添加");
        for (name, png, grid) in &self.textures {
            match assets.load_png(png) {
                Ok(id) => {
                    let named = grid.map_or(NamedTexture::Texture(id), |g| NamedTexture::Atlas(assets.atlas(id, g)));
                    if textures.0.insert(name.clone(), named).is_some() {
                        log::warn!("纹理名 {name} 重复注册，用后一次");
                    }
                }
                Err(e) => log::error!("纹理 {name} 加载失败: {e}"),
            }
        }
        app.world_mut().spawn((Camera2d::default(), SceneCamera));
        app.insert_resource(textures)
            .add_prop(SCENE)
            .add_event(TAP)
            .add_systems(PreUpdate, sync::sync.before(PointerWorld))
            .add_systems(Update, (behave::spin, behave::tap));
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use nge_core::Viewport;
    use nge_ecs::App;
    use nge_render2d::{Grid, Render2dPlugin};
    use serde_json::Value as JsonValue;

    use crate::ScenePlugin;

    pub const VIEWPORT: Viewport = Viewport { size: [100.0, 100.0], pixels: [100, 100] };

    /// 4×2 的图集，每格 8×8。
    pub fn app() -> App {
        let mut png = Vec::new();
        image::RgbaImage::new(32, 16).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let mut app = App::new();
        app.add_plugins(Render2dPlugin)
            .add_plugins(ScenePlugin::new().texture("plain", &png).atlas("grid", &png, Grid { cols: 4, rows: 2 }));
        app
    }

    pub fn frame(app: &mut App, scene: Option<JsonValue>) -> Vec<(String, JsonValue)> {
        if let Some(scene) = scene {
            app.prop("scene", Some(&scene));
        }
        app.tick(VIEWPORT, 0.5)
    }
}
