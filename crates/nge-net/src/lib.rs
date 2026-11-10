//! 联网：WebSocket 连接（原生 tokio-tungstenite / 浏览器 WebSocket）、中继协议、客户端状态机、远端插值、HTTP。见 docs/networking.md。

pub mod backoff;
pub mod client;
pub mod limit;
pub mod link;
pub mod proto;
#[doc(hidden)]
pub mod testing;

pub use client::{ClientEvent, NetClient, SendError, Status, Target};
pub use link::{Connector, Frame, Link, LinkEvent};
pub use proto::{Peer, PeerId};
