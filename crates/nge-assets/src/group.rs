//! 一组资源一起加载：查进度（已完成 / 总数）、等全部完成，失败的也算完成并记下原因。

use std::any::TypeId;
use std::collections::HashSet;

use bevy_ecs::prelude::World;

use crate::server::{Asset, Assets, Handle, LoadState};

type Check = Box<dyn Fn(&World) -> LoadState + Send + Sync>;

/// 一组正在加载的资源，类型可以不一样（贴图、声音、模型……）。
#[derive(Default)]
pub struct LoadGroup {
    items: Vec<(String, Check)>,
    seen: HashSet<(TypeId, u32)>,
}

/// 进度：加载好的和失败的都算 `done`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadProgress {
    pub done: usize,
    pub total: usize,
    /// 失败的 uri 和原因，按加进来的顺序。
    pub failed: Vec<(String, String)>,
}

impl LoadProgress {
    pub fn finished(&self) -> bool {
        self.done == self.total
    }
}

impl LoadGroup {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一个句柄（通常刚从 [`crate::AssetServer::load`] 拿到）；同一个句柄只记一次，记过返回 false。
    pub fn add<A: Asset>(&mut self, uri: impl Into<String>, handle: Handle<A>) -> bool {
        if !self.seen.insert((TypeId::of::<A>(), handle.index())) {
            return false;
        }
        let check: Check = Box::new(move |world: &World| match world.get_resource::<Assets<A>>() {
            Some(assets) => assets.state(handle),
            None => LoadState::Failed(format!("{} 没有注册", std::any::type_name::<A>())),
        });
        self.items.push((uri.into(), check));
        true
    }

    /// 记一个一开始就失败的（比如认不出类型）。
    pub fn fail(&mut self, uri: impl Into<String>, reason: impl Into<String>) {
        let reason = reason.into();
        self.items.push((uri.into(), Box::new(move |_: &World| LoadState::Failed(reason.clone()))));
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn progress(&self, world: &World) -> LoadProgress {
        let mut progress = LoadProgress { total: self.items.len(), ..Default::default() };
        for (uri, check) in &self.items {
            match check(world) {
                LoadState::Loading => {}
                LoadState::Loaded => progress.done += 1,
                LoadState::Failed(e) => {
                    progress.done += 1;
                    progress.failed.push((uri.clone(), e));
                }
            }
        }
        progress
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use bevy_ecs::message::Messages;

    use super::*;
    use crate::server::{AssetEvent, AssetServer, LoadContext};
    use crate::source::MemorySource;

    struct Text;

    impl Asset for Text {
        fn decode(bytes: Vec<u8>, _ctx: &LoadContext) -> Result<Self, String> {
            if bytes.is_empty() { Err("空的".into()) } else { Ok(Text) }
        }
    }

    struct Other;

    impl Asset for Other {
        fn decode(_bytes: Vec<u8>, _ctx: &LoadContext) -> Result<Self, String> {
            Ok(Other)
        }
    }

    fn world() -> World {
        let source = MemorySource::default().with("asset:/a.txt", "a").with("asset:/b.txt", "b").with("asset:/empty.txt", "");
        let mut world = World::new();
        world.insert_resource(AssetServer::new(Arc::new(source)));
        world.init_resource::<Assets<Text>>();
        world.init_resource::<Messages<AssetEvent<Text>>>();
        world.init_resource::<Assets<Other>>();
        world.init_resource::<Messages<AssetEvent<Other>>>();
        world
    }

    fn load<A: Asset>(world: &mut World, group: &mut LoadGroup, uri: &str) -> bool {
        let handle = world.resource_scope(|world, mut server: bevy_ecs::prelude::Mut<AssetServer>| server.load(&mut world.resource_mut::<Assets<A>>(), uri));
        group.add(uri, handle)
    }

    /// 按墙钟等到全部完成（最多 15 秒）。
    fn wait(world: &mut World, group: &LoadGroup) -> LoadProgress {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            AssetServer::apply(world, false);
            let progress = group.progress(world);
            if progress.finished() || Instant::now() > deadline {
                return progress;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn counts_loaded_and_failed_as_done() {
        let mut world = world();
        let mut group = LoadGroup::new();
        load::<Text>(&mut world, &mut group, "asset:/a.txt");
        load::<Other>(&mut world, &mut group, "asset:/b.txt");
        load::<Text>(&mut world, &mut group, "asset:/missing.txt");
        load::<Text>(&mut world, &mut group, "asset:/empty.txt");
        group.fail("asset:/x.what", "不认识");
        let first = group.progress(&world);
        assert_eq!((first.total, first.finished()), (5, false), "{first:?}");
        assert!(first.done >= 1, "一开始就失败的已经算完成");
        let done = wait(&mut world, &group);
        assert_eq!((done.done, done.total), (5, 5));
        let failed: Vec<&str> = done.failed.iter().map(|(u, _)| u.as_str()).collect();
        assert_eq!(failed, ["asset:/missing.txt", "asset:/empty.txt", "asset:/x.what"]);
        assert!(done.failed[1].1.contains("空的"), "带原因：{:?}", done.failed[1]);
    }

    #[test]
    fn the_same_handle_counts_once() {
        let mut world = world();
        let mut group = LoadGroup::new();
        assert!(load::<Text>(&mut world, &mut group, "asset:/a.txt"));
        assert!(!load::<Text>(&mut world, &mut group, "asset://a.txt"), "同一个文件的另一种写法");
        assert!(load::<Other>(&mut world, &mut group, "asset:/a.txt"), "不同类型各算一个");
        assert_eq!(group.len(), 2);
    }

    #[test]
    fn already_loaded_assets_are_done_at_once_and_an_empty_group_is_finished() {
        let mut world = world();
        let mut warm = LoadGroup::new();
        load::<Text>(&mut world, &mut warm, "asset:/a.txt");
        wait(&mut world, &warm);
        let mut group = LoadGroup::new();
        load::<Text>(&mut world, &mut group, "asset:/a.txt");
        assert!(group.progress(&world).finished());
        assert!(LoadGroup::new().progress(&world).finished());
    }

    #[test]
    fn unregistered_types_fail_instead_of_hanging() {
        let world = World::new();
        let mut group = LoadGroup::new();
        group.add::<Text>("asset:/a.txt", Handle::new(0));
        assert_eq!(group.progress(&world).failed.len(), 1);
    }
}
