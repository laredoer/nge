//! macOS：引擎挂在一个 NSView 上；主线程的 CADisplayLink 只采样视口，出帧在渲染线程。
//! 键盘用 AppKit 的本地事件监听拿 keyDown / keyUp / flagsChanged，转给引擎和 NSH（见 docs/keyboard.md）；
//! 手柄在 display link 里每帧读一遍 GameController（见 docs/gamepad.md）。

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::BTreeSet;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use block2::RcBlock;
use nge_core::bridge::{GAMEPAD, KEY, KEY_EVENT, KeyInput, PAD, PadInput, RumbleOut};
use nge_core::{Engine, GameFactory, Viewport, create_surface};
use nsh_platform_macos::{ComponentContext, ComponentFactory, NativeComponent};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, AnyProtocol, NSObjectProtocol, ProtocolObject};
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSText, NSView, NSWindow, NSWindowDidResignKeyNotification};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObject, NSPoint, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::CADisplayLink;
use raw_window_handle::{AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle};
use serde_json::{Value as JsonValue, json};

use crate::input::{self, CAPTURE_KEYS, capture_keys};
use crate::{gc, keys, pad, rumble};
use crate::shared::Proxy;

/// 把 `factory` 包成 NSH 原生组件工厂，注册到 `AppOptions.components`。
pub fn component(factory: GameFactory) -> ComponentFactory {
    Rc::new(move |ctx| Box::new(GameView::new(ctx, factory.clone())))
}

struct Shared {
    // 字段按声明顺序释放：proxy（内含 Engine，join 渲染线程）先于 view
    proxy: Option<Proxy>,
    view: Retained<HostView>,
    ctx: ComponentContext,
    /// 转发过按下、还没松开的键，切走时补发松开。
    keys: RefCell<BTreeSet<&'static str>>,
    /// prop `captureKeys`：游戏要吞的键；`None` 时用 [`input::default_capture`]。
    capture: RefCell<Option<Vec<String>>>,
    /// 上一帧按着的手柄键、摇杆和扳机（几个手柄合起来的）。
    pad_keys: RefCell<BTreeSet<&'static str>>,
    analog: Cell<pad::Analog>,
    /// 连着的手柄（编号、震动）和每个手柄上次发出去的状态。
    pads: RefCell<gc::Pads>,
    devices: RefCell<pad::Devices>,
    /// 引擎要震的，每帧取出来放。
    rumbles: rumble::Queue,
}

impl Shared {
    fn viewport(&self) -> Viewport {
        let bounds = self.view.bounds().size;
        let backing = self.view.convertSizeToBacking(bounds);
        Viewport {
            size: [bounds.width as f32, bounds.height as f32],
            pixels: [backing.width as u32, backing.height as u32],
        }
    }

    fn forward(&self, code: &'static str, down: bool, repeat: bool) {
        let key = KeyInput { code: code.to_owned(), down, repeat };
        if let Some(proxy) = &self.proxy {
            proxy.command(KEY.name(), &KEY.encode(&key));
        }
        self.ctx.emit(KEY_EVENT.name(), KEY_EVENT.encode(&key));
    }

    /// 给每个按着的键补发松开。
    fn release_keys(&self) {
        let held = std::mem::take(&mut *self.keys.borrow_mut());
        for code in held {
            self.forward(code, false, false);
        }
    }

    /// 窗口不再是 key window、或者视图离开了窗口时补发松开（每帧在主线程查一次）。
    fn check_focus(&self) {
        if !self.view.window().is_some_and(|w| w.isKeyWindow()) {
            if !self.keys.borrow().is_empty() {
                self.release_keys();
            }
            self.release_pads();
        }
    }

    /// 每帧（`check_focus` 之后）读一遍手柄；视图被隐藏时按「什么都没按」算，按着的就松开了。
    fn poll_pads(&self) {
        let key = self.view.window().is_some_and(|w| w.isKeyWindow());
        if key && !self.view.isHiddenOrHasHiddenAncestor() {
            let readings = self.pads.borrow_mut().read();
            self.update_pads(&readings.iter().map(|(_, r)| *r).collect::<Vec<_>>());
            let changed = self.devices.borrow_mut().read(&readings);
            self.forward_devices(changed);
        } else {
            self.release_pads();
        }
    }

    /// 手柄按着的全部松开、摇杆归零；手柄还算连着。正在震的停掉，排着的不震了。
    fn release_pads(&self) {
        self.update_pads(&[]);
        let released = self.devices.borrow_mut().release();
        self.forward_devices(released);
        self.rumbles.take();
        self.pads.borrow_mut().rumble(&RumbleOut::stop(None));
    }

    /// 每个手柄各自的状态（按设备的那条路）。
    fn forward_devices(&self, changed: Vec<PadInput>) {
        let Some(proxy) = &self.proxy else { return };
        for input in changed {
            proxy.command(PAD.name(), &PAD.encode(&input));
        }
    }

    /// 引擎要震的手柄；窗口不在前台、视图藏起来时不震。
    fn play_rumbles(&self) {
        let key = self.view.window().is_some_and(|w| w.isKeyWindow());
        if !key || self.view.isHiddenOrHasHiddenAncestor() {
            self.rumbles.take();
            return;
        }
        for rumble in self.rumbles.take() {
            self.pads.borrow_mut().rumble(&rumble);
        }
    }

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
        if let (Some(analog), Some(proxy)) = (changes.analog, &self.proxy) {
            proxy.command(GAMEPAD.name(), &GAMEPAD.encode(&analog.input()));
        }
    }

    /// 处理一个按键事件，返回 true 表示吞掉（不再往下传）。
    fn on_key(&self, event: &NSEvent) -> bool {
        // 万一错过了切走的通知，先补发松开
        self.check_focus();
        let Some(code) = keys::code(event.keyCode()) else { return false };
        let kind = match event.r#type() {
            NSEventType::KeyDown => input::Kind::Down,
            NSEventType::KeyUp => input::Kind::Up,
            NSEventType::FlagsChanged => input::Kind::Flags,
            _ => return false,
        };
        let flags = event.modifierFlags();
        let facts = input::Facts {
            code,
            kind,
            cmd: flags.contains(NSEventModifierFlags::Command),
            repeat: kind == input::Kind::Down && event.isARepeat(),
            modifier_down: keys::modifier_down(code, flags.0),
            eligible: self.eligible(event),
            capture: input::captured(self.capture.borrow().as_deref(), code),
        };
        let decision = input::decide(&mut self.keys.borrow_mut(), facts);
        if let Some((down, repeat)) = decision.forward {
            self.forward(code, down, repeat);
        }
        decision.consume
    }

    /// 事件是游戏视图所在的 key window 的、视图看得见、没在输入框里打字。
    fn eligible(&self, event: &NSEvent) -> bool {
        let Some(window) = self.view.window() else { return false };
        let ours = event.window(self.view.mtm()).is_some_and(|w| w == window);
        ours && window.isKeyWindow() && !self.view.isHiddenOrHasHiddenAncestor() && !editing(&window)
    }
}

/// 第一响应者是不是文本编辑（输入框），是的话按键留给它。
fn editing(window: &NSWindow) -> bool {
    let Some(responder) = window.firstResponder() else { return false };
    let text_client = AnyProtocol::get(c"NSTextInputClient").is_some_and(|p| responder.conformsToProtocol(p));
    responder.isKindOfClass(NSText::class()) || text_client
}

define_class!(
    /// 承载 surface 的 NSView：离开窗口前同步暂停渲染线程（wgpu 取画面时会访问 `[view window]`）。
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "NgeMacHostView"]
    #[ivars = OnceCell<Weak<Shared>>]
    struct HostView;

    impl HostView {
        #[unsafe(method(viewWillMoveToWindow:))]
        fn view_will_move_to_window(&self, window: Option<&AnyObject>) {
            // 离开或换到另一个窗口都先暂停（渲染线程取画面时会访问 [view window]），didMoveToWindow 里再恢复
            self.set_in_window(false);
            if let Some(shared) = self.ivars().get().and_then(Weak::upgrade) {
                shared.release_keys();
                shared.release_pads();
            }
            let _: () = unsafe { msg_send![super(self), viewWillMoveToWindow: window] };
        }

        #[unsafe(method(viewDidMoveToWindow))]
        fn view_did_move_to_window(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidMoveToWindow] };
            let window: Option<Retained<AnyObject>> = unsafe { msg_send![self, window] };
            if window.is_some() {
                self.set_in_window(true);
            }
        }

        // 导航切走屏幕时 NSView 被隐藏，display link 可能不再回调：手柄按着的先松开
        #[unsafe(method(viewDidHide))]
        fn view_did_hide(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidHide] };
            if let Some(shared) = self.ivars().get().and_then(Weak::upgrade) {
                shared.release_pads();
            }
        }

        // 鼠标交给外层的节点视图，NSH 的 onTouch* 才收得到
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> Option<Retained<NSView>> {
            None
        }
    }
);

impl HostView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(OnceCell::new());
        unsafe { msg_send![super(this), init] }
    }

    fn set_in_window(&self, in_window: bool) {
        if let Some(proxy) = self.ivars().get().and_then(Weak::upgrade).as_ref().and_then(|s| s.proxy.as_ref()) {
            proxy.set_in_window(in_window);
        }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NgeMacTicker"]
    #[ivars = Rc<Shared>]
    struct Ticker;

    impl Ticker {
        #[unsafe(method(tick:))]
        fn tick(&self, _link: &CADisplayLink) {
            let shared = self.ivars();
            shared.check_focus();
            shared.poll_pads();
            shared.play_rumbles();
            if let Some(proxy) = &shared.proxy {
                proxy.tick(shared.viewport());
            }
        }
    }
);

impl Ticker {
    fn new(mtm: MainThreadMarker, shared: Rc<Shared>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(shared);
        unsafe { msg_send![super(this), init] }
    }
}

pub struct GameView {
    shared: Rc<Shared>,
    link: Retained<CADisplayLink>,
    key_monitor: Option<Retained<AnyObject>>,
    resign_observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl GameView {
    pub fn new(ctx: ComponentContext, factory: GameFactory) -> Self {
        let mtm = MainThreadMarker::new().expect("原生组件必须在主线程创建");
        let view = HostView::new(mtm);
        view.setWantsLayer(true);

        let rumbles = rumble::Queue::default();
        let window = RawWindowHandle::AppKit(AppKitWindowHandle::new(NonNull::from(&*view).cast()));
        let display = RawDisplayHandle::AppKit(AppKitDisplayHandle::new());
        // SAFETY: view 由 Shared 持有，且 Shared 先释放 proxy（Engine）再释放 view
        let proxy = match unsafe { create_surface(window, display) } {
            Ok((instance, surface)) => {
                let emit_ctx = ctx.clone();
                // 引擎发的震动截下来，不交给 NSH
                let emit = rumbles.wrap(Box::new(move |n: &str, d| emit_ctx.emit(n, d)));
                match Engine::spawn(instance, surface, emit, factory, Box::new(view.clone())) {
                    Ok(engine) => Some(Proxy::new(engine, true)),
                    Err(e) => {
                        log::error!("{e}");
                        ctx.emit("error", json!({ "message": e }));
                        None
                    }
                }
            }
            Err(e) => {
                log::error!("创建 surface 失败: {e}");
                ctx.emit("error", json!({ "message": e }));
                None
            }
        };
        let shared = Rc::new(Shared {
            proxy,
            view,
            ctx,
            keys: RefCell::default(),
            capture: RefCell::default(),
            pad_keys: RefCell::default(),
            analog: Cell::default(),
            pads: RefCell::default(),
            devices: RefCell::default(),
            rumbles,
        });
        let _ = shared.view.ivars().set(Rc::downgrade(&shared));

        let ticker = Ticker::new(mtm, shared.clone());
        // NSView 的 display link 跟随视图所在屏幕
        let link = unsafe { shared.view.displayLinkWithTarget_selector(&ticker, sel!(tick:)) };
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        let key_monitor = key_monitor(Rc::downgrade(&shared));
        let resign_observer = resign_observer(Rc::downgrade(&shared));
        Self { shared, link, key_monitor, resign_observer }
    }
}

impl NativeComponent for GameView {
    fn view(&self) -> Retained<NSView> {
        self.shared.view.clone().into_super()
    }

    fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
        if name == CAPTURE_KEYS {
            *self.shared.capture.borrow_mut() = capture_keys(value);
            return;
        }
        if let Some(proxy) = &self.shared.proxy {
            proxy.prop(name, value);
        }
    }

    fn command(&mut self, name: &str, args: &JsonValue) {
        if let Some(proxy) = &self.shared.proxy {
            proxy.command(name, args);
        }
    }
}

/// 本地事件监听（只看本 App 的事件，在主线程派发之前调用）。
fn key_monitor(shared: Weak<Shared>) -> Option<Retained<AnyObject>> {
    let mask = NSEventMask::KeyDown | NSEventMask::KeyUp | NSEventMask::FlagsChanged;
    let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit 传进来的事件在回调期间有效
        let consumed = shared.upgrade().is_some_and(|s| s.on_key(unsafe { event.as_ref() }));
        if consumed { std::ptr::null_mut() } else { event.as_ptr() }
    });
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &handler) };
    if monitor.is_none() {
        log::warn!("装不上键盘监听，游戏收不到按键");
    }
    monitor
}

/// 任何窗口不再是 key window（切到别的窗口、别的 App）时补发松开；display link 在窗口被挡住时可能不回调。
fn resign_observer(shared: Weak<Shared>) -> Retained<ProtocolObject<dyn NSObjectProtocol>> {
    let handler = RcBlock::new(move |_: NonNull<NSNotification>| {
        if let Some(shared) = shared.upgrade() {
            shared.check_focus();
        }
    });
    let center = NSNotificationCenter::defaultCenter();
    unsafe { center.addObserverForName_object_queue_usingBlock(Some(NSWindowDidResignKeyNotification), None, None, &handler) }
}

impl Drop for GameView {
    fn drop(&mut self) {
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(AsRef::<AnyObject>::as_ref(&*self.resign_observer)) };
        if let Some(monitor) = self.key_monitor.take() {
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
        // 断开 display link → Ticker → Shared；最后一个 Rc 释放时 Engine 会 join 渲染线程
        self.link.invalidate();
    }
}
