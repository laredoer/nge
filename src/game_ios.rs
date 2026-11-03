//! iOS 上的游戏视图：一个 UIView 交给 game_core 渲染（Metal），
//! CADisplayLink 驱动帧循环（ProMotion 机型可到 120Hz）。

use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;

use nsh_platform_ios::{ComponentContext, NativeComponent};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{NSObject, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::{CADisplayLink, CAFrameRateRange};
use objc2_ui_kit::{UIView, UITraitEnvironment};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, UiKitDisplayHandle, UiKitWindowHandle};
use serde_json::Value as JsonValue;

use crate::game_core::{Game, Target, Viewport};

const MAX_FPS: f32 = 120.0;

struct State {
    view: Retained<UIView>,
    game: Game,
}

impl State {
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
    /// CADisplayLink 的回调目标：每次屏幕刷新推进一帧。
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NshGameTicker"]
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
    view: Retained<UIView>,
    state: Rc<RefCell<State>>,
    link: Retained<CADisplayLink>,
}

impl GameView {
    pub fn new(ctx: ComponentContext) -> Self {
        let mtm = MainThreadMarker::new().expect("原生组件必须在主线程创建");
        let view = UIView::new(mtm);
        let target = Target {
            window: RawWindowHandle::UiKit(UiKitWindowHandle::new(NonNull::from(&*view).cast())),
            display: RawDisplayHandle::UiKit(UiKitDisplayHandle::new()),
        };
        let game = Game::new(target, Box::new(move |name, data| ctx.emit(name, data)));
        let state = Rc::new(RefCell::new(State { view: view.clone(), game }));

        let ticker = Ticker::new(mtm, state.clone());
        let link = unsafe { CADisplayLink::displayLinkWithTarget_selector(&ticker, sel!(tick:)) };
        link.setPreferredFrameRateRange(CAFrameRateRange { minimum: 30.0, maximum: MAX_FPS, preferred: MAX_FPS });
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        Self { view, state, link }
    }
}

impl NativeComponent for GameView {
    fn view(&self) -> &UIView {
        &self.view
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
