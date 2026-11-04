//! 节点里写的资源名 → 贴图 / 字体 / 模型句柄：注册过的纹理名，或者交给资源层加载的 `asset:/` 路径。

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use bevy_ecs::system::SystemParam;
use nge_assets::{AssetServer, Assets, Image};
use nge_render2d::{AtlasId, Atlases, Grid, SpriteImage, TextureId};
use nge_render3d::Gltf;
use nge_text::{Font, FontId};

use crate::{NamedTexture, SceneTextures};

/// `asset:/` 贴图按 (uri, 网格) 建的图集，同一组合只建一次。
#[derive(Resource, Default)]
pub(crate) struct UriAtlases(HashMap<(String, [u32; 2]), AtlasId>);

#[derive(SystemParam)]
pub(crate) struct SceneAssets<'w> {
    server: ResMut<'w, AssetServer>,
    images: ResMut<'w, Assets<Image>>,
    fonts: ResMut<'w, Assets<Font>>,
    gltfs: ResMut<'w, Assets<Gltf>>,
    atlases: ResMut<'w, Atlases>,
    named: Res<'w, SceneTextures>,
    uri_atlases: ResMut<'w, UriAtlases>,
}

/// 图集最多这么多格，太大的 `grid` 当成整张图（避免格数相乘溢出）。
const MAX_FRAMES: u32 = 1 << 16;

impl SceneAssets<'_> {
    /// `frame` 只对图集有用。找不到的名字记 warn，画成白色。
    pub fn texture(&mut self, owner: &str, name: Option<&str>, grid: Option<[u32; 2]>, frame: u32) -> SpriteImage {
        let Some(name) = name else { return SpriteImage::Texture(TextureId::WHITE) };
        if name.starts_with("asset:/") {
            let texture = self.server.load(&mut self.images, name);
            let Some(grid) = grid.filter(|g| g[0] > 0 && g[1] > 0 && g[0].checked_mul(g[1]).is_some_and(|n| n <= MAX_FRAMES)) else { return SpriteImage::Texture(texture) };
            let atlases = &mut self.atlases;
            let atlas = *self.uri_atlases.0.entry((name.to_owned(), grid)).or_insert_with(|| atlases.add(texture, Grid { cols: grid[0], rows: grid[1] }));
            return SpriteImage::Atlas(atlas, frame);
        }
        match self.named.0.get(name) {
            Some(NamedTexture::Texture(t)) => SpriteImage::Texture(*t),
            Some(NamedTexture::Atlas(a)) => SpriteImage::Atlas(*a, frame),
            None => {
                log::warn!("节点 {owner:?} 引用了未注册的纹理 {name:?}");
                SpriteImage::Texture(TextureId::WHITE)
            }
        }
    }

    pub fn font(&mut self, uri: Option<&str>) -> Option<FontId> {
        uri.map(|uri| self.server.load(&mut self.fonts, uri))
    }

    pub fn model(&mut self, uri: &str) -> nge_assets::Handle<Gltf> {
        self.server.load(&mut self.gltfs, uri)
    }
}
