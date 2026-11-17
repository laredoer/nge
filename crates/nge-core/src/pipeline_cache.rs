//! 管线磁盘缓存：wgpu 的 `PipelineCache` 数据存到平台层给的目录，下次启动接着用（见 docs/android.md）。
//! wgpu 30 只在 Vulkan 上实现了它；文件外面再包一层魔数、版本和校验和，坏了、版本不对当没有。

use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// 文件开头的魔数（最后一位是我们这层格式的版本）。
pub const MAGIC: &[u8; 8] = b"NGEPCAC1";

/// 存进文件的引擎版本：对不上的当没有（wgpu 自己的数据头另外核对 wgpu 和驱动的版本）。
pub fn version() -> String {
    format!("nge-core {}", env!("CARGO_PKG_VERSION"))
}

/// 读出来的文件为什么不用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    Magic,
    Version,
    Length,
    Checksum,
}

/// FNV-1a 64：不是防篡改，只防截断、改坏的数据交给驱动。
fn checksum(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// 魔数、版本（u16 长度 + 字节）、数据长度（u64）、校验和（u64），再是 wgpu 的数据。整数都是小端。
pub fn encode(version: &str, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(MAGIC.len() + 2 + version.len() + 16 + data.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(version.len() as u16).to_le_bytes());
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    out.extend_from_slice(&checksum(data).to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// [`encode`] 的反面：头对得上、长度对、校验和对才交出数据。
pub fn decode<'a>(version: &str, bytes: &'a [u8]) -> Result<&'a [u8], Reject> {
    let rest = bytes.strip_prefix(MAGIC.as_slice()).ok_or(Reject::Magic)?;
    let (len, rest) = rest.split_first_chunk::<2>().ok_or(Reject::Length)?;
    let (stored, rest) = rest.split_at_checked(u16::from_le_bytes(*len) as usize).ok_or(Reject::Length)?;
    if stored != version.as_bytes() {
        return Err(Reject::Version);
    }
    let (len, rest) = rest.split_first_chunk::<8>().ok_or(Reject::Length)?;
    let (sum, data) = rest.split_first_chunk::<8>().ok_or(Reject::Length)?;
    if data.len() as u64 != u64::from_le_bytes(*len) {
        return Err(Reject::Length);
    }
    if checksum(data) != u64::from_le_bytes(*sum) {
        return Err(Reject::Checksum);
    }
    Ok(data)
}

/// 写到一半的临时文件：带上进程号和一个计数，同时写的不撞名（同一个目录里，改名是原子的）。
fn tmp_path(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".{}.{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    path.with_file_name(name)
}

/// 缓存比上次小了一半多：wgpu 多半判定旧数据过期、当空的重来了（日志里要看得出来）。
pub fn shrank(before: usize, after: usize) -> bool {
    after * 2 < before
}

/// 这块 GPU 的缓存文件名；只有实现了管线缓存的后端（Vulkan）有。
pub fn file_name(info: &wgpu::AdapterInfo) -> Option<String> {
    wgpu::util::pipeline_cache_key(info)
}

/// 一个缓存文件：读、原子地写，记着上次存的是什么，没变不重写。
pub struct Store {
    path: PathBuf,
    version: String,
    saved: Option<u64>,
}

impl Store {
    pub fn new(dir: &Path, name: &str, version: &str) -> Self {
        Self { path: dir.join(name), version: version.to_owned(), saved: None }
    }

    /// 存着的数据；不在、读不了当没有，坏了或版本不对删掉文件也当没有。
    pub fn load(&mut self) -> Option<Vec<u8>> {
        let bytes = std::fs::read(&self.path).ok()?;
        match decode(&self.version, &bytes) {
            Ok(data) => {
                self.saved = Some(checksum(data));
                Some(data.to_vec())
            }
            Err(reject) => {
                log::warn!("管线缓存 {} 不能用（{reject:?}），删掉重来", self.path.display());
                let _ = std::fs::remove_file(&self.path);
                None
            }
        }
    }

    /// 写进文件（先写临时文件再改名，写不成删掉临时文件）；和上次存的、读到的一样时不写，返回 false。
    pub fn save(&mut self, data: &[u8]) -> std::io::Result<bool> {
        let sum = checksum(data);
        if self.saved == Some(sum) {
            return Ok(false);
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = tmp_path(&self.path);
        let written = std::fs::write(&tmp, encode(&self.version, data)).and_then(|()| std::fs::rename(&tmp, &self.path));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        self.saved = Some(sum);
        Ok(true)
    }
}

/// 平台层给的缓存目录（Android 的 `codeCacheDir/nge`）；没给时不开管线缓存。
static DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

/// 平台层在起引擎之前调用。
pub fn set_dir(dir: PathBuf) {
    *DIR.write().unwrap_or_else(|e| e.into_inner()) = Some(dir);
}

pub(crate) fn dir() -> Option<PathBuf> {
    DIR.read().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: &str = "nge-core 0.1.0";

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nge-pipeline-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn what_was_encoded_decodes_back() {
        let data = b"driver blob \x00\x01\x02".to_vec();
        assert_eq!(decode(V, &encode(V, &data)), Ok(&data[..]));
        assert_eq!(decode(V, &encode(V, &[])), Ok(&[][..]), "空的也行");
    }

    #[test]
    fn broken_files_are_rejected() {
        let good = encode(V, b"0123456789");
        assert_eq!(decode(V, &good[..good.len() - 1]), Err(Reject::Length), "截断");
        assert_eq!(decode(V, &[good.clone(), vec![0]].concat()), Err(Reject::Length), "多了一截");
        let mut flipped = good.clone();
        *flipped.last_mut().unwrap() ^= 1;
        assert_eq!(decode(V, &flipped), Err(Reject::Checksum), "改了一个字节");
        let mut magic = good.clone();
        magic[0] = b'X';
        assert_eq!(decode(V, &magic), Err(Reject::Magic));
        assert_eq!(decode("nge-core 0.2.0", &good), Err(Reject::Version), "别的版本存的");
        assert_eq!(decode(V, &[]), Err(Reject::Magic), "空文件");
        assert_eq!(decode(V, &good[..MAGIC.len() + 2]), Err(Reject::Length), "头都不全");
    }

    #[test]
    fn only_vulkan_adapters_have_a_file() {
        let mut info = wgpu::AdapterInfo::new(wgpu::DeviceType::IntegratedGpu, wgpu::Backend::Vulkan);
        info.vendor = 0x13b5;
        info.device = 0x92020010;
        let name = file_name(&info).expect("Vulkan 有文件名");
        assert!(name.contains("vulkan") && name.contains(&0x13b5.to_string()), "{name}");
        info.device += 1;
        assert_ne!(file_name(&info), Some(name), "换了 GPU 是另一个文件");
        for backend in [wgpu::Backend::Metal, wgpu::Backend::Gl, wgpu::Backend::BrowserWebGpu, wgpu::Backend::Dx12] {
            assert_eq!(file_name(&wgpu::AdapterInfo::new(wgpu::DeviceType::IntegratedGpu, backend)), None, "{backend:?}");
        }
    }

    #[test]
    fn save_then_load_and_unchanged_data_is_not_written_again() {
        let dir = dir("save");
        let mut store = Store::new(&dir, "gpu", V);
        assert_eq!(store.load(), None, "还没存过");
        assert!(store.save(b"abc").unwrap(), "目录不在时建好");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["gpu"], "先写临时文件再改名，不留下它");
        assert!(!store.save(b"abc").unwrap(), "没变不写");
        assert!(store.save(b"abcd").unwrap());
        let mut again = Store::new(&dir, "gpu", V);
        assert_eq!(again.load().as_deref(), Some(&b"abcd"[..]));
        assert!(!again.save(b"abcd").unwrap(), "读出来的就是存着的，不用再写");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporary_files_are_named_per_process_and_per_write() {
        let path = Path::new("/c/gpu");
        let (a, b) = (tmp_path(path), tmp_path(path));
        assert_ne!(a, b, "同一个进程两次写不撞名");
        assert_eq!(a.parent(), path.parent(), "同一个目录里，改名是原子的");
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("gpu.") && name.ends_with(".tmp") && name.contains(&std::process::id().to_string()), "{name}");
    }

    #[test]
    fn a_much_smaller_cache_is_noticed() {
        assert!(shrank(1000, 400), "少了一半多：wgpu 多半是把旧缓存当空的了");
        assert!(!shrank(1000, 600) && !shrank(1000, 2000) && !shrank(0, 0));
    }

    #[test]
    fn a_broken_or_old_file_is_removed_and_counts_as_none() {
        let dir = dir("broken");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gpu");
        std::fs::write(&path, &encode(V, b"0123456789")[..20]).unwrap();
        assert_eq!(Store::new(&dir, "gpu", V).load(), None);
        assert!(!path.exists(), "坏文件删掉，下次存一份好的");
        std::fs::write(&path, encode("nge-core 0.0.1", b"old")).unwrap();
        let mut store = Store::new(&dir, "gpu", V);
        assert_eq!(store.load(), None);
        assert!(!path.exists());
        assert!(store.save(b"new").unwrap(), "删掉以后照常能存");
        assert_eq!(Store::new(&dir, "gpu", V).load().as_deref(), Some(&b"new"[..]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
