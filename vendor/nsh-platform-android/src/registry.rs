//! 按名字登记的工厂（本仓库加的）：Rust 写的原生组件注册在这里，Java 的 `Mounter` 找不到 Java 工厂时来问。
//! 不碰 JNI，在 Android 之外也能测。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

pub(crate) struct Registry<F> {
    factories: Mutex<HashMap<String, F>>,
    /// 问过、没登记的名字：只报一次。
    missing: Mutex<HashSet<String>>,
}

impl<F: Clone> Registry<F> {
    pub(crate) fn new() -> Self {
        Self { factories: Mutex::new(HashMap::new()), missing: Mutex::new(HashSet::new()) }
    }

    /// 登记；同名的换掉（应用的 `#[native_module]` 每启动一个引擎都会再登记一次）。
    pub(crate) fn register(&self, name: &str, factory: F) {
        self.factories.lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_owned(), factory);
    }

    pub(crate) fn get(&self, name: &str) -> Option<F> {
        self.factories.lock().unwrap_or_else(|e| e.into_inner()).get(name).cloned()
    }

    /// 没登记过时第一次返回 true（调用方记一次 warn）。
    pub(crate) fn first_miss(&self, name: &str) -> bool {
        self.get(name).is_none() && self.missing.lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_names_are_found_and_registering_again_replaces() {
        let registry = Registry::new();
        assert_eq!(registry.get("nge.game"), None);
        registry.register("nge.game", 1);
        assert_eq!(registry.get("nge.game"), Some(1));
        registry.register("nge.game", 2);
        assert_eq!(registry.get("nge.game"), Some(2), "同名的换掉");
        assert_eq!(registry.get("nge.other"), None);
    }

    #[test]
    fn an_unregistered_name_is_reported_once() {
        let registry = Registry::<i32>::new();
        assert!(registry.first_miss("map"));
        assert!(!registry.first_miss("map"), "只报一次");
        registry.register("game", 1);
        assert!(!registry.first_miss("game"), "登记了的不算");
    }
}
