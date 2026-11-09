//! web 上建游戏之前把资源取进内存：取哪些、进度怎么告诉页面（加载页听 `nge-progress` 等事件，见 docs/web.md）。

use serde_json::{Value as JsonValue, json};

/// 页面上的事件：资源进度 `{ done, total }`。
pub(crate) const PROGRESS: &str = "nge-progress";
/// 页面上的事件：游戏画出了第一帧。
pub(crate) const READY: &str = "nge-ready";
/// 页面上的事件：起不来了 `{ message }`。
pub(crate) const ERROR: &str = "nge-error";

/// 要取的资源：只要 `asset:/` 的，去掉重复的，顺序不变。
pub(crate) fn manifest<'a>(uris: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut list: Vec<String> = Vec::new();
    for uri in uris {
        if uri.starts_with("asset:/") && !list.iter().any(|u| u == uri) {
            list.push(uri.to_owned());
        }
    }
    list
}

/// [`PROGRESS`] 的内容。
pub(crate) fn progress(done: usize, total: usize) -> JsonValue {
    json!({ "done": done.min(total), "total": total })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_keeps_assets_once_in_order() {
        let list = manifest(["asset:/@g/b.png", "asset:/@g/a.glb", "asset:/@g/b.png", "https://x/y.png", "asset:/@g/c.wav"]);
        assert_eq!(list, ["asset:/@g/b.png", "asset:/@g/a.glb", "asset:/@g/c.wav"]);
        assert!(manifest([]).is_empty());
    }

    #[test]
    fn progress_never_passes_the_total() {
        assert_eq!(progress(3, 18), json!({ "done": 3, "total": 18 }));
        assert_eq!(progress(20, 18)["done"], 18);
        assert_eq!(progress(0, 0), json!({ "done": 0, "total": 0 }));
    }
}
