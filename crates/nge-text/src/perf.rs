//! 性能面板：开着 [`PerfOverlay`] 时在左上角用文字画出 [`Perf`] 的汇总（见 docs/perf-overlay.md）。

use bevy_ecs::prelude::*;
use glam::Vec4;
use nge_ecs::{Perf, PerfReport};
use nge_render2d::Transform2d;

use crate::{Anchor, ScreenText, Text2d};

/// 资源：开着时在左上角画性能面板。
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PerfOverlay(pub bool);

/// 性能面板那段文字。
#[derive(Component)]
pub struct PerfText;

const SIZE: f32 = 12.0;
const MARGIN: f32 = 8.0;
const COLOR: Vec4 = Vec4::new(0.6, 1.0, 0.6, 1.0);

/// 面板上的几行字。
pub fn perf_lines(report: Option<&PerfReport>) -> String {
    let Some(r) = report else { return "性能统计中…".into() };
    format!(
        "{:.0} fps  帧 {:.1} ms（最长 {:.1}）\n更新 {:.1}  渲染 {:.1}  等待 {:.1} ms\ndraw {}  实例 {}  实体 {}",
        r.fps, r.frame_ms, r.worst_ms, r.update_ms, r.render_ms, r.wait_ms, r.draws, r.instances, r.entities
    )
}

/// 开关面板、跟着汇总改字（字一样时不碰，免得每帧重新排版）。
pub(crate) fn perf_overlay(
    mut commands: Commands,
    overlay: Res<PerfOverlay>,
    perf: Res<Perf>,
    mut texts: Query<(Entity, &mut Text2d), With<PerfText>>,
    mut shown: Local<Option<PerfReport>>,
) {
    let lines = || perf_lines(perf.report());
    match (overlay.0, texts.single_mut()) {
        (true, Ok((_, mut text))) => {
            // 汇总半秒才变一次，没变就不重新拼字
            if *shown != perf.report().copied() {
                *shown = perf.report().copied();
                text.text = lines();
            }
        }
        (true, Err(_)) => {
            *shown = perf.report().copied();
            commands.spawn((Text2d::new(lines()).with_size(SIZE).with_color(COLOR).with_anchor(Anchor::TopLeft), Transform2d::from_xy(MARGIN, MARGIN), ScreenText, PerfText));
        }
        (false, Ok((entity, _))) => {
            commands.entity(entity).despawn();
        }
        (false, Err(_)) => {}
    }
}

#[cfg(test)]
mod tests {
    use nge_ecs::{App, FrameTiming, PostUpdate};

    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<PerfOverlay>().add_systems(PostUpdate, perf_overlay);
        app
    }

    fn panel(app: &mut App) -> Vec<String> {
        app.world_mut().query_filtered::<&Text2d, (With<PerfText>, With<ScreenText>)>().iter(app.world()).map(|t| t.text.clone()).collect()
    }

    #[test]
    fn lines_show_every_number() {
        let r = PerfReport { fps: 59.6, frame_ms: 16.78, worst_ms: 33.3, update_ms: 1.24, render_ms: 2.5, wait_ms: 12.96, draws: 42, instances: 1830, entities: 312 };
        assert_eq!(perf_lines(Some(&r)), "60 fps  帧 16.8 ms（最长 33.3）\n更新 1.2  渲染 2.5  等待 13.0 ms\ndraw 42  实例 1830  实体 312");
        assert_eq!(perf_lines(None), "性能统计中…");
    }

    #[test]
    fn the_panel_comes_and_goes_with_the_switch_and_follows_the_report() {
        let mut app = app();
        app.tick(Default::default(), 0.016);
        assert!(panel(&mut app).is_empty(), "默认关着");
        app.world_mut().resource_mut::<PerfOverlay>().0 = true;
        app.tick(Default::default(), 0.016);
        assert_eq!(panel(&mut app), ["性能统计中…"]);
        let timing = FrameTiming { frame: 0.6, update: 0.001, render: 0.002, wait: 0.5 };
        app.world_mut().resource_mut::<Perf>().record(timing, 3, 10, || 7);
        app.tick(Default::default(), 0.016);
        let text = panel(&mut app);
        assert_eq!(text.len(), 1, "只有一个面板");
        assert!(text[0].starts_with("2 fps") && text[0].ends_with("draw 3  实例 10  实体 7"), "{text:?}");
        app.world_mut().resource_mut::<PerfOverlay>().0 = false;
        app.tick(Default::default(), 0.016);
        assert!(panel(&mut app).is_empty());
    }
}
