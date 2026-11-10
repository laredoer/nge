//! NSH 游戏引擎平台层：把引擎挂成 NSH 原生组件（macOS、iOS、Android、浏览器）。

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
mod shared;
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
pub use shared::STALL_ENV;

// macOS 的键盘用 AppKit 的按键事件，HID 表只在 iOS 上用（测试在 Mac 上跑）
#[cfg(any(target_os = "ios", test))]
mod hid;
#[cfg(target_os = "macos")]
mod keys;
#[cfg(any(target_os = "macos", target_arch = "wasm32", target_os = "android", test))]
mod input;
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android", test))]
mod pad;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod gc;
#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "ios")]
pub mod ios;

#[cfg(target_os = "android")]
pub mod android;

// Android 的键码和手柄的轴：纯函数，Mac 上也测
#[cfg(any(target_os = "android", test))]
mod android_keys;
#[cfg(any(target_os = "android", test))]
mod android_state;

#[cfg(any(target_arch = "wasm32", test))]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod loading;
#[cfg(target_arch = "wasm32")]
pub mod web;
#[cfg(target_arch = "wasm32")]
mod web_audio;
