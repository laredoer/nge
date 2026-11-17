//! 一条 WebSocket 连接：原生（tokio-tungstenite，单独的线程）和浏览器（`WebSocket`）同一套接口。
//! `send` / `poll` 都不阻塞，引擎每帧 `poll` 一次。

#[cfg(all(feature = "client", not(target_arch = "wasm32")))]
mod native;
#[cfg(all(feature = "client", target_arch = "wasm32"))]
mod web;

#[cfg(all(feature = "client", not(target_arch = "wasm32")))]
#[doc(hidden)]
pub use native::live_threads;

/// 单条消息的上限（字节）。
pub const MAX_MESSAGE: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Binary(Vec<u8>),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEvent {
    /// 握手完成，可以发了。
    Open,
    Frame(Frame),
    /// 断了（或者没连上），带原因。之后不会再有事件。
    Closed(String),
}

pub trait Link: Send + Sync {
    /// 发一帧；还没连上时先排着，连上后按顺序发。断了以后发的丢掉。
    fn send(&mut self, frame: Frame);
    /// 取出这段时间的事件。
    fn poll(&mut self) -> Vec<LinkEvent>;
}

/// 建连接的函数：真正的是 [`connect`]，测试里换成假的。
pub type Connector = Box<dyn FnMut(&str) -> Box<dyn Link> + Send + Sync>;

/// 开始连 `url`（`ws://` / `wss://`），马上返回；连上发 `Open`，连不上发 `Closed`。
#[cfg(feature = "client")]
pub fn connect(url: &str) -> Box<dyn Link> {
    #[cfg(not(target_arch = "wasm32"))]
    return Box::new(native::NativeLink::connect(url));
    #[cfg(target_arch = "wasm32")]
    return Box::new(web::WebLink::connect(url));
}
