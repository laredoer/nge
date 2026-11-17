//! 浏览器的连接：`web_sys::WebSocket`，回调往队列里放，引擎每帧取。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

use super::{Frame, Link, LinkEvent};

type Queue = Rc<RefCell<VecDeque<LinkEvent>>>;

pub struct WebLink {
    socket: Option<WebSocket>,
    events: Queue,
    /// 连上之前发的先排着。
    pending: Vec<Frame>,
    open: bool,
    _callbacks: Vec<Closure<dyn FnMut(Event)>>,
}

// 浏览器上只有一个线程：Send + Sync 只是为了放进 ECS 的资源（同 docs/web.md 里 wgpu 的做法）
unsafe impl Send for WebLink {}
unsafe impl Sync for WebLink {}

impl WebLink {
    pub fn connect(url: &str) -> Self {
        let events: Queue = Rc::default();
        let socket = match WebSocket::new(url) {
            Ok(s) => s,
            Err(e) => {
                events.borrow_mut().push_back(LinkEvent::Closed(format!("连不上 {url}: {e:?}")));
                return Self { socket: None, events, pending: vec![], open: false, _callbacks: vec![] };
            }
        };
        socket.set_binary_type(BinaryType::Arraybuffer);
        let push = |events: &Queue, f: fn(Event) -> Option<LinkEvent>| {
            let events = events.clone();
            Closure::<dyn FnMut(Event)>::new(move |e: Event| {
                if let Some(event) = f(e) {
                    events.borrow_mut().push_back(event);
                }
            })
        };
        let on_open = push(&events, |_| Some(LinkEvent::Open));
        let on_message = push(&events, |e| {
            let data = e.dyn_into::<MessageEvent>().ok()?.data();
            if let Some(text) = data.as_string() {
                return Some(LinkEvent::Frame(Frame::Text(text)));
            }
            let buffer = data.dyn_into::<js_sys::ArrayBuffer>().ok()?;
            Some(LinkEvent::Frame(Frame::Binary(js_sys::Uint8Array::new(&buffer).to_vec())))
        });
        let on_close = push(&events, |e| {
            let reason = e.dyn_into::<CloseEvent>().map(|c| format!("连接关闭（{}）{}", c.code(), c.reason())).unwrap_or_else(|_| "连接关闭".into());
            Some(LinkEvent::Closed(reason))
        });
        // error 之后浏览器一定会发 close，原因在 close 里报
        let on_error = push(&events, |_| None);
        socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        socket.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        Self { socket: Some(socket), events, pending: vec![], open: false, _callbacks: vec![on_open, on_message, on_close, on_error] }
    }

    fn write(&self, frame: &Frame) {
        let Some(socket) = &self.socket else { return };
        let sent = match frame {
            Frame::Binary(b) => socket.send_with_u8_array(b),
            Frame::Text(t) => socket.send_with_str(t),
        };
        if let Err(e) = sent {
            log::warn!("WebSocket 发送失败: {e:?}");
        }
    }
}

impl Link for WebLink {
    fn send(&mut self, frame: Frame) {
        if self.open { self.write(&frame) } else { self.pending.push(frame) }
    }

    fn poll(&mut self) -> Vec<LinkEvent> {
        let events: Vec<LinkEvent> = self.events.borrow_mut().drain(..).collect();
        for event in &events {
            match event {
                LinkEvent::Open => {
                    self.open = true;
                    for frame in std::mem::take(&mut self.pending) {
                        self.write(&frame);
                    }
                }
                LinkEvent::Closed(_) => {
                    self.open = false;
                    self.pending.clear();
                }
                LinkEvent::Frame(_) => {}
            }
        }
        events
    }
}

impl Drop for WebLink {
    fn drop(&mut self) {
        if let Some(socket) = &self.socket {
            socket.set_onopen(None);
            socket.set_onmessage(None);
            socket.set_onclose(None);
            socket.set_onerror(None);
            let _ = socket.close();
        }
    }
}
