//! 关卡编辑器：界面、关卡数据、编辑逻辑都在 TSX 里；游戏视图就是平台跳跃的引擎（渲染、资源、角色控制），
//! 编辑时场景里没有角色、时间缩放是 0，试玩时整套平台跳跃照常跑（见 docs/editor.md）。

pub fn factory() -> nge_core::GameFactory {
    nge_ecs::factory(platformer::app)
}

/// `nsh run` / `nsh package` 生成的宿主要求应用的 crate 有它；资源用的是平台跳跃的（`asset:/@platformer/…`）。
#[nsh_core::prelude::native_module("editor/native")]
fn native(_registry: &mut nsh_core::prelude::Registry) {}

/// 把游戏视图注册进生成的宿主。
#[cfg(target_os = "macos")]
#[nsh_core::prelude::native_components(macos)]
fn components(components: &nsh_platform_macos::Components) {
    components.register(nge_core::GAME_COMPONENT, nge_platform::macos::component(factory()));
}

#[cfg(target_os = "ios")]
#[nsh_core::prelude::native_components(ios)]
fn components(components: &nsh_platform_ios::Components) {
    components.register(nge_core::GAME_COMPONENT, nge_platform::ios::component(factory()));
}
