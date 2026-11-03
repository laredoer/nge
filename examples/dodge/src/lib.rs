//! M2 验收示例：游戏逻辑全在 ECS 里（渲染线程），HUD、暂停、难度、结束面板用 NSH 写，
//! 两边只通过 `protocol` 里的类型化通道通信。

use nge_core::FrameReport;
use nge_nsh::{FRAME_STATS, GameRef, GameViewExt, game_ref, game_view};
use nsh_base::theme;
use nsh_core::prelude::*;

mod game;
pub mod protocol;

use protocol::{CONTROL, Control, DIFFICULTY, Difficulty, GAME_OVER, GameOver, HUD, Hud};

pub fn factory() -> nge_core::GameFactory {
    nge_ecs::factory(game::app)
}

#[native_module("dodge/native")]
fn ui_set(registry: &mut Registry) {
    registry.register::<App>();
}

fn button(label: impl IntoReactive<SharedString>, on_press: impl Fn() + 'static) -> impl IntoElement {
    view()
        .px_4()
        .py_2()
        .rounded(px(8.0))
        .bg(move || theme().primary)
        .active(|s| s.opacity(0.6))
        .on_press(on_press)
        .child(text(label).text_size(px(15.0)).text_color(move || theme().primary_foreground))
}

fn hearts(lives: u32) -> String {
    "♥".repeat(lives as usize) + &"♡".repeat(protocol::START_LIVES.saturating_sub(lives) as usize)
}

fn controls(game: GameRef, hud: Signal<Hud>, difficulty: Signal<Difficulty>) -> impl IntoElement {
    view()
        .flex_row()
        .justify_center()
        .gap_3()
        .child(button(
            move || SharedString::from(if hud.get().paused { "继续" } else { "暂停" }),
            move || game.send(CONTROL, &if hud.get().paused { Control::Resume } else { Control::Pause }),
        ))
        .child(button(
            move || {
                SharedString::from(match difficulty.get() {
                    Difficulty::Normal => "难度：普通",
                    Difficulty::Hard => "难度：困难",
                })
            },
            move || {
                difficulty.update(|d| {
                    *d = match d {
                        Difficulty::Normal => Difficulty::Hard,
                        Difficulty::Hard => Difficulty::Normal,
                    }
                })
            },
        ))
        .child(button("卡主线程 200ms", move || game.command("nge.stall_main", serde_json::json!({ "ms": 200 }))))
}

fn game_over_panel(game: GameRef, over: Signal<Option<GameOver>>) -> impl IntoElement {
    view()
        .flex_row()
        .justify_center()
        .items_center()
        .gap_3()
        .child(
            text(move || match over.get() {
                Some(o) if o.new_best => format!("游戏结束 · 得分 {} · 新纪录！", o.score),
                Some(o) => format!("游戏结束 · 得分 {} · 最高 {}", o.score, o.best),
                None => String::new(),
            })
            .text_size(px(17.0)),
        )
        .child(button("再来一局", move || {
            over.set(None);
            game.send(CONTROL, &Control::Restart);
        }))
}

#[component]
pub fn App() -> impl IntoElement {
    let game = game_ref();
    let hud = signal(Hud::default());
    let stats = signal(FrameReport::default());
    let over = signal(None::<GameOver>);
    let difficulty = signal(Difficulty::Normal);

    let header = view()
        .flex_row()
        .justify_between()
        .items_center()
        .px_4()
        .py_3()
        .child(text(move || format!("得分 {}", hud.get().score)).text_size(px(22.0)))
        .child(text(move || hearts(hud.get().lives)).text_size(px(20.0)).text_color(|| rgb(0xff4d6d)))
        .child(
            text(move || {
                let s = stats.get();
                format!("最高 {}  ·  {} FPS  ·  最长帧 {}ms", hud.get().best, s.fps, s.worst_ms)
            })
            .text_size(px(14.0)),
        );

    let canvas = game_view(game)
        .flex_1()
        .game_prop(DIFFICULTY, move || difficulty.get())
        .sync_state(HUD, hud)
        .on_game_event(GAME_OVER, move |e| over.set(Some(e)))
        .on_game_event(FRAME_STATS, move |s| stats.set(s));

    let footer = view().px_4().py_3().child(
        when(move || over.get().is_some(), move || game_over_panel(game, over))
            .otherwise(move || controls(game, hud, difficulty)),
    );

    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(view().flex_1().p_safe_area().child(header).child(canvas).child(footer))
}
