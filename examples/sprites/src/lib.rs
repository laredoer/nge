//! M1 验收示例：上万个精灵的 ECS + 实例化渲染压测，界面上显示 FPS 和最长帧间隔。

use nge_nsh::{FRAME_STATS, GameViewExt, game_ref, game_view};
use nsh_base::theme;
use nsh_core::prelude::*;
use serde_json::json;

mod game;

pub use game::COUNT_ENV;
use game::{ADD, Add, COUNT, REMOVE, Remove};

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

#[native_module("sprites/native")]
fn ui_set(registry: &mut Registry) {
    registry.register::<App>();
}

fn control(label: impl IntoReactive<SharedString>, on_press: impl Fn() + 'static) -> impl IntoElement {
    view()
        .px_4()
        .py_2()
        .rounded(px(8.0))
        .bg(move || theme().primary)
        .active(|s| s.opacity(0.6))
        .on_press(on_press)
        .child(text(label).text_size(px(15.0)).text_color(move || theme().primary_foreground))
}

#[component]
pub fn App() -> impl IntoElement {
    let count = signal(0_u64);
    let fps = signal(0_u32);
    let worst_ms = signal(0_u32);
    let game = game_ref();

    let header = view()
        .flex_row()
        .justify_between()
        .items_center()
        .px_4()
        .py_3()
        .child(text("nge 精灵压测").text_size(px(20.0)))
        .child(
            text(move || format!("精灵 {}  ·  {} FPS  ·  最长帧 {}ms", count.get(), fps.get(), worst_ms.get()))
                .text_size(px(15.0)),
        );

    let canvas = game_view(game)
        .flex_1()
        .on_game_event(COUNT, move |n| count.set(n))
        .on_game_event(FRAME_STATS, move |s| {
            log::debug!("fps {} worst {}ms", s.fps, s.worst_ms);
            fps.set(s.fps);
            worst_ms.set(s.worst_ms);
        });

    let controls = view()
        .flex_row()
        .justify_center()
        .gap_3()
        .px_4()
        .py_3()
        .child(control("+1000", move || game.send(ADD, &Add(1000))))
        .child(control("-1000", move || game.send(REMOVE, &Remove(1000))))
        .child(control("+10000", move || game.send(ADD, &Add(10_000))))
        .child(control("卡主线程 200ms", move || game.command("nge.stall_main", json!({ "ms": 200 }))));

    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(view().flex_1().p_safe_area().child(header).child(canvas).child(controls))
}
