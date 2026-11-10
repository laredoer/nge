//! 中继服务：按房间转发客户端的消息，顺带一个内存里的排行榜（见 docs/networking.md）。
//! `hub`、`scores`、`http` 是纯逻辑，`server` 用 tokio 把它们接到 TCP 上。

pub mod http;
pub mod hub;
pub mod scores;
pub mod server;

pub use hub::{Config, Hub};
pub use server::{serve, spawn};
