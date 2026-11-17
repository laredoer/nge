//! iOS：引擎挂在一个 UIView 上；主线程的 CADisplayLink 只采样视口，出帧在渲染线程。
//! 手柄和硬件键盘在 display link 里每帧读一遍 GameController（见 docs/ios-input.md）。

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::BTreeSet;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use nge_core::bridge::{GAMEPAD, KEY, KEY_EVENT, KeyInput, PAD, PadInput};
use nge_core::{Engine, GameFactory, Viewport, create_surface};
use nsh_platform_ios::{ComponentContext, ComponentFactory, NativeComponent};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use block2::RcBlock;
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObject, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::CADisplayLink;
use objc2_quartz_core::CAFrameRateRange;
use objc2_ui_kit::{UIApplication, UIApplicationDidBecomeActiveNotification, UIApplicationDidEnterBackgroundNotification, UIApplicationState, UIApplicationWillEnterForegroundNotification, UIApplicationWillResignActiveNotification, UITraitEnvironment, UIView, UIWindow};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, UiKitDisplayHandle, UiKitWindowHandle};
use serde_json::{Value as JsonValue, json};

use crate::shared::Proxy;
use crate::{gc, hid, pad, rumble};

/// 把 `factory` 包成 NSH 原生组件工厂，注册到宿主的 `Components`。
pub fn component(factory: GameFactory) -> ComponentFactory {
    Rc::new(move |ctx| {
        watch_app_state();
        Box::new(GameView::new(ctx, factory.clone()))
    })
}

/// 监听 App 进后台 / 回前台（第一次建游戏视图时装上，跟 App 同生命周期）。不靠宿主转告：nsh 生成的宿主不会通知原生组件。
fn watch_app_state() {
    thread_local! {
        static WATCHING: Cell<bool> = const { Cell::new(false) };
    }
    if WATCHING.with(|w| w.replace(true)) {
        return;
    }
    // 装上之前 App 可能已经在后台（游戏视图晚挂载），先按当前状态起步
    if let Some(mtm) = MainThreadMarker::new() {
        let background = UIApplication::sharedApplication(mtm).applicationState() == UIApplicationState::Background;
        APP_ACTIVE.with(|a| a.set(!background));
    }
    let center = NSNotificationCenter::defaultCenter();
    for (name, active) in unsafe { [(UIApplicationDidEnterBackgroundNotification, false), (UIApplicationWillEnterForegroundNotification, true)] } {
        let handler = RcBlock::new(move |_: NonNull<NSNotification>| set_app_active(active));
        // 观察者要一直留着；返回的令牌泄漏掉，免得释放时被移除
        let token = unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &handler) };
        std::mem::forget(token);
    }
    // 控制中心、来电、分屏切走焦点：App 没进后台，但输入归别人了（只管输入，不暂停渲染）
    if let Some(mtm) = MainThreadMarker::new() {
        let active = UIApplication::sharedApplication(mtm).applicationState() == UIApplicationState::Active;
        INPUT_ACTIVE.with(|a| a.set(active));
    }
    for (name, active) in unsafe { [(UIApplicationWillResignActiveNotification, false), (UIApplicationDidBecomeActiveNotification, true)] } {
        let handler = RcBlock::new(move |_: NonNull<NSNotification>| set_input_active(active));
        let token = unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &handler) };
        std::mem::forget(token);
    }
}

/// App 失去 / 拿回焦点（主线程）：失去时按着的键和手柄补发松开，之后不读输入。
fn set_input_active(active: bool) {
    INPUT_ACTIVE.with(|a| a.set(active));
    if !active {
        LIVE.with(|live| live.borrow().iter().filter_map(Weak::upgrade).for_each(|shared| shared.release_input()));
    }
}

struct Shared {
    // 字段按声明顺序释放：proxy（内含 Engine，join 渲染线程）先于 view
    proxy: Option<Proxy>,
    view: Retained<HostView>,
    ctx: ComponentContext,
    /// 转发过的硬件键盘按键。
    keyboard: RefCell<hid::Keyboard>,
    /// 上一帧按着的手柄键、摇杆和扳机（几个手柄合起来的）。
    pad_keys: RefCell<BTreeSet<&'static str>>,
    analog: Cell<pad::Analog>,
    /// 连着的手柄（编号、震动）和每个手柄上次发出去的状态。
    pads: RefCell<gc::Pads>,
    devices: RefCell<pad::Devices>,
    /// 引擎要震的，每帧取出来放。
    rumbles: rumble::Queue,
}

thread_local! {
    /// 所有存活的游戏 View，App 前后台切换时逐个通知。
    static LIVE: RefCell<Vec<Weak<Shared>>> = const { RefCell::new(Vec::new()) };
    /// App 当前是否在前台；后台新建的 View 也要按它起步。
    static APP_ACTIVE: Cell<bool> = const { Cell::new(true) };
    /// App 是否拿着焦点（active）：控制中心、来电横幅、分屏切走时是 false，这时不读手柄和键盘。
    static INPUT_ACTIVE: Cell<bool> = const { Cell::new(true) };
}

/// App 进后台 / 回前台（主线程）：后台不能再提交 GPU 命令，否则会被系统杀掉。
fn set_app_active(active: bool) {
    APP_ACTIVE.with(|a| a.set(active));
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        live.retain(|w| w.strong_count() > 0);
        for shared in live.iter().filter_map(Weak::upgrade) {
            if !active {
                shared.release_input();
            }
            if let Some(proxy) = &shared.proxy {
                proxy.set_active(active);
            }
        }
    });
}

/// 视图自己或者哪一层父视图隐藏了（UIKit 没有 `isHiddenOrHasHiddenAncestor`）。
fn hidden(view: &UIView) -> bool {
    if view.isHidden() {
        return true;
    }
    let mut current = view.superview();
    while let Some(v) = current {
        if v.isHidden() {
            return true;
        }
        current = v.superview();
    }
    false
}

impl Shared {
    fn forward(&self, code: &'static str, down: bool) {
        let key = KeyInput { code: code.to_owned(), down, repeat: false };
        if let Some(proxy) = &self.proxy {
            proxy.command(KEY.name(), &KEY.encode(&key));
        }
        self.ctx.emit(KEY_EVENT.name(), KEY_EVENT.encode(&key));
    }

    /// 每帧读一遍手柄和硬件键盘：App 在前台、拿着焦点，视图在窗口里、自己和父视图都没隐藏时才读，不然按「什么都没按」算，按着的就松开了。
    fn poll_input(&self) {
        let eligible = APP_ACTIVE.with(Cell::get) && INPUT_ACTIVE.with(Cell::get) && self.view.window().is_some() && !hidden(&self.view);
        if !eligible {
            return self.release_input();
        }
        let mut keys = BTreeSet::new();
        gc::read_keyboard(&mut keys);
        self.update_keyboard(&keys);
        let readings = self.pads.borrow_mut().read();
        self.update_pads(&readings.iter().map(|(_, r)| *r).collect::<Vec<_>>());
        let states = readings.iter().map(|(id, r)| (*id, r.state())).collect();
        let changed = self.devices.borrow_mut().update(states);
        self.forward_devices(changed);
        for rumble in self.rumbles.take() {
            self.pads.borrow_mut().rumble(&rumble);
        }
    }

    /// 按着的键、手柄全部补发松开，摇杆归零；手柄还算连着。这时候要震的不震了。
    fn release_input(&self) {
        self.update_keyboard(&BTreeSet::new());
        self.update_pads(&[]);
        let released = self.devices.borrow_mut().release();
        self.forward_devices(released);
        self.rumbles.take();
    }

    /// 每个手柄各自的状态（按设备的那条路）。
    fn forward_devices(&self, changed: Vec<PadInput>) {
        let Some(proxy) = &self.proxy else { return };
        for input in changed {
            proxy.command(PAD.name(), &PAD.encode(&input));
        }
    }

    fn update_keyboard(&self, now: &BTreeSet<&'static str>) {
        let changes = self.keyboard.borrow_mut().update(now);
        for (code, down) in changes {
            self.forward(code, down);
        }
    }

    fn update_pads(&self, readings: &[pad::Reading]) {
        let (now, analog) = pad::merge(readings);
        let changes = pad::diff(&self.pad_keys.borrow(), &now, self.analog.get(), analog);
        *self.pad_keys.borrow_mut() = now;
        self.analog.set(analog);
        for code in changes.released {
            self.forward(code, false);
        }
        for code in changes.pressed {
            self.forward(code, true);
        }
        if let (Some(analog), Some(proxy)) = (changes.analog, &self.proxy) {
            proxy.command(GAMEPAD.name(), &GAMEPAD.encode(&analog.input()));
        }
    }

    fn viewport(&self) -> Viewport {
        let size = self.view.bounds().size;
        let scale = unsafe { self.view.traitCollection().displayScale() };
        Viewport {
            size: [size.width as f32, size.height as f32],
            pixels: [(size.width * scale) as u32, (size.height * scale) as u32],
        }
    }
}

define_class!(
    /// 承载 surface 的 UIView：离开窗口前同步暂停渲染线程。
    #[unsafe(super(UIView))]
    #[thread_kind = MainThreadOnly]
    #[name = "NgeIosHostView"]
    #[ivars = OnceCell<Weak<Shared>>]
    struct HostView;

    impl HostView {
        #[unsafe(method(willMoveToWindow:))]
        fn will_move_to_window(&self, window: Option<&UIWindow>) {
            // 离开或换到另一个窗口都先暂停，didMoveToWindow 里再恢复
            self.set_in_window(false);
            let _: () = unsafe { msg_send![super(self), willMoveToWindow: window] };
        }

        #[unsafe(method(didMoveToWindow))]
        fn did_move_to_window(&self) {
            let _: () = unsafe { msg_send![super(self), didMoveToWindow] };
            if self.window().is_some() {
                self.set_in_window(true);
            }
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
    #[name = "NgeIosTicker"]
    #[ivars = Rc<Shared>]
    struct Ticker;

    impl Ticker {
        #[unsafe(method(tick:))]
        fn tick(&self, _link: &CADisplayLink) {
            let shared = self.ivars();
            shared.poll_input();
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
}

impl GameView {
    pub fn new(ctx: ComponentContext, factory: GameFactory) -> Self {
        let mtm = MainThreadMarker::new().expect("原生组件必须在主线程创建");
        let view = HostView::new(mtm);

        let rumbles = rumble::Queue::default();
        let window = RawWindowHandle::UiKit(UiKitWindowHandle::new(NonNull::from(&*view).cast()));
        let display = RawDisplayHandle::UiKit(UiKitDisplayHandle::new());
        // SAFETY: view 由 Shared 持有，且 Shared 先释放 proxy（Engine）再释放 view
        let proxy = match unsafe { create_surface(window, display) } {
            Ok((instance, surface)) => {
                let emit_ctx = ctx.clone();
                // 引擎发的震动截下来，不交给 NSH
                let emit = rumbles.wrap(Box::new(move |n: &str, d| emit_ctx.emit(n, d)));
                match Engine::spawn(instance, surface, emit, factory, Box::new(view.clone())) {
                    Ok(engine) => Some(Proxy::new(engine, APP_ACTIVE.with(Cell::get))),
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
            keyboard: RefCell::default(),
            pad_keys: RefCell::default(),
            analog: Cell::default(),
            pads: RefCell::default(),
            devices: RefCell::default(),
            rumbles,
        });
        let _ = shared.view.ivars().set(Rc::downgrade(&shared));
        LIVE.with(|live| {
            let mut live = live.borrow_mut();
            live.retain(|w| w.strong_count() > 0);
            live.push(Rc::downgrade(&shared));
        });

        let ticker = Ticker::new(mtm, shared.clone());
        let link = unsafe { CADisplayLink::displayLinkWithTarget_selector(&ticker, sel!(tick:)) };
        // ProMotion 机型请求到 120Hz（还需 Info.plist 的 CADisableMinimumFrameDurationOnPhone）
        link.setPreferredFrameRateRange(CAFrameRateRange { minimum: 30.0, maximum: 120.0, preferred: 120.0 });
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        Self { shared, link }
    }
}

impl NativeComponent for GameView {
    fn view(&self) -> &UIView {
        &self.shared.view
    }

    fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
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

impl Drop for GameView {
    fn drop(&mut self) {
        // 断开 display link → Ticker → Shared；最后一个 Rc 释放时 Engine 会 join 渲染线程
        self.link.invalidate();
    }
}
