//! 句柄、资源库和后台加载。

use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::mpsc::{self, Receiver, Sender};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bevy_ecs::message::{Message, Messages};
use bevy_ecs::prelude::*;

use crate::source::{AssetSource, asset_path};

/// 一种资源：从文件字节解码（在后台线程上）。
pub trait Asset: Sized + Send + Sync + 'static {
    fn decode(bytes: Vec<u8>, ctx: &LoadContext) -> Result<Self, String>;
}

/// 解码时能拿到的东西：自己的 uri，以及按相对路径再读别的文件（glTF 的外部 `.bin` / 贴图）。
pub struct LoadContext<'a> {
    pub uri: &'a str,
    source: &'a dyn AssetSource,
}

impl LoadContext<'_> {
    /// 相对当前文件所在目录读一个文件（`a/b.gltf` 里的 `../c.bin` → `c.bin`），不能退出自己的包（`@pkg`）。
    pub fn read_relative(&self, path: &str) -> Result<Vec<u8>, String> {
        self.source.read(&join_relative(self.uri, path)?)
    }
}

fn join_relative(uri: &str, path: &str) -> Result<String, String> {
    let mut parts: Vec<&str> = asset_path(uri)?.split('/').collect();
    parts.pop();
    // 包名那一段不能退掉
    let floor = usize::from(parts.first().is_some_and(|p| p.starts_with('@')));
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.len() > floor => {
                parts.pop();
            }
            ".." => return Err(format!("{path} 超出了 {uri} 所在的包")),
            part => parts.push(part),
        }
    }
    Ok(format!("asset:/{}", parts.join("/")))
}

/// 同一个文件的不同写法（`asset://@g/a.png`、`asset:/@g/./a.png`）归成一个 key。
fn canonical(uri: &str) -> String {
    match asset_path(uri) {
        Ok(path) => format!("asset:/{}", path.split('/').filter(|p| !p.is_empty() && *p != ".").collect::<Vec<_>>().join("/")),
        Err(_) => uri.to_owned(),
    }
}

/// 资源的编号。只是一个下标（`Copy`），不做引用计数，资源加载后一直留着。
pub struct Handle<A>(u32, PhantomData<fn() -> A>);

impl<A> Handle<A> {
    pub const fn new(index: u32) -> Self {
        Self(index, PhantomData)
    }

    pub const fn index(self) -> u32 {
        self.0
    }
}

impl<A> Clone for Handle<A> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<A> Copy for Handle<A> {}
impl<A> PartialEq for Handle<A> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<A> Eq for Handle<A> {}
impl<A> PartialOrd for Handle<A> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<A> Ord for Handle<A> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}
impl<A> Hash for Handle<A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}
impl<A> fmt::Debug for Handle<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = std::any::type_name::<A>().rsplit("::").next().unwrap_or("?");
        write!(f, "Handle<{name}>({})", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadState {
    Loading,
    Loaded,
    Failed(String),
}

enum Slot<A> {
    Loading,
    Loaded(A),
    Failed(String),
}

/// 一种资源的仓库：同步添加的（`add`）和后台加载的（[`AssetServer::load`]）都在这里。
#[derive(Resource)]
pub struct Assets<A: Asset> {
    slots: Vec<Slot<A>>,
}

impl<A: Asset> Default for Assets<A> {
    fn default() -> Self {
        Self { slots: Vec::new() }
    }
}

impl<A: Asset> Assets<A> {
    pub fn add(&mut self, asset: A) -> Handle<A> {
        self.slots.push(Slot::Loaded(asset));
        Handle::new(self.slots.len() as u32 - 1)
    }

    /// 没加载完、加载失败或者不存在（比如来自另一个仓库的句柄）时返回 None。
    pub fn get(&self, handle: Handle<A>) -> Option<&A> {
        match self.slots.get(handle.0 as usize) {
            Some(Slot::Loaded(a)) => Some(a),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, handle: Handle<A>) -> Option<&mut A> {
        match self.slots.get_mut(handle.0 as usize) {
            Some(Slot::Loaded(a)) => Some(a),
            _ => None,
        }
    }

    /// 不存在的句柄按失败处理。
    pub fn state(&self, handle: Handle<A>) -> LoadState {
        match self.slots.get(handle.0 as usize) {
            Some(Slot::Loading) => LoadState::Loading,
            Some(Slot::Loaded(_)) => LoadState::Loaded,
            Some(Slot::Failed(e)) => LoadState::Failed(e.clone()),
            None => LoadState::Failed(format!("{handle:?} 不存在")),
        }
    }

    /// 编号上限（包括还在加载的）。
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// 已加载的资源和它们的句柄。
    pub fn iter(&self) -> impl Iterator<Item = (Handle<A>, &A)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| match s {
            Slot::Loaded(a) => Some((Handle::new(i as u32), a)),
            _ => None,
        })
    }

    fn reserve(&mut self) -> Handle<A> {
        self.slots.push(Slot::Loading);
        Handle::new(self.slots.len() as u32 - 1)
    }

    fn finish(&mut self, handle: Handle<A>, result: Result<A, String>) {
        self.slots[handle.0 as usize] = match result {
            Ok(a) => Slot::Loaded(a),
            Err(e) => Slot::Failed(e),
        };
    }
}

/// 后台加载完成（或失败）时发出，每个句柄一次。
#[derive(Message)]
pub enum AssetEvent<A: Asset> {
    Loaded(Handle<A>),
    Failed(Handle<A>, String),
}

impl<A: Asset> fmt::Debug for AssetEvent<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Loaded(h) => write!(f, "Loaded({h:?})"),
            Self::Failed(h, e) => write!(f, "Failed({h:?}, {e:?})"),
        }
    }
}

type Job = Box<dyn FnOnce(&dyn AssetSource) -> Done + Send>;
type Done = Box<dyn FnOnce(&mut World) + Send>;

/// 按 uri 加载资源：读文件和解码在一个后台线程上做，每帧 `PreUpdate` 收回结果。
#[derive(Resource)]
pub struct AssetServer {
    jobs: Option<Sender<Job>>,
    done: Mutex<Receiver<Done>>,
    pending: usize,
    known: HashMap<(TypeId, String), u32>,
    /// 置位后后台线程不再接新任务。
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl AssetServer {
    pub fn new(source: Arc<dyn AssetSource>) -> Self {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (done_tx, done) = mpsc::channel::<Done>();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = std::thread::Builder::new()
            .name("nge-assets".into())
            .spawn(move || {
                for job in job_rx {
                    if stopped.load(Ordering::Acquire) || done_tx.send(job(&*source)).is_err() {
                        break;
                    }
                }
            })
            .expect("创建资源加载线程失败");
        Self { jobs: Some(jobs), done: Mutex::new(done), pending: 0, known: HashMap::new(), stop, worker: Some(worker) }
    }

    /// 立即返回句柄；同一种资源的同一个 uri 只加载一次。
    pub fn load<A: Asset>(&mut self, assets: &mut Assets<A>, uri: &str) -> Handle<A> {
        let uri = canonical(uri);
        let key = (TypeId::of::<A>(), uri.clone());
        if let Some(&index) = self.known.get(&key) {
            return Handle::new(index);
        }
        let handle = assets.reserve();
        self.known.insert(key, handle.0);
        let job: Job = Box::new(move |source| {
            // 解码库遇到坏文件可能 panic：只让这一个资源失败，后台线程继续干活
            let result = catch_unwind(AssertUnwindSafe(|| {
                source.read(&uri).and_then(|bytes| A::decode(bytes, &LoadContext { uri: &uri, source }))
            }))
            .unwrap_or_else(|panic| Err(panic_message(&*panic)));
            let result = result.map_err(|e| format!("加载 {uri} 失败: {e}"));
            Box::new(move |world: &mut World| {
                let event = match &result {
                    Ok(_) => AssetEvent::Loaded(handle),
                    Err(e) => {
                        log::warn!("{e}");
                        AssetEvent::Failed(handle, e.clone())
                    }
                };
                world.resource_mut::<Assets<A>>().finish(handle, result);
                match world.get_resource_mut::<Messages<AssetEvent<A>>>() {
                    Some(mut messages) => {
                        messages.write(event);
                    }
                    None => log::warn!("{} 没有用 init_asset 注册，收不到 AssetEvent", std::any::type_name::<A>()),
                }
            })
        });
        self.pending += 1;
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
        handle
    }

    /// 还没收回结果的加载数。
    pub fn pending(&self) -> usize {
        self.pending
    }

    /// 收回已经完成的加载（`PreUpdate` 里自动调用）。`wait` 为真时阻塞到全部完成（测试和加载界面用）。
    pub fn apply(world: &mut World, wait: bool) {
        loop {
            let done: Vec<Done> = {
                let server = world.resource::<AssetServer>();
                let rx = server.done.lock().unwrap_or_else(|e| e.into_inner());
                if wait && server.pending > 0 { rx.recv().into_iter().chain(rx.try_iter()).collect() } else { rx.try_iter().collect() }
            };
            if done.is_empty() {
                return;
            }
            world.resource_mut::<AssetServer>().pending -= done.len();
            for finish in done {
                finish(world);
            }
            if !wait {
                return;
            }
        }
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    let message = panic.downcast_ref::<&str>().copied().or_else(|| panic.downcast_ref::<String>().map(String::as_str));
    format!("解码时 panic: {}", message.unwrap_or("未知原因"))
}

impl Drop for AssetServer {
    fn drop(&mut self) {
        // 排着队的不再做，后台线程读完手上这个就退出
        self.stop.store(true, Ordering::Release);
        self.jobs = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemorySource;

    struct Text(String);

    impl Asset for Text {
        fn decode(bytes: Vec<u8>, _ctx: &LoadContext) -> Result<Self, String> {
            String::from_utf8(bytes).map(Text).map_err(|e| e.to_string())
        }
    }

    /// 读自己的内容当作相对路径，再读那个文件。
    struct Pointer(String);

    impl Asset for Pointer {
        fn decode(bytes: Vec<u8>, ctx: &LoadContext) -> Result<Self, String> {
            let target = String::from_utf8(bytes).map_err(|e| e.to_string())?;
            String::from_utf8(ctx.read_relative(&target)?).map(Pointer).map_err(|e| e.to_string())
        }
    }

    fn world(source: MemorySource) -> World {
        let mut world = World::new();
        world.insert_resource(AssetServer::new(Arc::new(source)));
        world.init_resource::<Assets<Text>>();
        world.init_resource::<Messages<AssetEvent<Text>>>();
        world.init_resource::<Assets<Pointer>>();
        world.init_resource::<Messages<AssetEvent<Pointer>>>();
        world
    }

    fn load<A: Asset>(world: &mut World, uri: &str) -> Handle<A> {
        world.resource_scope(|world, mut server: Mut<AssetServer>| server.load(&mut world.resource_mut::<Assets<A>>(), uri))
    }

    fn events<A: Asset>(world: &mut World) -> Vec<String> {
        world.resource_mut::<Messages<AssetEvent<A>>>().drain().map(|e| format!("{e:?}")).collect()
    }

    #[test]
    fn loads_in_the_background() {
        let mut world = world(MemorySource::default().with("asset:/a.txt", "hello"));
        let a = load::<Text>(&mut world, "asset:/a.txt");
        assert_eq!(world.resource::<Assets<Text>>().state(a), LoadState::Loading);
        AssetServer::apply(&mut world, true);
        let assets = world.resource::<Assets<Text>>();
        assert_eq!(assets.state(a), LoadState::Loaded);
        assert_eq!(assets.get(a).unwrap().0, "hello");
        assert_eq!(world.resource::<AssetServer>().pending(), 0);
        assert_eq!(events::<Text>(&mut world), ["Loaded(Handle<Text>(0))"]);
    }

    #[test]
    fn same_uri_loads_once_per_type() {
        let mut world = world(MemorySource::default().with("asset:/a.txt", "a.txt").with("asset:/b.txt", "b"));
        let a = load::<Text>(&mut world, "asset:/a.txt");
        assert_eq!(load::<Text>(&mut world, "asset:/a.txt"), a);
        assert_ne!(load::<Text>(&mut world, "asset:/b.txt"), a);
        let p = load::<Pointer>(&mut world, "asset:/a.txt");
        assert_eq!(world.resource::<AssetServer>().pending(), 3, "不同类型各自加载");
        AssetServer::apply(&mut world, true);
        assert_eq!(world.resource::<Assets<Pointer>>().get(p).unwrap().0, "a.txt");
        assert_eq!(events::<Text>(&mut world).len(), 2);
    }

    #[test]
    fn failures_are_reported() {
        let mut world = world(MemorySource::default().with("asset:/bad.txt", vec![0xff, 0xfe]));
        let missing = load::<Text>(&mut world, "asset:/missing.txt");
        let bad = load::<Text>(&mut world, "asset:/bad.txt");
        AssetServer::apply(&mut world, true);
        let assets = world.resource::<Assets<Text>>();
        for h in [missing, bad] {
            assert!(matches!(assets.state(h), LoadState::Failed(e) if e.contains("加载 asset:/")), "{:?}", assets.state(h));
            assert!(assets.get(h).is_none());
        }
        let events = events::<Text>(&mut world);
        assert!(events.iter().all(|e| e.starts_with("Failed")), "{events:?}");
    }

    #[test]
    fn relative_reads_resolve_against_the_file_dir() {
        let source = MemorySource::default().with("asset:/@g/levels/index", "one.txt").with("asset:/@g/levels/one.txt", "level 1");
        let mut world = world(source);
        let p = load::<Pointer>(&mut world, "asset:/@g/levels/index");
        AssetServer::apply(&mut world, true);
        assert_eq!(world.resource::<Assets<Pointer>>().get(p).unwrap().0, "level 1");
    }

    #[test]
    fn relative_reads_normalize_dot_dot_within_the_package() {
        let source = MemorySource::default()
            .with("asset:/@g/models/a", "../tex/./b.txt")
            .with("asset:/@g/tex/b.txt", "texture")
            .with("asset:/@g/escape", "../../other/c.txt")
            .with("asset:/other/c.txt", "outside");
        let mut world = world(source);
        let ok = load::<Pointer>(&mut world, "asset:/@g/models/a");
        let escape = load::<Pointer>(&mut world, "asset:/@g/escape");
        AssetServer::apply(&mut world, true);
        let assets = world.resource::<Assets<Pointer>>();
        assert_eq!(assets.get(ok).unwrap().0, "texture");
        assert!(matches!(assets.state(escape), LoadState::Failed(e) if e.contains("超出")), "{:?}", assets.state(escape));
    }

    /// 解码时 panic。
    struct Boom;

    impl Asset for Boom {
        fn decode(_bytes: Vec<u8>, _ctx: &LoadContext) -> Result<Self, String> {
            panic!("坏文件");
        }
    }

    #[test]
    fn a_panicking_decode_fails_only_that_asset() {
        let mut world = world(MemorySource::default().with("asset:/a.txt", "a"));
        world.init_resource::<Assets<Boom>>();
        world.init_resource::<Messages<AssetEvent<Boom>>>();
        let boom = load::<Boom>(&mut world, "asset:/a.txt");
        let text = load::<Text>(&mut world, "asset:/a.txt");
        AssetServer::apply(&mut world, true);
        assert!(matches!(world.resource::<Assets<Boom>>().state(boom), LoadState::Failed(e) if e.contains("坏文件")));
        assert_eq!(world.resource::<Assets<Text>>().get(text).unwrap().0, "a", "后台线程还活着");
    }

    #[test]
    fn spellings_of_the_same_uri_share_a_handle() {
        let mut world = world(MemorySource::default().with("asset:/@g/a.txt", "a"));
        let a = load::<Text>(&mut world, "asset:/@g/a.txt");
        assert_eq!(load::<Text>(&mut world, "asset://@g/a.txt"), a);
        assert_eq!(load::<Text>(&mut world, "asset:/@g//./a.txt"), a);
        assert_eq!(world.resource::<AssetServer>().pending(), 1);
    }

    /// 每次读都要等一会儿。
    struct Slow;

    impl AssetSource for Slow {
        fn read(&self, _uri: &str) -> Result<Vec<u8>, String> {
            std::thread::sleep(std::time::Duration::from_millis(100));
            Ok(b"x".to_vec())
        }
    }

    #[test]
    fn drop_skips_queued_loads() {
        let mut world = World::new();
        world.insert_resource(AssetServer::new(Arc::new(Slow)));
        world.init_resource::<Assets<Text>>();
        for i in 0..10 {
            load::<Text>(&mut world, &format!("asset:/{i}.txt"));
        }
        let start = std::time::Instant::now();
        drop(world);
        assert!(start.elapsed() < std::time::Duration::from_millis(350), "只等手上那一个：{:?}", start.elapsed());
    }

    #[test]
    fn sync_adds_share_the_numbering() {
        let mut world = world(MemorySource::default().with("asset:/a.txt", "a"));
        let first = world.resource_mut::<Assets<Text>>().add(Text("sync".into()));
        let loaded = load::<Text>(&mut world, "asset:/a.txt");
        let second = world.resource_mut::<Assets<Text>>().add(Text("sync 2".into()));
        assert_eq!((first.index(), loaded.index(), second.index()), (0, 1, 2));
        AssetServer::apply(&mut world, true);
        assert_eq!(world.resource::<Assets<Text>>().iter().count(), 3);
    }

    #[test]
    fn apply_without_wait_does_not_block() {
        let mut world = world(MemorySource::default());
        AssetServer::apply(&mut world, false);
        AssetServer::apply(&mut world, true);
    }
}
