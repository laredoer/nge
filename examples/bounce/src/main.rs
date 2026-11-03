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
    components.register(bounce::GAME_COMPONENT, nge_platform::macos::component(bounce::factory()));
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<bounce::App>()),
        platform::AppOptions {
            title: "nge bounce".to_owned(),
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            app_id: "com.lewu.nshgame".to_owned(),
            size: (720, 640),
            components,
            ..Default::default()
        },
    )
}

#[cfg(target_os = "ios")]
fn main() {
    env_logger::Builder::new().parse_filters("bounce=debug,nge_core=info,nge_platform=info,warn").init();
    let factory = bounce::factory();
    nge_platform::ios_app::run_app(nsh_platform_ios::Root::of::<bounce::App>(), move |components| {
        components.register(bounce::GAME_COMPONENT, nge_platform::ios::component(factory.clone()));
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows", target_os = "ios")))]
compile_error!(
    "Run this binary on macOS, Windows or Linux; use the TSX entry with nsh run for iOS and Android."
);
