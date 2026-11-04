//! M5 验收示例：资源层异步加载、文字、粒子。场景在 ECS 里（渲染线程），HUD 和「加载中」用 NSH 写。

use nge_nsh::{FRAME_STATS, GameViewExt, game_ref, game_view};
use nsh_base::theme;
use nsh_core::prelude::*;

pub mod game;

pub fn factory() -> nge_core::GameFactory {
    nge_ecs::factory(|| game::app(assets!().iter().map(|f| (f.uri, f.source))))
}

#[component]
pub fn App() -> impl IntoElement {
    let fps = signal(0_u32);
    let status = signal(game::Status { loading: true, particles: 0 });
    let stress = signal(false);
    let game = game_ref();

    let header = view()
        .flex_row()
        .justify_between()
        .items_center()
        .px_4()
        .py_3()
        .child(text("nge 特效示例").text_size(px(20.0)))
        .child(text(move || format!("{} FPS · {} 个粒子", fps.get(), status.get().particles)).text_size(px(15.0)));

    let canvas = game_view(game)
        .flex_1()
        .game_prop(game::STRESS, move || game::Stress(stress.get()))
        .sync_state(game::STATUS, status)
        .on_game_event(FRAME_STATS, move |s| {
            log::debug!("fps {} worst {}ms", s.fps, s.worst_ms);
            fps.set(s.fps);
        });

    // 盖在画面上，资源加载完就去掉
    let loading = when(
        move || status.get().loading,
        || {
            view()
                .absolute()
                .inset_0()
                .items_center()
                .justify_center()
                .bg(|| rgba(0x000000b0))
                .child(text("加载中…").text_size(px(22.0)).text_color(|| rgb(0xffffff)))
        },
    );

    let toggle = view()
        .self_center()
        .my_3()
        .px_4()
        .py_2()
        .rounded(px(8.0))
        .bg(move || theme().primary)
        .active(|s| s.opacity(0.6))
        .on_press(move || stress.update(|s| *s = !*s))
        .child(
            text(move || SharedString::from(if stress.get() { "停止压力测试" } else { "压力测试：10k 粒子" }))
                .text_size(px(15.0))
                .text_color(move || theme().primary_foreground),
        );

    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(
            view()
                .flex_1()
                .p_safe_area()
                .child(header)
                .child(view().flex_1().relative().child(canvas).child(loading))
                .child(toggle),
        )
}
