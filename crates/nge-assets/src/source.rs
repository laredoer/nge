//! `asset:/…` 文件从哪里读：App 包里的资源目录，或者编译时记下的源文件。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// 按 uri 读出文件的全部字节。在资源层的后台线程上调用。
pub trait AssetSource: Send + Sync {
    fn read(&self, uri: &str) -> Result<Vec<u8>, String>;
}

/// `asset:/@pkg/a.png` → `@pkg/a.png`；不是 `asset:/` 开头时返回错误。
pub fn asset_path(uri: &str) -> Result<&str, String> {
    let path = uri.strip_prefix("asset:/").ok_or_else(|| format!("{uri} 不是 asset:/ 路径"))?.trim_start_matches('/');
    if path.is_empty() || path.split('/').any(|part| part == "..") {
        return Err(format!("{uri} 不是合法的资源路径"));
    }
    Ok(path)
}

/// 资源根目录：`<dir>/<asset 路径>`。
pub struct DirSource(pub PathBuf);

impl AssetSource for DirSource {
    fn read(&self, uri: &str) -> Result<Vec<u8>, String> {
        let path = self.0.join(asset_path(uri)?);
        std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// 编译时记下的源文件（NSH 的 `assets!()`：uri → 构建时的绝对路径）。Mac 上 `nsh run` 时用。
#[derive(Default)]
pub struct SourceFiles(pub HashMap<String, PathBuf>);

impl AssetSource for SourceFiles {
    fn read(&self, uri: &str) -> Result<Vec<u8>, String> {
        let path = self.0.get(uri).ok_or_else(|| format!("{uri} 不在 assets!() 列出的文件里"))?;
        std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// 依次尝试，第一个读到的为准；都失败时把每一个的错误拼起来。
pub struct Chain(pub Vec<Arc<dyn AssetSource>>);

impl AssetSource for Chain {
    fn read(&self, uri: &str) -> Result<Vec<u8>, String> {
        let mut errors = Vec::new();
        for source in &self.0 {
            match source.read(uri) {
                Ok(bytes) => return Ok(bytes),
                Err(e) => errors.push(e),
            }
        }
        Err(if errors.is_empty() { format!("{uri}: 没有可用的资源来源") } else { errors.join("；") })
    }
}

/// 内存里的文件，测试用。
#[derive(Default)]
pub struct MemorySource(pub HashMap<String, Vec<u8>>);

impl MemorySource {
    pub fn with(mut self, uri: &str, bytes: impl Into<Vec<u8>>) -> Self {
        self.0.insert(uri.to_owned(), bytes.into());
        self
    }
}

impl AssetSource for MemorySource {
    fn read(&self, uri: &str) -> Result<Vec<u8>, String> {
        self.0.get(uri).cloned().ok_or_else(|| format!("{uri} 不存在"))
    }
}

static GLOBAL: RwLock<Option<Arc<dyn AssetSource>>> = RwLock::new(None);

/// 平台层在创建引擎之前调用，换成别的来源。没调用时用 [`bundle_dir`]。
pub fn set_source(source: Arc<dyn AssetSource>) {
    *GLOBAL.write().unwrap_or_else(|e| e.into_inner()) = Some(source);
}

/// 平台层设置的来源；没设置时是 App 包里的资源目录（找不到目录时为 None）。
pub fn platform_source() -> Option<Arc<dyn AssetSource>> {
    if let Some(source) = GLOBAL.read().unwrap_or_else(|e| e.into_inner()).clone() {
        return Some(source);
    }
    let exe = std::env::current_exe().and_then(|e| e.canonicalize()).ok();
    let dir = bundle_dir(std::env::var_os("NSH_APP_DIR").map(PathBuf::from), exe.as_deref())?;
    Some(Arc::new(DirSource(dir)))
}

/// App 的资源目录（和 nsh 的 host 一致）：
/// `NSH_APP_DIR/assets`，Mac App 的 `Contents/Resources/nsh/assets`，iOS 包里的 `nsh/assets`（nsh 0.4 的 `nsh package`）或根目录下的 `assets`。
pub fn bundle_dir(app_dir: Option<PathBuf>, exe: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = app_dir {
        return Some(dir.join("assets"));
    }
    let exe_dir = exe?.parent()?;
    [exe_dir.parent().map(|contents| contents.join("Resources/nsh/assets")), Some(exe_dir.join("nsh/assets")), Some(exe_dir.join("assets"))]
        .into_iter()
        .flatten()
        .find(|dir| dir.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_paths() {
        assert_eq!(asset_path("asset:/@game/a.png"), Ok("@game/a.png"));
        assert_eq!(asset_path("asset://@game/a.png"), Ok("@game/a.png"));
        for bad in ["@game/a.png", "asset:/", "asset:/@game/../../etc/passwd", "file:///a.png"] {
            assert!(asset_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn chain_takes_the_first_hit_and_joins_errors() {
        let a = Arc::new(MemorySource::default().with("asset:/a", "from a"));
        let b = Arc::new(MemorySource::default().with("asset:/a", "from b").with("asset:/b", "only b"));
        let chain = Chain(vec![a, b]);
        assert_eq!(chain.read("asset:/a").unwrap(), b"from a");
        assert_eq!(chain.read("asset:/b").unwrap(), b"only b");
        let err = chain.read("asset:/c").unwrap_err();
        assert_eq!(err.matches("不存在").count(), 2, "{err}");
    }

    #[test]
    fn dir_source_reads_under_its_root() {
        let root = std::env::temp_dir().join(format!("nge-assets-{}", std::process::id()));
        std::fs::create_dir_all(root.join("@game")).unwrap();
        std::fs::write(root.join("@game/a.txt"), "hi").unwrap();
        let source = DirSource(root.clone());
        assert_eq!(source.read("asset:/@game/a.txt").unwrap(), b"hi");
        assert!(source.read("asset:/@game/missing.txt").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bundle_dir_prefers_nsh_app_dir() {
        assert_eq!(bundle_dir(Some("/dev/app".into()), Some(Path::new("/x/y"))), Some(PathBuf::from("/dev/app/assets")));
        assert_eq!(bundle_dir(None, None), None);

        let root = std::env::temp_dir().join(format!("nge-bundle-{}", std::process::id()));
        let mac = root.join("Game.app/Contents");
        std::fs::create_dir_all(mac.join("MacOS")).unwrap();
        std::fs::create_dir_all(mac.join("Resources/nsh/assets")).unwrap();
        assert_eq!(bundle_dir(None, Some(&mac.join("MacOS/game"))), Some(mac.join("Resources/nsh/assets")));
        let ios = root.join("Game.ios.app");
        std::fs::create_dir_all(ios.join("assets")).unwrap();
        assert_eq!(bundle_dir(None, Some(&ios.join("game"))), Some(ios.join("assets")));
        // nsh 0.4 打的 iOS 包：资源在 nsh/assets 下
        let ios4 = root.join("Game4.app");
        std::fs::create_dir_all(ios4.join("nsh/assets")).unwrap();
        assert_eq!(bundle_dir(None, Some(&ios4.join("game"))), Some(ios4.join("nsh/assets")));
        assert_eq!(bundle_dir(None, Some(&root.join("plain/game"))), None, "没有资源目录");
        std::fs::remove_dir_all(root).unwrap();
    }
}
