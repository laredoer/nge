//! 浏览器：游戏视图是组件容器里的一个 `<canvas>`，引擎跑在页面主线程上；
//! 每个 `requestAnimationFrame` 采一次视口、让引擎跑一帧（见 docs/web.md）。

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use nge_core::bridge::{ENGINE_ERROR, EngineError};
use nge_core::{Engine, GAME_COMPONENT, GameFactory, Msg, Viewport};
use nsh_platform_web::{ComponentContext, ComponentFactory, NativeComponent};
use serde_json::Value as JsonValue;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{Document, HtmlCanvasElement, Window};

/// 把游戏视图注册成 `nge.game` 组件。在应用的 `#[native_module]` 里调用（生成的 web 运行时只调用它）。
pub fn register(factory: GameFactory) {
    nsh_platform_web::register_component(GAME_COMPONENT, component(factory));
}

/// 把 `factory` 包成 web 平台的组件工厂。
pub fn component(factory: GameFactory) -> ComponentFactory {
    Rc::new(move |ctx| Box::new(GameView::new(ctx, factory.clone())))
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
    }

    fn request_frame(&self) {
        if let Some(callback) = &*self.frame.borrow() {
            self.frame_id.set(self.window.request_animation_frame(callback.as_ref().unchecked_ref()).ok());
        }
    }
}

pub struct GameView {
    shared: Rc<Shared>,
    document: Document,
    on_visibility: Option<Closure<dyn FnMut()>>,
}

impl GameView {
    pub fn new(ctx: ComponentContext, factory: GameFactory) -> Self {
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
        let engine = start(&window, canvas.clone(), &ctx, factory);
        let shared = Rc::new(Shared { engine, window, canvas, viewport: Cell::default(), frame: RefCell::default(), frame_id: Cell::default() });
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
        Self { shared, document, on_visibility }
    }
}

/// 建引擎；浏览器没有 WebGPU（或者建不了画面）时发 `error`，返回 `None`。
fn start(window: &Window, canvas: HtmlCanvasElement, ctx: &ComponentContext, factory: GameFactory) -> Option<Engine> {
    let emit_ctx = ctx.clone();
    let emit = Box::new(move |n: &str, d| emit_ctx.emit(n, d));
    let has_gpu = js_sys::Reflect::has(&window.navigator(), &"gpu".into()).unwrap_or(false);
    let engine = if has_gpu { Engine::spawn(canvas, emit, factory) } else { Err("浏览器不支持 WebGPU".to_owned()) };
    engine
        .map_err(|message| {
            log::error!("{message}");
            ctx.emit(ENGINE_ERROR.name(), ENGINE_ERROR.encode(&EngineError { message }));
        })
        .ok()
}

/// 页面隐藏时暂停（浏览器也不再回调 rAF），回来接着跑。
fn visibility_listener(document: &Document, shared: Weak<Shared>) -> Option<Closure<dyn FnMut()>> {
    let doc = document.clone();
    let listener = Closure::<dyn FnMut()>::new(move || {
        let Some(shared) = shared.upgrade() else { return };
        let Some(engine) = &shared.engine else { return };
        if doc.hidden() {
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
        if let Some(listener) = self.on_visibility.take() {
            let _ = self.document.remove_event_listener_with_callback("visibilitychange", listener.as_ref().unchecked_ref());
        }
        self.shared.canvas.remove();
    }
}
