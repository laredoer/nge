//! M1 验收示例：上万个精灵的 ECS + 实例化渲染压测，界面上显示 FPS 和最长帧间隔。

use nsh_base::theme;
use nsh_core::prelude::*;
use nsh_core::{TouchEvent, TouchPhase};
use serde_json::json;

mod game;

pub use game::COUNT_ENV;

/// 宿主注册的游戏渲染组件名。
pub const GAME_COMPONENT: &str = "game.view";

pub fn factory() -> nge_core::GameFactory {
    nge_ecs::factory(game::app)
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

fn forward_touch(game: ElementRef, e: &TouchEvent) {
    let phase = match e.phase {
        TouchPhase::Start => "start",
        TouchPhase::Move => "move",
        TouchPhase::End => "end",
        TouchPhase::Cancel => "cancel",
    };
    for p in e.points.iter().filter(|p| p.changed) {
        game.command("touch", json!({ "phase": phase, "id": p.id, "x": p.location.x.0, "y": p.location.y.0 }));
    }
}

#[component]
pub fn App() -> impl IntoElement {
    let count = signal(0_u64);
    let fps = signal(0_u64);
    let worst_ms = signal(0_u64);
    let game = element_ref();

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

    let canvas = native(GAME_COMPONENT)
        .flex_1()
        .element_ref(&game)
        .on_touch(move |e: &TouchEvent| forward_touch(game, e))
        .on_native_event(move |e: &NativeEvent| match &*e.name {
            "count" => count.set(e.data["count"].as_u64().unwrap_or(0)),
            "fps" => {
                log::debug!("fps {} worst {}ms", e.data["fps"], e.data["worst_ms"]);
                fps.set(e.data["fps"].as_u64().unwrap_or(0));
                worst_ms.set(e.data["worst_ms"].as_u64().unwrap_or(0));
            }
            "error" => log::error!("引擎错误: {}", e.data),
            _ => {}
        });

    let controls = view()
        .flex_row()
        .justify_center()
        .gap_3()
        .px_4()
        .py_3()
        .child(control("+1000", move || game.command("add", json!({ "n": 1000 }))))
        .child(control("-1000", move || game.command("remove", json!({ "n": 1000 }))))
        .child(control("+10000", move || game.command("add", json!({ "n": 10000 }))))
        .child(control("卡主线程 200ms", move || game.command("nge.stall_main", json!({ "ms": 200 }))));

    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(view().flex_1().p_safe_area().child(header).child(canvas).child(controls))
}
