//! 文字：在游戏画面里画文字（glyphon：cosmic-text 排版 + 字形图集），画在 2D 和 3D 之上。
//!
//! [`Text2d`] 的位置取 `Transform2d`：默认是世界坐标（跟着 2D 相机走），带 [`ScreenText`] 时是视图内坐标
//! （左上角原点、y 朝下，单位 point，不跟相机）。字体是资源（[`Font`]），加载完之前这段文字不画。

mod layout;
mod renderer;

use bevy_ecs::prelude::*;
use glam::Vec4;
use nge_assets::{Asset, AssetApp, Handle, LoadContext};
use nge_ecs::{App, Plugin};

pub use layout::Anchor;

/// 字体文件（`.ttf` / `.otf`）。字体名在渲染线程注册进字体库时才读。
pub struct Font(pub std::sync::Arc<Vec<u8>>);

impl Asset for Font {
    fn decode(bytes: Vec<u8>, _ctx: &LoadContext) -> Result<Self, String> {
        // TrueType / OpenType / 字体集合的文件头
        match bytes.get(..4) {
            Some([0, 1, 0, 0] | b"OTTO" | b"true" | b"ttcf") => Ok(Font(std::sync::Arc::new(bytes))),
            _ => Err("不是 TrueType / OpenType 字体".into()),
        }
    }
}

pub type FontId = Handle<Font>;

/// 一段文字。`size` 是字号（point），世界坐标下还会乘上相机的 zoom。
#[derive(Component, Debug, Clone, PartialEq)]
pub struct Text2d {
    pub text: String,
    /// 不填时用系统字体（第一次用到时才扫描系统字体，有一点耗时；iOS 上不保证有中文）。
    pub font: Option<FontId>,
    pub size: f32,
    /// 线性 RGBA。
    pub color: Vec4,
    /// 位置对准文字框的哪一点。
    pub anchor: Anchor,
    /// 超过这个宽度（point）自动换行。
    pub max_width: Option<f32>,
}

impl Text2d {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), font: None, size: 16.0, color: Vec4::ONE, anchor: Anchor::Center, max_width: None }
    }

    pub fn with_font(self, font: FontId) -> Self {
        Self { font: Some(font), ..self }
    }

    pub fn with_size(self, size: f32) -> Self {
        Self { size, ..self }
    }

    pub fn with_color(self, color: Vec4) -> Self {
        Self { color, ..self }
    }

    pub fn with_anchor(self, anchor: Anchor) -> Self {
        Self { anchor, ..self }
    }

    pub fn with_max_width(self, width: f32) -> Self {
        Self { max_width: Some(width), ..self }
    }
}

/// [`Text2d`] 的位置是视图内坐标（左上角原点、y 朝下），不跟 2D 相机。
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ScreenText;

/// 注册字体资源和文字渲染器。要放在 2D / 3D 渲染插件后面，文字才会画在最上面。
pub struct TextPlugin;

impl Plugin for TextPlugin {
    fn build(&self, app: &mut App) {
        nge_assets::ensure_plugin(app);
        app.init_asset::<Font>().add_renderer(renderer::TextRenderer::default());
    }
}

pub mod prelude {
    pub use crate::{Anchor, Font, FontId, ScreenText, Text2d, TextPlugin};
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nge_assets::{AssetPlugin, AssetServer, Assets, LoadState, MemorySource};

    use super::*;

    #[test]
    fn only_font_files_decode() {
        let source = MemorySource::default()
            .with("asset:/@t/a.ttf", include_bytes!("../tests/data/Inter-Bold.ttf").as_slice())
            .with("asset:/@t/b.ttf", b"<html>not found</html>".as_slice());
        let mut app = App::new();
        app.add_plugins(AssetPlugin::new().source(Arc::new(source))).add_plugins(TextPlugin);
        let world = app.world_mut();
        let (good, bad) = world.resource_scope(|world, mut server: Mut<AssetServer>| {
            let mut fonts = world.resource_mut::<Assets<Font>>();
            (server.load(&mut fonts, "asset:/@t/a.ttf"), server.load(&mut fonts, "asset:/@t/b.ttf"))
        });
        AssetServer::apply(world, true);
        let fonts = world.resource::<Assets<Font>>();
        assert_eq!(fonts.state(good), LoadState::Loaded);
        assert!(matches!(fonts.state(bad), LoadState::Failed(e) if e.contains("字体")));
    }
}
