//! NSH 游戏引擎平台层：把引擎挂成 NSH 原生组件。

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod shared;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use shared::STALL_ENV;

#[cfg(target_os = "macos")]
mod keys;
#[cfg(target_os = "macos")]
mod pad;
#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "ios")]
pub mod ios;
#[cfg(target_os = "ios")]
pub mod ios_app;
