//! iOS：引擎挂在一个 UIView 上；主线程的 CADisplayLink 只采样视口，出帧在渲染线程。

use std::cell::{Cell, OnceCell, RefCell};
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use nge_core::{Engine, GameFactory, Viewport, create_surface};
use nsh_platform_ios::{ComponentContext, ComponentFactory, NativeComponent};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{NSObject, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::CADisplayLink;
use objc2_quartz_core::CAFrameRateRange;
use objc2_ui_kit::{UITraitEnvironment, UIView, UIWindow};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, UiKitDisplayHandle, UiKitWindowHandle};
use serde_json::{Value as JsonValue, json};

use crate::shared::Proxy;

/// 把 `factory` 包成 NSH 原生组件工厂，注册到 `AppOptions.components`。
pub fn component(factory: GameFactory) -> ComponentFactory {
    Rc::new(move |ctx| Box::new(GameView::new(ctx, factory.clone())))
}

struct Shared {
    // 字段按声明顺序释放：proxy（内含 Engine，join 渲染线程）先于 view
    proxy: Option<Proxy>,
    view: Retained<HostView>,
}

thread_local! {
    /// 所有存活的游戏 View，App 前后台切换时逐个通知。
    static LIVE: RefCell<Vec<Weak<Shared>>> = const { RefCell::new(Vec::new()) };
    /// App 当前是否在前台；后台新建的 View 也要按它起步。
    static APP_ACTIVE: Cell<bool> = const { Cell::new(true) };
}

/// App 进后台 / 回前台（主线程）：后台不能再提交 GPU 命令，否则会被系统杀掉。
pub(crate) fn set_app_active(active: bool) {
    APP_ACTIVE.with(|a| a.set(active));
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        live.retain(|w| w.strong_count() > 0);
        for shared in live.iter().filter_map(Weak::upgrade) {
            if let Some(proxy) = &shared.proxy {
                proxy.set_active(active);
            }
        }
    });
}

impl Shared {
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

        let window = RawWindowHandle::UiKit(UiKitWindowHandle::new(NonNull::from(&*view).cast()));
        let display = RawDisplayHandle::UiKit(UiKitDisplayHandle::new());
        // SAFETY: view 由 Shared 持有，且 Shared 先释放 proxy（Engine）再释放 view
        let proxy = match unsafe { create_surface(window, display) } {
            Ok((instance, surface)) => {
                let emit_ctx = ctx.clone();
                let emit = Box::new(move |n: &str, d| emit_ctx.emit(n, d));
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
        let shared = Rc::new(Shared { proxy, view });
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
