//! 直接在本机原生后端上跑。

#[cfg(target_os = "macos")]
fn main() -> impl std::process::Termination {
    use nsh_platform_macos as platform;
    env_logger::init();
    let components = platform::Components::default();
    components.register(nge_core::GAME_COMPONENT, nge_platform::macos::component(dodge::factory()));
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<dodge::App>()),
        platform::AppOptions { title: "nge dodge".to_owned(), size: (480, 800), components, ..Default::default() },
    )
}

#[cfg(target_os = "ios")]
fn main() {
    env_logger::Builder::new().parse_filters("dodge=info,nge_core=info,nge_platform=info,warn").init();
    let factory = dodge::factory();
    nge_platform::ios_app::run_app(nsh_platform_ios::Root::of::<dodge::App>(), move |components| {
        components.register(nge_core::GAME_COMPONENT, nge_platform::ios::component(factory.clone()));
    })
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
compile_error!("dodge 示例目前只支持 macOS 和 iOS");
