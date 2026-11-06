//! M4 验收示例：带贴图的 glTF 模型。3D 场景在 ECS 里（渲染线程），HUD 用 NSH 写。

use nge_nsh::{FRAME_STATS, GameViewExt, game_ref, game_view};
use nsh_base::theme;
use nsh_core::prelude::*;

pub mod game;

pub fn factory() -> nge_core::GameFactory {
    nge_ecs::factory(game::app)
}

/// 把游戏视图注册进 `nsh run` / `nsh package` 生成的宿主（见 docs/generated-hosts.md）。
#[cfg(target_os = "macos")]
#[native_components(macos)]
fn components(components: &nsh_platform_macos::Components) {
    components.register(nge_core::GAME_COMPONENT, nge_platform::macos::component(factory()));
}

#[cfg(target_os = "ios")]
#[native_components(ios)]
fn components(components: &nsh_platform_ios::Components) {
    components.register(nge_core::GAME_COMPONENT, nge_platform::ios::component(factory()));
}

#[native_module("model/native")]
fn ui_set(registry: &mut Registry) {
    registry.register::<App>();
}

#[component]
pub fn App() -> impl IntoElement {
    let fps = signal(0_u32);
    let paused = signal(false);
    let game = game_ref();

    let header = view()
        .flex_row()
        .justify_between()
        .items_center()
        .px_4()
        .py_3()
        .child(text("nge glTF 示例").text_size(px(20.0)))
        .child(text(move || format!("{} FPS · 拖动画面绕着看", fps.get())).text_size(px(15.0)));

    let canvas = game_view(game)
        .flex_1()
        .game_prop(game::PAUSED, move || game::Paused(paused.get()))
        .on_game_event(FRAME_STATS, move |s| {
            log::debug!("fps {} worst {}ms", s.fps, s.worst_ms);
            fps.set(s.fps);
        });

    let toggle = view()
        .self_center()
        .my_3()
        .px_4()
        .py_2()
        .rounded(px(8.0))
        .bg(move || theme().primary)
        .active(|s| s.opacity(0.6))
        .on_press(move || paused.update(|p| *p = !*p))
        .child(
            text(move || SharedString::from(if paused.get() { "继续自转" } else { "暂停自转" }))
                .text_size(px(15.0))
                .text_color(move || theme().primary_foreground),
        );

    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(view().flex_1().p_safe_area().child(header).child(canvas).child(toggle))
}
