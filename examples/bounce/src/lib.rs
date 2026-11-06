//! The app's Rust UI: its root component, which `nsh run` and `nsh package` mount.

use nge_nsh::{FRAME_STATS, GameViewExt, game_ref, game_view};
use nsh_base::theme;
use nsh_core::prelude::*;
use serde_json::json;

mod game;
mod render;
mod sim;

/// 环境变量：每隔这么多毫秒卸载 / 重新挂载一次游戏视图，压测引擎的创建和释放。
pub const REMOUNT_ENV: &str = "NGE_DEBUG_REMOUNT_MS";

/// 引擎在渲染线程上用它创建游戏。
pub fn factory() -> nge_core::GameFactory {
    std::sync::Arc::new(|gfx| Box::new(game::Bounce::new(gfx)))
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

/// The app's UI set (`nsh.json` `"rust"`): what a TSX entry may import from
/// `nsh-game/native`.
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

/// The application's root component.
#[component]
pub fn App() -> impl IntoElement {
    let score = signal(0_u64);
    let balls = signal(0_u64);
    let fps = signal(0_u32);
    let worst_ms = signal(0_u32);
    let paused = signal(false);
    let speed = signal(1.0_f64);
    let game = game_ref();
    let mounted = signal(true);

    if let Some(ms) = std::env::var(REMOUNT_ENV).ok().and_then(|v| v.parse::<u64>().ok()).filter(|&ms| ms > 0) {
        log::warn!("压测：每 {ms}ms 卸载 / 挂载一次游戏视图");
        let _ = set_interval(std::time::Duration::from_millis(ms), move || mounted.update(|m| *m = !*m));
    }

    let header = view()
        .flex_row()
        .justify_between()
        .items_center()
        .px_4()
        .py_3()
        .child(text("nge 弹球示例").text_size(px(20.0)))
        .child(
            text(move || format!("得分 {}  ·  球 {}  ·  {} FPS  ·  最长帧 {}ms", score.get(), balls.get(), fps.get(), worst_ms.get()))
                .text_size(px(15.0)),
        );

    // bounce 直接实现 `Game`（不用 ECS），自己的事件仍按名字手动处理
    let canvas = move || game_view(game)
        .flex_1()
        .prop("speed", move || json!(speed.get()))
        .on_game_event(FRAME_STATS, move |s| {
            fps.set(s.fps);
            worst_ms.set(s.worst_ms);
        })
        .on_native_event(move |e: &NativeEvent| {
            log::debug!("game.view 事件 {} {}", e.name, e.data);
            match &*e.name {
            "score" => {
                score.set(e.data["score"].as_u64().unwrap_or(0));
                balls.set(e.data["balls"].as_u64().unwrap_or(0));
            }
            "state" => paused.set(e.data["paused"].as_bool().unwrap_or(false)),
            "error" => log::error!("引擎错误: {}", e.data),
            _ => {}
            }
        });
    let canvas = when(mounted, canvas).otherwise(|| view().flex_1());

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
        .child(control(
            move || SharedString::from(if mounted.get() { "卸载" } else { "挂载" }),
            move || mounted.update(|m| *m = !*m),
        ))
        .child(control("卡主线程 200ms", move || game.command("nge.stall_main", json!({ "ms": 200 }))))
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
