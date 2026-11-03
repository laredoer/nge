//! macOS：引擎挂在一个 NSView 上；主线程的 CADisplayLink 只采样视口，出帧在渲染线程。

use std::cell::OnceCell;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use nge_core::{Engine, GameFactory, Viewport, create_surface};
use nsh_platform_macos::{ComponentContext, ComponentFactory, NativeComponent};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSView;
use objc2_foundation::{NSObject, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::CADisplayLink;
use raw_window_handle::{AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle};
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

impl Shared {
    fn viewport(&self) -> Viewport {
        let bounds = self.view.bounds().size;
        let backing = self.view.convertSizeToBacking(bounds);
        Viewport {
            size: [bounds.width as f32, bounds.height as f32],
            pixels: [backing.width as u32, backing.height as u32],
        }
    }
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
        view.setWantsLayer(true);

        let window = RawWindowHandle::AppKit(AppKitWindowHandle::new(NonNull::from(&*view).cast()));
        let display = RawDisplayHandle::AppKit(AppKitDisplayHandle::new());
        // SAFETY: view 由 Shared 持有，且 Shared 先释放 proxy（Engine）再释放 view
        let proxy = match unsafe { create_surface(window, display) } {
            Ok((instance, surface)) => {
                let emit_ctx = ctx.clone();
                let emit = Box::new(move |n: &str, d| emit_ctx.emit(n, d));
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
        let shared = Rc::new(Shared { proxy, view });
        let _ = shared.view.ivars().set(Rc::downgrade(&shared));

        let ticker = Ticker::new(mtm, shared.clone());
        // NSView 的 display link 跟随视图所在屏幕
        let link = unsafe { shared.view.displayLinkWithTarget_selector(&ticker, sel!(tick:)) };
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        Self { shared, link }
    }
}

impl NativeComponent for GameView {
    fn view(&self) -> Retained<NSView> {
        self.shared.view.clone().into_super()
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
