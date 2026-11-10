//! 原生组件（本仓库加的 `src/native.rs`）不在 WebAssembly 上测：和 `text` 一样
//! 直接拿源文件编，测试在源文件里。

#[path = "../../src/native.rs"]
#[allow(dead_code, unreachable_pub)]
mod native;
