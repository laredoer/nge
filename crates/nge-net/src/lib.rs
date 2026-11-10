//! 联网：WebSocket 连接（原生 tokio-tungstenite / 浏览器 WebSocket）、中继协议、客户端状态机、远端插值、HTTP。见 docs/networking.md。

pub mod backoff;
pub mod client;
pub mod http;
pub mod interp;
pub mod limit;
pub mod link;
#[cfg(feature = "ecs")]
mod plugin;
pub mod proto;
#[cfg(feature = "ecs")]
mod sync;
#[doc(hidden)]
pub mod testing;

pub use client::{ClientEvent, NetClient, SendError, Status, Target};
pub use http::{HttpClient, HttpDone, HttpRequest, Method};
#[cfg(feature = "ecs")]
pub use http::{HTTP, HTTP_REQUEST, Http, HttpCommand, HttpPlugin, HttpReply};
pub use link::{Connector, Frame, Link, LinkEvent};
#[cfg(feature = "ecs")]
pub use plugin::{DEFAULT_URL, NET, NET_SEND, Net, NetEvent, NetPlugin, NetSend, NetSet, NetWire};
pub use proto::{Peer, PeerId};
#[cfg(feature = "ecs")]
pub use sync::{KeepOnLeave, NetSync, Remote, Remotes};
