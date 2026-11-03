//! The app's Rust UI: its root component, which `nsh run`, `nsh package`
//! and `cargo run` mount.

use nsh_base::theme;
use nsh_core::prelude::*;
use nsh_core::{TouchEvent, TouchPhase};
use serde_json::json;

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod game_core;
#[cfg(target_os = "macos")]
pub mod game_macos;
#[cfg(target_os = "ios")]
mod game_ios;
#[cfg(target_os = "ios")]
pub mod ios_host;

/// 宿主注册的游戏渲染组件名（各平台保持一致）。
pub const GAME_COMPONENT: &str = "game.view";

/// The app's UI set (`nsh.json` `"rust"`): what a TSX entry may import from
/// `nsh-game/native` (`README.md`, "Adding a TSX entry").
#[native_module("nsh-game/native")]
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
        .child(
            text(label)
                .text_size(px(15.0))
                .text_color(move || theme().primary_foreground),
        )
}

/// 把触摸转发给引擎：只发这次变化的手指，坐标是相对游戏视图的 point。
fn forward_touch(game: ElementRef, e: &TouchEvent) {
    let phase = match e.phase {
        TouchPhase::Start => "start",
        TouchPhase::Move => "move",
        TouchPhase::End => "end",
        TouchPhase::Cancel => "cancel",
    };
    for p in e.points.iter().filter(|p| p.changed) {
        game.command(
            "touch",
            json!({ "phase": phase, "id": p.id, "x": p.location.x.0, "y": p.location.y.0, "t": e.timestamp }),
        );
    }
}

/// The application's root component.
#[component]
pub fn App() -> impl IntoElement {
    let score = signal(0_u64);
    let balls = signal(0_u64);
    let fps = signal(0_u64);
    let paused = signal(false);
    let speed = signal(1.0_f64);
    let game = element_ref();

    let header = view()
        .flex_row()
        .justify_between()
        .items_center()
        .px_4()
        .py_3()
        .child(text("NSH × wgpu 弹球").text_size(px(20.0)))
        .child(
            text(move || format!("得分 {}  ·  球 {}  ·  {} FPS", score.get(), balls.get(), fps.get()))
                .text_size(px(15.0)),
        );

    let canvas = native(GAME_COMPONENT)
        .flex_1()
        .element_ref(&game)
        .prop("speed", move || json!(speed.get()))
        .on_touch(move |e: &TouchEvent| forward_touch(game, e))
        .on_native_event(move |e: &NativeEvent| {
            log::debug!("game.view 事件 {} {}", e.name, e.data);
            match &*e.name {
            "score" => {
                score.set(e.data["score"].as_u64().unwrap_or(0));
                balls.set(e.data["balls"].as_u64().unwrap_or(0));
            }
            "fps" => fps.set(e.data["fps"].as_u64().unwrap_or(0)),
            "state" => paused.set(e.data["paused"].as_bool().unwrap_or(false)),
            _ => {}
            }
        });

    let controls = view()
        .flex_row()
        .justify_center()
        .gap_3()
        .px_4()
        .py_3()
        .child(control(
            move || SharedString::from(if paused.get() { "继续" } else { "暂停" }),
            move || game.command(if paused.get() { "resume" } else { "pause" }, json!(null)),
        ))
        .child(control("加一个球", move || game.command("spawn", json!(null))))
        .child(control("重置", move || game.command("reset", json!(null))))
        .child(control("减速", move || speed.update(|s| *s = (*s - 0.25).max(0.25))))
        .child(control(move || SharedString::from(format!("加速 ×{:.2}", speed.get())), move || {
            speed.update(|s| *s = (*s + 0.25).min(4.0))
        }));

    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(view().flex_1().p_safe_area().child(header).child(canvas).child(controls))
}
