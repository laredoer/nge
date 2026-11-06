//! 资源层：`asset:/` 路径、后台加载、2D / 3D 共用的贴图。
//!
//! 文件按 NSH 的约定放在游戏 crate 的 `assets/` 目录，`asset!("a.png")` 拿到 `asset:/@<包名>/a.png`。
//! 从哪里读由平台层决定（[`set_source`]），Mac 上 `nsh run` 时回退到 `assets!()` 记下的源文件（[`AssetPlugin::files`]）。

mod gpu;
mod image;
mod server;
mod source;

use std::path::PathBuf;
use std::sync::Arc;

use bevy_ecs::prelude::*;
use nge_ecs::{App, Plugin, PreUpdate};

pub use gpu::GpuImages;
pub use image::{Image, ImageId, MAX_TEXTURE_SIDE};
pub use server::{Asset, AssetEvent, AssetServer, Assets, Handle, LoadContext, LoadState};
pub use source::{AssetSource, Chain, DirSource, MemorySource, SourceFiles, asset_path, bundle_dir, platform_source, set_source};

/// 资源系统集：`PreUpdate` 里最先收回后台加载的结果。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssetsApplied;

/// 加上 [`AssetServer`]、贴图仓库 `Assets<Image>` 和 [`GpuImages`]。2D / 3D 渲染插件没找到它时会自己加一个默认的。
#[derive(Default)]
pub struct AssetPlugin {
    source: Option<Arc<dyn AssetSource>>,
    files: SourceFiles,
}

impl AssetPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    /// 编译时的源文件，平台来源里找不到时用（Mac 上 `nsh run` 时没有 bundle）：
    /// `.files(assets!().iter().map(|f| (f.uri, f.source)))`。
    pub fn files<'a>(mut self, files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        self.files.0.extend(files.into_iter().map(|(uri, path)| (uri.to_owned(), PathBuf::from(path))));
        self
    }

    /// 换掉平台来源（测试用）。
    pub fn source(mut self, source: Arc<dyn AssetSource>) -> Self {
        self.source = Some(source);
        self
    }
}

impl Plugin for AssetPlugin {
    fn build(&self, app: &mut App) {
        if app.world().contains_resource::<AssetServer>() {
            log::warn!("AssetPlugin 已经加过了（渲染插件会自动加一个默认的），要自定义就把它放在渲染插件前面");
            return;
        }
        let source = match &self.source {
            Some(source) => source.clone(),
            None => {
                let files: Arc<dyn AssetSource> = Arc::new(SourceFiles(self.files.0.clone()));
                Arc::new(Chain(platform_source().into_iter().chain([files]).collect()))
            }
        };
        app.insert_resource(AssetServer::new(source))
            .insert_resource(Assets::<Image>::with_white())
            .add_message::<AssetEvent<Image>>()
            .init_resource::<GpuImages>()
            .add_systems(PreUpdate, (|world: &mut World| AssetServer::apply(world, false)).in_set(AssetsApplied));
    }
}

pub trait AssetApp {
    /// 注册一种资源：`Assets<A>` 和 `AssetEvent<A>`。重复调用没有影响。
    fn init_asset<A: Asset>(&mut self) -> &mut Self;
}

impl AssetApp for App {
    fn init_asset<A: Asset>(&mut self) -> &mut Self {
        if !self.world().contains_resource::<Assets<A>>() {
            self.init_resource::<Assets<A>>();
        }
        self.add_message::<AssetEvent<A>>()
    }
}

/// 渲染插件用：没有 [`AssetPlugin`] 时加一个默认的。
pub fn ensure_plugin(app: &mut App) {
    if !app.world().contains_resource::<AssetServer>() {
        app.add_plugins(AssetPlugin::new());
    }
}
