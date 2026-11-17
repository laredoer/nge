//! JNI 入口用的（本仓库加的）：Rust 组件 panic 时不能跨到 Java（会直接崩进程），记日志、返回默认值。
//! 不碰 JNI，在 Android 之外也能测。

/// 跑 `f`；panic 了记一条错误日志（带上入口名 `name`），返回 `fallback`。
pub(crate) fn guard<R>(name: &str, fallback: R, f: impl FnOnce() -> R) -> R {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(panic) => {
            let message = panic.downcast_ref::<&str>().map(|s| (*s).to_owned()).or_else(|| panic.downcast_ref::<String>().cloned()).unwrap_or_default();
            log::error!("Rust 组件的 {name} panic 了: {message}");
            fallback
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_is_logged_and_the_fallback_returned() {
        assert_eq!(guard("ok", 0, || 7), 7);
        assert_eq!(guard("boom", 0, || -> i32 { panic!("坏了") }), 0, "panic 不往外传");
        assert!(!guard("boom", false, || -> bool { panic!("{}", String::from("坏了")) }));
    }
}
