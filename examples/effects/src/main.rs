//! 直接在本机原生后端上跑。

#[cfg(target_os = "macos")]
fn main() -> impl std::process::Termination {
    use nsh_platform_macos as platform;
    env_logger::init();
    let components = platform::Components::default();
    components.register(nge_core::GAME_COMPONENT, nge_platform::macos::component(effects::factory()));
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<effects::App>()),
        platform::AppOptions { title: "nge 特效示例".to_owned(), size: (800, 700), components, ..Default::default() },
    )
}

#[cfg(target_os = "ios")]
fn main() {
    env_logger::Builder::new().parse_filters("effects=info,nge_core=info,nge_platform=info,warn").init();
    let factory = effects::factory();
    nge_platform::ios_app::run_app(nsh_platform_ios::Root::of::<effects::App>(), move |components| {
        components.register(nge_core::GAME_COMPONENT, nge_platform::ios::component(factory.clone()));
    })
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
compile_error!("effects 示例目前只支持 macOS 和 iOS");
