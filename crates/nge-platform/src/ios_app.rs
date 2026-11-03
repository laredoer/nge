//! iOS 应用宿主。nsh 0.3 的 `run_app` 不支持注册原生组件，
//! 这里照它的结构自建 AppDelegate / SceneDelegate，用 AppView 挂载 NSH，并让应用注册自己的组件。

use std::cell::RefCell;

use nsh_platform_ios::{AppView, Components, Root, SurfaceOptions};
use objc2::rc::Retained;
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
use objc2_ui_kit::{
    UIApplication, UIApplicationDelegate, UIColor, UIResponder, UIScene, UISceneConfiguration,
    UISceneConnectionOptions, UISceneDelegate, UISceneSession, UIViewController, UIWindow,
    UIWindowScene, UIWindowSceneDelegate, UIWindowSceneSessionRoleApplication,
};

/// 启动参数：根组件 + 注册组件的回调。
struct Launch {
    root: Root,
    register: Box<dyn Fn(&Components)>,
}

struct AppState {
    _window: Retained<UIWindow>,
    _view: AppView,
}

thread_local! {
    static LAUNCH: RefCell<Option<Launch>> = const { RefCell::new(None) };
    static APP: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

define_class!(
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NgeAppDelegate"]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl UIApplicationDelegate for AppDelegate {
        #[unsafe(method_id(application:configurationForConnectingSceneSession:options:))]
        fn configuration_for_scene(
            &self,
            _application: &UIApplication,
            session: &UISceneSession,
            _options: &UISceneConnectionOptions,
        ) -> Retained<UISceneConfiguration> {
            let config = UISceneConfiguration::initWithName_sessionRole(
                UISceneConfiguration::alloc(self.mtm()),
                None,
                &session.role(),
            );
            unsafe { config.setDelegateClass(Some(SceneDelegate::class())) };
            config
        }
    }
);

define_class!(
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NgeSceneDelegate"]
    struct SceneDelegate;

    unsafe impl NSObjectProtocol for SceneDelegate {}

    unsafe impl UISceneDelegate for SceneDelegate {
        #[unsafe(method(scene:willConnectToSession:options:))]
        fn will_connect(
            &self,
            scene: &UIScene,
            session: &UISceneSession,
            _options: &UISceneConnectionOptions,
        ) {
            if *session.role() != *unsafe { UIWindowSceneSessionRoleApplication } {
                return;
            }
            if let Some(scene) = scene.downcast_ref::<UIWindowScene>() {
                connect(self.mtm(), scene);
            }
        }

        #[unsafe(method(sceneDidEnterBackground:))]
        fn did_enter_background(&self, _scene: &UIScene) {
            crate::ios::set_app_active(false);
        }

        #[unsafe(method(sceneWillEnterForeground:))]
        fn will_enter_foreground(&self, _scene: &UIScene) {
            crate::ios::set_app_active(true);
        }
    }

    unsafe impl UIWindowSceneDelegate for SceneDelegate {}
);

fn connect(mtm: MainThreadMarker, scene: &UIWindowScene) {
    let Some(launch) = LAUNCH.with(|l| l.borrow_mut().take()) else {
        return log::error!("没有要启动的应用");
    };
    let window = UIWindow::initWithWindowScene(UIWindow::alloc(mtm), scene);
    let controller = UIViewController::new(mtm);
    let root = controller.view().expect("view controller has a view");
    root.setBackgroundColor(Some(&UIColor::blackColor()));
    window.setRootViewController(Some(&controller));
    window.makeKeyAndVisible();

    let components = Components::default();
    (launch.register)(&components);
    let view = AppView::new(
        mtm,
        root.bounds(),
        SurfaceOptions {
            components,
            limits: nsh_engine::Limits::standalone(),
            ..SurfaceOptions::default()
        },
    );
    root.addSubview(view.view());
    view.run(launch.root);
    APP.with(|app| {
        *app.borrow_mut() = Some(AppState {
            _window: window,
            _view: view,
        })
    });
}

/// 以 `root` 为根组件启动 UIKit 主循环，`register` 用来注册原生组件。不返回。
pub fn run_app(root: Root, register: impl Fn(&Components) + 'static) -> ! {
    let mtm = MainThreadMarker::new().expect("必须在主线程启动");
    LAUNCH.with(|l| *l.borrow_mut() = Some(Launch { root, register: Box::new(register) }));
    let delegate = NSString::from_str(AppDelegate::class().name().to_str().expect("类名是 UTF-8"));
    UIApplication::main(None, Some(&delegate), mtm)
}
