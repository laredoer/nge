//! 浏览器：游戏视图是组件容器里的一个 `<canvas>`，引擎跑在页面主线程上；
//! 每个 `requestAnimationFrame` 采一次视口、让引擎跑一帧（见 docs/web.md）。
//! 建游戏之前先把应用登记的资源全部取进内存，进度用页面事件告诉加载页（`loading.rs`）。
//! 键盘在 window 的捕获阶段收 keydown / keyup，规则和 macOS 一样（`input.rs`，见 docs/keyboard.md）。

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use nge_assets::MemorySource;

use nge_core::bridge::{ENGINE_ERROR, EngineError, KEY, KEY_EVENT, KeyInput};
use nge_core::{Engine, GAME_COMPONENT, GameFactory, Msg, Viewport};
use nsh_platform_web::{ComponentContext, ComponentFactory, NativeComponent};
use serde_json::Value as JsonValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{CustomEvent, CustomEventInit, Document, Element, HtmlCanvasElement, KeyboardEvent, Response, Window};

use crate::input::{self, CAPTURE_KEYS, capture_keys};
use crate::loading::{self, ERROR, PROGRESS, READY};

/// 把游戏视图注册成 `nge.game` 组件。在应用的 `#[native_module]` 里调用（生成的 web 运行时只调用它）。
/// `assets`：应用登记的资源（`assets!()` 的 uri），建游戏之前全部取进内存。
pub fn register<'a>(factory: GameFactory, assets: impl IntoIterator<Item = &'a str>) {
    nsh_platform_web::register_component(GAME_COMPONENT, component(factory, assets));
}

/// 把 `factory` 包成 web 平台的组件工厂。
pub fn component<'a>(factory: GameFactory, assets: impl IntoIterator<Item = &'a str>) -> ComponentFactory {
    let assets = Rc::new(loading::manifest(assets));
    Rc::new(move |ctx| Box::new(GameView::new(ctx, factory.clone(), assets.clone())))
}

type FrameCallback = Closure<dyn FnMut(f64)>;

struct Shared {
    engine: Option<Engine>,
    window: Window,
    canvas: HtmlCanvasElement,
    /// 上次发给引擎的视口，变了才发。
    viewport: Cell<Viewport>,
    /// 当前排着的 rAF 回调和它的编号。
    frame: RefCell<Option<FrameCallback>>,
    frame_id: Cell<Option<i32>>,
    /// 已经告诉页面游戏画出来了。
    shown: Cell<bool>,
    ctx: ComponentContext,
    /// 转发过按下、还没松开的键，切走时补发松开。
    keys: RefCell<BTreeSet<&'static str>>,
    /// prop `captureKeys`：游戏要吞的键；`None` 时用 [`input::default_capture`]。
    capture: RefCell<Option<Vec<String>>>,
}

impl Shared {
    /// 画布的 CSS 尺寸 × devicePixelRatio。
    fn viewport(&self) -> Viewport {
        let (w, h) = (self.canvas.client_width().max(0) as f32, self.canvas.client_height().max(0) as f32);
        let scale = self.window.device_pixel_ratio() as f32;
        Viewport { size: [w, h], pixels: [(w * scale).round() as u32, (h * scale).round() as u32] }
    }

    fn tick(&self) {
        let Some(engine) = &self.engine else { return };
        let viewport = self.viewport();
        if viewport != self.viewport.replace(viewport) {
            engine.send(Msg::Viewport(viewport));
        }
        engine.frame();
        if !self.shown.get() && engine.running() {
            self.shown.set(true);
            dispatch(&self.window, READY, &JsonValue::Null);
        }
    }

    fn forward(&self, code: &'static str, down: bool, repeat: bool) {
        let key = KeyInput { code: code.to_owned(), down, repeat };
        if let Some(engine) = &self.engine {
            engine.send(Msg::Command(KEY.name().to_owned(), KEY.encode(&key)));
        }
        self.ctx.emit(KEY_EVENT.name(), KEY_EVENT.encode(&key));
    }

    fn release_keys(&self) {
        let held = std::mem::take(&mut *self.keys.borrow_mut());
        for code in held {
            self.forward(code, false, false);
        }
    }

    /// 处理一个 keydown / keyup：转给游戏，游戏要的键不再往下传（不滚页面，NSH 也不处理）。
    fn on_key(&self, event: &KeyboardEvent) {
        let Some(code) = input::name(&event.code()) else { return };
        let down = event.type_() == "keydown";
        let facts = input::Facts {
            code,
            kind: if down { input::Kind::Down } else { input::Kind::Up },
            cmd: event.meta_key() || event.ctrl_key(),
            repeat: down && event.repeat(),
            modifier_down: false,
            eligible: !event.is_composing() && self.canvas.is_connected() && !self.document().hidden() && !editing(event),
            capture: input::captured(self.capture.borrow().as_deref(), code),
        };
        let decision = input::decide(&mut self.keys.borrow_mut(), facts);
        if let Some((down, repeat)) = decision.forward {
            self.forward(code, down, repeat);
        }
        if decision.consume {
            event.prevent_default();
            event.stop_propagation();
        }
    }

    fn document(&self) -> Document {
        self.window.document().expect("页面里有 document")
    }

    fn request_frame(&self) {
        if let Some(callback) = &*self.frame.borrow() {
            self.frame_id.set(self.window.request_animation_frame(callback.as_ref().unchecked_ref()).ok());
        }
    }
}

type Listener = Closure<dyn FnMut(web_sys::Event)>;

pub struct GameView {
    shared: Rc<Shared>,
    document: Document,
    on_visibility: Option<Closure<dyn FnMut()>>,
    /// window 上的 keydown、keyup（捕获阶段）和 blur：(事件名, 是否捕获, 回调)。
    on_window: Vec<(&'static str, bool, Listener)>,
}

impl GameView {
    pub fn new(ctx: ComponentContext, factory: GameFactory, assets: Rc<Vec<String>>) -> Self {
        let window = web_sys::window().expect("页面里有 window");
        let document = window.document().expect("页面里有 document");
        let canvas: HtmlCanvasElement = document.create_element("canvas").expect("能建 canvas").unchecked_into();
        let _ = canvas.set_attribute("style", "position:absolute;left:0;top:0;width:100%;height:100%;display:block");
        match document.query_selector(&ctx.selector()) {
            Ok(Some(host)) => {
                let _ = host.append_child(&canvas);
            }
            _ => log::error!("找不到游戏视图的容器 {}", ctx.selector()),
        }
        let engine = start(&window, canvas.clone(), &ctx, factory, assets);
        let shared = Rc::new(Shared {
            engine,
            window,
            canvas,
            viewport: Cell::default(),
            frame: RefCell::default(),
            frame_id: Cell::default(),
            shown: Cell::new(false),
            ctx,
            keys: RefCell::default(),
            capture: RefCell::default(),
        });
        if shared.engine.is_some() {
            let weak = Rc::downgrade(&shared);
            *shared.frame.borrow_mut() = Some(Closure::new(move |_: f64| {
                if let Some(shared) = weak.upgrade() {
                    shared.tick();
                    shared.request_frame();
                }
            }));
            shared.request_frame();
        }
        let on_visibility = visibility_listener(&document, Rc::downgrade(&shared));
        let on_window = window_listeners(&shared);
        Self { shared, document, on_visibility, on_window }
    }
}

/// 建引擎（先取资源再初始化 GPU）；浏览器没有 WebGPU（或者建不了画面）时发 `error`，返回 `None`。
/// 引擎的 `error` 同时告诉页面（[`ERROR`]），加载页据此显示错误。
fn start(window: &Window, canvas: HtmlCanvasElement, ctx: &ComponentContext, factory: GameFactory, assets: Rc<Vec<String>>) -> Option<Engine> {
    let emit_ctx = ctx.clone();
    let page = window.clone();
    let emit = Box::new(move |n: &str, d: JsonValue| {
        if n == ENGINE_ERROR.name() {
            dispatch(&page, ERROR, &d);
        }
        emit_ctx.emit(n, d)
    });
    let has_gpu = js_sys::Reflect::has(&window.navigator(), &"gpu".into()).unwrap_or(false);
    let engine = if has_gpu {
        Engine::spawn_after(canvas, emit, factory, fetch_assets(window.clone(), assets))
    } else {
        Err("浏览器不支持 WebGPU，请用新版的 Chrome、Edge 或 Safari 打开".to_owned())
    };
    engine
        .map_err(|message| {
            log::error!("{message}");
            let error = ENGINE_ERROR.encode(&EngineError { message });
            dispatch(window, ERROR, &error);
            ctx.emit(ENGINE_ERROR.name(), error);
        })
        .ok()
}

/// 把资源全部取进内存（同时发出请求），每取完一个报一次进度；取完设成资源层的来源。
async fn fetch_assets(window: Window, assets: Rc<Vec<String>>) -> Result<(), String> {
    let total = assets.len();
    dispatch(&window, PROGRESS, &loading::progress(0, total));
    let requests: Vec<(&String, js_sys::Promise)> = assets.iter().map(|uri| (uri, window.fetch_with_str(&nsh_platform_web::asset_url(uri)))).collect();
    let mut source = MemorySource::default();
    for (done, (uri, request)) in requests.into_iter().enumerate() {
        let bytes = fetch_bytes(request).await.map_err(|e| format!("取不到资源 {uri}: {e}"))?;
        source.0.insert(uri.clone(), bytes);
        dispatch(&window, PROGRESS, &loading::progress(done + 1, total));
    }
    nge_assets::set_source(Arc::new(source));
    Ok(())
}

async fn fetch_bytes(request: js_sys::Promise) -> Result<Vec<u8>, String> {
    let text = |e: JsValue| e.as_string().or_else(|| js_sys::JSON::stringify(&e).ok().and_then(|s| s.as_string())).unwrap_or_default();
    let response: Response = JsFuture::from(request).await.map_err(text)?.unchecked_into();
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(text)?).await.map_err(text)?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// 在 `window` 上发一个页面事件（`detail` 是 JSON）。
fn dispatch(window: &Window, name: &str, detail: &JsonValue) {
    let init = CustomEventInit::new();
    if let Ok(detail) = js_sys::JSON::parse(&detail.to_string()) {
        init.set_detail(&detail);
    }
    if let Ok(event) = CustomEvent::new_with_event_init_dict(name, &init) {
        let _ = window.dispatch_event(&event);
    }
}

/// 焦点在输入框、可编辑区域、NSH 的输入框或 iframe 里：按键留给它。
fn editing(event: &KeyboardEvent) -> bool {
    let target = event.target().and_then(|t| t.dyn_into::<Element>().ok());
    target.is_some_and(|el| el.closest("input, textarea, select, [contenteditable]:not([contenteditable=false]), .nsh-field, iframe").ok().flatten().is_some())
}

/// window 上的键盘（捕获阶段，比 NSH 自己的处理先拿到）和窗口失焦（补发松开；不捕获，元素失焦不算）。
fn window_listeners(shared: &Rc<Shared>) -> Vec<(&'static str, bool, Listener)> {
    let mut listeners = Vec::new();
    for (name, capture) in [("keydown", true), ("keyup", true), ("blur", false)] {
        let weak = Rc::downgrade(shared);
        let listener = Listener::new(move |event: web_sys::Event| {
            let Some(shared) = weak.upgrade() else { return };
            match event.dyn_ref::<KeyboardEvent>() {
                Some(key) => shared.on_key(key),
                None => shared.release_keys(),
            }
        });
        if shared.window.add_event_listener_with_callback_and_bool(name, listener.as_ref().unchecked_ref(), capture).is_ok() {
            listeners.push((name, capture, listener));
        }
    }
    listeners
}

/// 页面隐藏时暂停（浏览器也不再回调 rAF）、补发松开，回来接着跑。
fn visibility_listener(document: &Document, shared: Weak<Shared>) -> Option<Closure<dyn FnMut()>> {
    let doc = document.clone();
    let listener = Closure::<dyn FnMut()>::new(move || {
        let Some(shared) = shared.upgrade() else { return };
        let Some(engine) = &shared.engine else { return };
        if doc.hidden() {
            shared.release_keys();
            engine.pause();
        } else {
            engine.resume();
        }
    });
    document.add_event_listener_with_callback("visibilitychange", listener.as_ref().unchecked_ref()).ok()?;
    Some(listener)
}

impl NativeComponent for GameView {
    fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
        if name == CAPTURE_KEYS {
            *self.shared.capture.borrow_mut() = capture_keys(value);
            return;
        }
        if let Some(engine) = &self.shared.engine {
            engine.send(Msg::Prop(name.to_owned(), value.cloned()));
        }
    }

    fn command(&mut self, name: &str, args: &JsonValue) {
        if let Some(engine) = &self.shared.engine {
            engine.send(Msg::Command(name.to_owned(), args.clone()));
        }
    }
}

impl Drop for GameView {
    fn drop(&mut self) {
        if let Some(id) = self.shared.frame_id.take() {
            let _ = self.shared.window.cancel_animation_frame(id);
        }
        self.shared.frame.borrow_mut().take();
        for (name, capture, listener) in self.on_window.drain(..) {
            let _ = self.shared.window.remove_event_listener_with_callback_and_bool(name, listener.as_ref().unchecked_ref(), capture);
        }
        if let Some(listener) = self.on_visibility.take() {
            let _ = self.document.remove_event_listener_with_callback("visibilitychange", listener.as_ref().unchecked_ref());
        }
        self.shared.canvas.remove();
    }
}
