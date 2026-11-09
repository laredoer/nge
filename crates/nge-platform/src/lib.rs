//! NSH 游戏引擎平台层：把引擎挂成 NSH 原生组件（macOS、iOS、浏览器）。

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod shared;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use shared::STALL_ENV;

// macOS 的键盘用 AppKit 的按键事件，HID 表只在 iOS 上用（测试在 Mac 上跑）
#[cfg(any(target_os = "ios", test))]
mod hid;
#[cfg(target_os = "macos")]
mod keys;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod pad;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod gc;
#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "ios")]
pub mod ios;

#[cfg(target_arch = "wasm32")]
pub mod web;
