//! macOS 上的游戏视图：一个 NSView 交给 game_core 渲染（Metal），
//! CADisplayLink 驱动帧循环（跟随所在屏幕刷新率）。

use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;

use nsh_platform_macos::{ComponentContext, NativeComponent};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSView;
use objc2_foundation::{NSObject, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::{CADisplayLink, CAFrameRateRange};
use raw_window_handle::{AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle};
use serde_json::Value as JsonValue;

use crate::game_core::{Game, Target, Viewport};

const MAX_FPS: f32 = 120.0;

struct State {
    view: Retained<NSView>,
    game: Game,
}

impl State {
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
    /// CADisplayLink 的回调目标：每次屏幕刷新推进一帧。
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Rc<RefCell<State>>]
    struct Ticker;

    impl Ticker {
        #[unsafe(method(tick:))]
        fn tick(&self, _link: &CADisplayLink) {
            if let Ok(mut state) = self.ivars().try_borrow_mut() {
                let viewport = state.viewport();
                state.game.tick(viewport);
            }
        }
    }
);

impl Ticker {
    fn new(mtm: MainThreadMarker, state: Rc<RefCell<State>>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(state);
        unsafe { msg_send![super(this), init] }
    }
}

pub struct GameView {
    view: Retained<NSView>,
    state: Rc<RefCell<State>>,
    link: Retained<CADisplayLink>,
}

impl GameView {
    pub fn new(ctx: ComponentContext) -> Self {
        let mtm = MainThreadMarker::new().expect("原生组件必须在主线程创建");
        let view = NSView::new(mtm);
        view.setWantsLayer(true);
        let target = Target {
            window: RawWindowHandle::AppKit(AppKitWindowHandle::new(NonNull::from(&*view).cast())),
            display: RawDisplayHandle::AppKit(AppKitDisplayHandle::new()),
        };
        let game = Game::new(target, Box::new(move |name, data| ctx.emit(name, data)));
        let state = Rc::new(RefCell::new(State { view: view.clone(), game }));

        let ticker = Ticker::new(mtm, state.clone());
        // NSView 的 display link 会跟着视图所在屏幕切换刷新率
        let link = unsafe { view.displayLinkWithTarget_selector(&ticker, sel!(tick:)) };
        link.setPreferredFrameRateRange(CAFrameRateRange { minimum: 30.0, maximum: MAX_FPS, preferred: MAX_FPS });
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        Self { view, state, link }
    }
}

impl NativeComponent for GameView {
    fn view(&self) -> Retained<NSView> {
        self.view.clone()
    }

    fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
        if name == "speed" {
            self.state.borrow_mut().game.set_speed(value);
        }
    }

    fn command(&mut self, name: &str, args: &JsonValue) {
        self.state.borrow_mut().game.command(name, args);
    }
}

impl Drop for GameView {
    fn drop(&mut self) {
        // 停掉帧循环，打破 display link → Ticker → State 的引用
        self.link.invalidate();
    }
}
