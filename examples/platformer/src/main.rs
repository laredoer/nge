//! TSX 开发：先在本目录跑 `nsh dev --server-only`，再 `cargo run -p platformer`。

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    nge_platform::macos::dev_host(platformer::factory())
}

// 其他平台还没有 TSX dev host；留空实现，不挡住工作区级别的构建
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("platformer 示例目前只支持 macOS");
}
