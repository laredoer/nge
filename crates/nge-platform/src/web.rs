//! 浏览器：游戏视图是组件容器里的一个 `<canvas>`，引擎跑在页面主线程上；
//! 每个 `requestAnimationFrame` 采一次视口、让引擎跑一帧（见 docs/web.md）。
//! 建游戏之前先把应用登记的资源全部取进内存，进度用页面事件告诉加载页（`loading.rs`）。
//! 键盘在 window 的捕获阶段收 keydown / keyup，规则和 macOS 一样（`input.rs`，见 docs/keyboard.md）。
//! 页面被按键、点按时在事件回调里建音频设备（[`nge_audio::unlock`]），之前浏览器不让出声；
//! 页面隐藏时挂起音频设备，回来时恢复（`web_audio.rs`）。
//! 画布藏起来（大小是 0）时发空视口，引擎不更新也不画（`nge_core` 的 `phase.rs`）。
//! 手柄每帧读一遍 `navigator.getGamepads()`（标准布局的），震动用 `vibrationActuator`（`web_pad.rs`，见 docs/input-actions.md）。

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use nge_assets::MemorySource;

use nge_core::bridge::{ENGINE_ERROR, EngineError, GAMEPAD, KEY, KEY_EVENT, KeyInput, PAD, PadInput, READY as ENGINE_READY, RUMBLE_OUT, RumbleOut};
use nge_core::{Engine, GAME_COMPONENT, GameFactory, Msg, Viewport};
use nsh_platform_web::{ComponentContext, ComponentFactory, NativeComponent};
use serde_json::Value as JsonValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{CustomEvent, CustomEventInit, Document, Element, Gamepad, GamepadButton, HtmlCanvasElement, KeyboardEvent, PointerEvent, Response, Window};

use crate::input::{self, CAPTURE_KEYS, capture_keys};
use crate::loading::{self, ERROR, PROGRESS, READY};
use crate::{pad, web_audio, web_pad};

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
    ctx: ComponentContext,
    /// 转发过按下、还没松开的键，切走时补发松开。
    keys: RefCell<BTreeSet<&'static str>>,
    /// prop `captureKeys`：游戏要吞的键；`None` 时用 [`input::default_capture`]。
    capture: RefCell<Option<Vec<String>>>,
    /// 上一帧按着的手柄键、摇杆和扳机（几个手柄合起来的），每个手柄上次发出去的状态。
    pad_keys: RefCell<BTreeSet<&'static str>>,
    analog: Cell<pad::Analog>,
    devices: RefCell<pad::Devices>,
}

thread_local! {
    /// 让手柄震过、还没停过：失焦、页面隐藏时只在这时候去停。
    static RUMBLING: Cell<bool> = const { Cell::new(false) };
}

/// 这一帧连着的、标准布局的手柄的读数，号是 `Gamepad.index`。页面不让读（权限策略）时是空的。
fn read_pads(window: &Window) -> Vec<(u32, pad::Reading)> {
    gamepads(window)
        .into_iter()
        .map(|g| {
            // 有的浏览器数字按钮只给 pressed，不给 value
            let value = |b: GamepadButton| if b.value() == 0.0 && b.pressed() { 1.0 } else { b.value() as f32 };
            let buttons: Vec<f32> = g.buttons().iter().map(|b| value(b.unchecked_into())).collect();
            let axes: Vec<f32> = g.axes().iter().map(|a| a.as_f64().unwrap_or(0.0) as f32).collect();
            (g.index(), web_pad::reading(&buttons, &axes))
        })
        .collect()
}

fn gamepads(window: &Window) -> Vec<Gamepad> {
    let Ok(list) = window.navigator().get_gamepads() else { return Vec::new() };
    let mapping = |g: &Gamepad| js_sys::Reflect::get(g, &"mapping".into()).ok().and_then(|m| m.as_string()).unwrap_or_default();
    list.iter().filter_map(|g| g.dyn_into::<Gamepad>().ok()).filter(|g| web_pad::usable(&mapping(g), g.connected())).collect()
}

thread_local! {
    /// 震不了时（页面没焦点、手柄不支持）`playEffect` 的 Promise 会拒绝：接住，不让控制台报没人处理的错。
    static IGNORE: Closure<dyn FnMut(JsValue)> = Closure::new(|_| {});
}

/// 让手柄震一下（指定的那个，或者全部），浏览器自己会顶掉正在震的；没有 `vibrationActuator` 的不震。
/// [`RumbleOut::is_stop`] 的是停（`reset()`）。
fn rumble_pads(window: &Window, rumble: &RumbleOut) {
    let effect = web_pad::effect(rumble);
    RUMBLING.set(!rumble.is_stop() || (RUMBLING.get() && rumble.pad.is_some()));
    for g in gamepads(window).into_iter().filter(|g| rumble.pad.is_none_or(|id| id == g.index())) {
        let Ok(actuator) = js_sys::Reflect::get(&g, &"vibrationActuator".into()) else { continue };
        if rumble.is_stop() {
            let reset = js_sys::Reflect::get(&actuator, &"reset".into()).and_then(|f| f.dyn_into::<js_sys::Function>());
            if let Ok(promise) = reset.and_then(|f| f.call0(&actuator)).and_then(|p| p.dyn_into::<js_sys::Promise>()) {
                IGNORE.with(|ignore| drop(promise.catch(ignore)));
            }
            continue;
        }
        let Ok(play) = js_sys::Reflect::get(&actuator, &"playEffect".into()).and_then(|f| f.dyn_into::<js_sys::Function>()) else { continue };
        let params = js_sys::Object::new();
        for (name, value) in [("startDelay", 0.0), ("duration", effect.duration_ms), ("strongMagnitude", effect.strong), ("weakMagnitude", effect.weak)] {
            let _ = js_sys::Reflect::set(&params, &name.into(), &value.into());
        }
        if let Ok(promise) = play.call2(&actuator, &"dual-rumble".into(), &params).and_then(|p| p.dyn_into::<js_sys::Promise>()) {
            IGNORE.with(|ignore| drop(promise.catch(ignore)));
        }
    }
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
        self.poll_pads();
        engine.frame();
    }

    /// 每帧读一遍手柄；画布不在页面里、页面隐藏或没焦点时按「什么都没按」算。
    fn poll_pads(&self) {
        let document = self.document();
        if !self.canvas.is_connected() || document.hidden() || !document.has_focus().unwrap_or(true) {
            return self.release_pads();
        }
        let readings = read_pads(&self.window);
        self.update_pads(&readings.iter().map(|(_, r)| *r).collect::<Vec<_>>());
        let changed = self.devices.borrow_mut().read(&readings);
        self.forward_devices(changed);
    }

    /// 手柄按着的全部松开、摇杆归零；手柄还算连着。正在震的停掉。
    fn release_pads(&self) {
        self.update_pads(&[]);
        let released = self.devices.borrow_mut().release();
        self.forward_devices(released);
        if RUMBLING.get() {
            rumble_pads(&self.window, &RumbleOut::stop(None));
        }
    }

    fn forward_devices(&self, changed: Vec<PadInput>) {
        let Some(engine) = &self.engine else { return };
        for input in changed {
            engine.send(Msg::Command(PAD.name().to_owned(), PAD.encode(&input)));
        }
    }

    /// 几个手柄合起来的那条路：按下、松开走 `key`，模拟量走 `gamepad`（和 macOS 一样）。
    fn update_pads(&self, readings: &[pad::Reading]) {
        let (now, analog) = pad::merge(readings, &self.pad_keys.borrow());
        let changes = pad::diff(&self.pad_keys.borrow(), &now, self.analog.get(), analog);
        *self.pad_keys.borrow_mut() = now;
        self.analog.set(analog);
        for code in changes.released {
            self.forward(code, false, false);
        }
        for code in changes.pressed {
            self.forward(code, true, false);
        }
        if let (Some(analog), Some(engine)) = (changes.analog, &self.engine) {
            engine.send(Msg::Command(GAMEPAD.name().to_owned(), GAMEPAD.encode(&analog.input())));
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
            // AltGr 在 Windows 上报成 Ctrl+Alt：它是打字用的，不算快捷键
            cmd: event.meta_key() || (event.ctrl_key() && !event.get_modifier_state("AltGraph")),
            repeat: down && event.repeat(),
            modifier_down: false,
            eligible: !event.is_composing()
                && self.canvas.is_connected()
                && !self.document().hidden()
                && !editing(event)
                && !(on_control(event) && input::activates_control(code)),
            capture: input::captured(self.capture.borrow().as_deref(), code),
        };
        let release_all = input::releases_all(&facts);
        let decision = input::decide(&mut self.keys.borrow_mut(), facts);
        if let Some((down, repeat)) = decision.forward {
            self.forward(code, down, repeat);
        }
        if release_all {
            self.release_keys();
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
    /// window 上的 keydown、keyup、pointerdown、pointerup（捕获阶段）和 blur：(事件名, 是否捕获, 回调)。
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
        web_audio::track();
        let engine = start(&window, canvas.clone(), &ctx, factory, assets);
        let shared = Rc::new(Shared {
            engine,
            window,
            canvas,
            viewport: Cell::default(),
            frame: RefCell::default(),
            frame_id: Cell::default(),
            ctx,
            keys: RefCell::default(),
            capture: RefCell::default(),
            pad_keys: RefCell::default(),
            analog: Cell::default(),
            devices: RefCell::default(),
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
/// 引擎的 `error`、`ready` 同时告诉页面（[`ERROR`]、[`READY`]），加载页据此显示错误、退场。
fn start(window: &Window, canvas: HtmlCanvasElement, ctx: &ComponentContext, factory: GameFactory, assets: Rc<Vec<String>>) -> Option<Engine> {
    let emit_ctx = ctx.clone();
    let page = window.clone();
    let emit = Box::new(move |n: &str, d: JsonValue| {
        // 引擎发的震动不交给 NSH，直接让手柄震
        if n == RUMBLE_OUT.name() {
            match RUMBLE_OUT.decode(&d) {
                Ok(rumble) => rumble_pads(&page, &rumble),
                Err(e) => log::warn!("{e}"),
            }
            return;
        }
        if n == ENGINE_ERROR.name() {
            dispatch(&page, ERROR, &d);
        } else if n == ENGINE_READY.name() {
            dispatch(&page, READY, &JsonValue::Null);
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

/// 把资源全部取进内存（同时发出请求），哪个先取完先算进进度；取完设成资源层的来源。
async fn fetch_assets(window: Window, assets: Rc<Vec<String>>) -> Result<(), String> {
    let total = assets.len();
    dispatch(&window, PROGRESS, &loading::progress(0, total));
    let source = Rc::new(RefCell::new(MemorySource::default()));
    let done = Rc::new(Cell::new(0));
    let all = js_sys::Array::new();
    for uri in assets.iter() {
        let (window, source, done, uri) = (window.clone(), source.clone(), done.clone(), uri.clone());
        all.push(&wasm_bindgen_futures::future_to_promise(async move {
            let request = window.fetch_with_str(&nsh_platform_web::asset_url(&uri));
            let bytes = fetch_bytes(request).await.map_err(|e| JsValue::from(format!("取不到资源 {uri}: {e}")))?;
            source.borrow_mut().0.insert(uri, bytes);
            done.set(done.get() + 1);
            dispatch(&window, PROGRESS, &loading::progress(done.get(), total));
            Ok(JsValue::UNDEFINED)
        }));
    }
    JsFuture::from(js_sys::Promise::all(&all)).await.map_err(error_text)?;
    let source = std::mem::take(&mut *source.borrow_mut());
    nge_assets::set_source(Arc::new(source));
    Ok(())
}

/// 取一个资源。没压缩时核对 `Content-Length`，半截的文件不交给解码器（web 上解码 panic 会让整页停掉）。
async fn fetch_bytes(request: js_sys::Promise) -> Result<Vec<u8>, String> {
    let response: Response = JsFuture::from(request).await.map_err(error_text)?.unchecked_into();
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let headers = response.headers();
    let header = |name: &str| headers.get(name).ok().flatten();
    let buffer = JsFuture::from(response.array_buffer().map_err(error_text)?).await.map_err(error_text)?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    loading::length_ok(header("content-length").as_deref(), header("content-encoding").as_deref(), bytes.len())?;
    Ok(bytes)
}

/// JS 抛出来的东西给人看的样子：`Error` 用 `message`，字符串原样，别的用 `String(e)`。
fn error_text(e: JsValue) -> String {
    if let Some(error) = e.dyn_ref::<js_sys::Error>() {
        return String::from(error.message());
    }
    e.as_string().unwrap_or_else(|| String::from(js_sys::JsString::from(e)))
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

/// 焦点在按钮、链接上：回车、空格是去按它的。
fn on_control(event: &KeyboardEvent) -> bool {
    let target = event.target().and_then(|t| t.dyn_into::<Element>().ok());
    target.is_some_and(|el| el.closest("button, a[href], [role=button], summary").ok().flatten().is_some())
}

/// 浏览器算作用户激活、可以开始出声的事件：用户真按的（不是脚本发的）除 Esc 外的按键，鼠标按下，触摸、笔抬起。
fn activates(event: &web_sys::Event) -> bool {
    if !event.is_trusted() {
        return false;
    }
    if let Some(key) = event.dyn_ref::<KeyboardEvent>() {
        return key.type_() == "keydown" && key.code() != "Escape";
    }
    event.dyn_ref::<PointerEvent>().is_some_and(|p| (p.pointer_type() == "mouse") == (p.type_() == "pointerdown"))
}

/// window 上的键盘（捕获阶段，比 NSH 自己的处理先拿到）、点按（只用来解锁声音）
/// 和窗口失焦（补发松开；不捕获，元素失焦不算）。
fn window_listeners(shared: &Rc<Shared>) -> Vec<(&'static str, bool, Listener)> {
    let mut listeners = Vec::new();
    let events = [("keydown", true), ("keyup", true), ("pointerdown", true), ("pointerup", true), ("blur", false)];
    for (name, capture) in events {
        let weak = Rc::downgrade(shared);
        let listener = Listener::new(move |event: web_sys::Event| {
            let Some(shared) = weak.upgrade() else { return };
            if activates(&event) {
                nge_audio::unlock();
                // 以前的设备（删掉又建的游戏视图、浏览器自己挂起的）拉起来
                web_audio::resume();
            }
            if let Some(key) = event.dyn_ref::<KeyboardEvent>() {
                shared.on_key(key);
            } else if event.type_() == "blur" {
                shared.release_keys();
                shared.release_pads();
            }
        });
        if shared.window.add_event_listener_with_callback_and_bool(name, listener.as_ref().unchecked_ref(), capture).is_ok() {
            listeners.push((name, capture, listener));
        }
    }
    listeners
}

/// 页面隐藏时暂停（浏览器也不再回调 rAF）、补发松开、挂起声音，回来接着跑。
fn visibility_listener(document: &Document, shared: Weak<Shared>) -> Option<Closure<dyn FnMut()>> {
    let doc = document.clone();
    let listener = Closure::<dyn FnMut()>::new(move || {
        let Some(shared) = shared.upgrade() else { return };
        let Some(engine) = &shared.engine else { return };
        if doc.hidden() {
            shared.release_keys();
            shared.release_pads();
            engine.pause();
            web_audio::suspend();
        } else {
            engine.resume();
            web_audio::resume();
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
