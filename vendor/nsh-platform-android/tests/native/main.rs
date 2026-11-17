//! Rust 写的原生组件的注册表和 JNI 入口的 panic 保护（本仓库加的 `src/registry.rs`、`src/guard.rs`）：JNI 部分只在 Android 上编得出来，
//! 这里直接拿注册表的源文件编，测试在源文件里。

#[path = "../../src/registry.rs"]
#[allow(dead_code, unreachable_pub)]
mod registry;

#[path = "../../src/guard.rs"]
#[allow(dead_code)]
mod guard;
