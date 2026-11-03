//! Runs the Rust application directly on the machine's native backend.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
use nsh_platform_linux as platform;
#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;
#[cfg(target_os = "windows")]
use nsh_platform_windows as platform;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn main() -> impl std::process::Termination {
    env_logger::init();
    let components = platform::Components::default();
    #[cfg(target_os = "macos")]
    components.register(
        nsh_app_nsh_game::GAME_COMPONENT,
        std::rc::Rc::new(|ctx| Box::new(nsh_app_nsh_game::game_macos::GameView::new(ctx))),
    );
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<nsh_app_nsh_game::App>()),
        platform::AppOptions {
            title: "nsh-game".to_owned(),
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            app_id: "com.example.nshgame".to_owned(),
            size: (720, 640),
            components,
            ..Default::default()
        },
    )
}

#[cfg(target_os = "ios")]
fn main() {
    env_logger::Builder::new().parse_filters("nsh_app_nsh_game=debug,warn").init();
    nsh_app_nsh_game::ios_host::run()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows", target_os = "ios")))]
compile_error!(
    "Run this binary on macOS, Windows or Linux; use the TSX entry with nsh run for iOS and Android."
);
