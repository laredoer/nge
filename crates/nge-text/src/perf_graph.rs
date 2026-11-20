//! 性能曲线：开着 [`PerfGraph`] 时在性能面板下面画最近几帧的耗时和数量（见 docs/big-scene-perf.md）。
//! 用白色方块拼线段，交给精灵合批，一帧几百个实例。

use bevy_ecs::prelude::*;
use glam::{Vec2, Vec4};
use nge_ecs::{HISTORY, PerfHistory, PerfSample, Screen};
use nge_render2d::{Camera2d, GIZMO_Z, SpriteImage, SpriteInstance, SpriteInstances, TextureId, Transform2d};

use crate::{Anchor, ScreenText, Text2d};

/// 资源：开着时画性能曲线。
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PerfGraph(pub bool);

/// 曲线下面那行图例。
#[derive(Component)]
pub struct PerfGraphText;

/// 一张图多大（point）。
pub const GRAPH_SIZE: Vec2 = Vec2::new(240.0, 48.0);
/// 第一张图的左上角（在面板的四行字下面）。
const TOP_LEFT: Vec2 = Vec2::new(8.0, 76.0);
const GAP: f32 = 6.0;
const LINE: f32 = 1.5;
const Z: f32 = GIZMO_Z + 1.0;
const BACK: Vec4 = Vec4::new(0.0, 0.0, 0.0, 0.55);

/// 耗时的曲线：帧、更新、渲染、等待、GPU。
const TIMES: [(&str, Vec4); 5] = [
    ("帧", Vec4::new(1.0, 1.0, 1.0, 1.0)),
    ("更新", Vec4::new(0.4, 1.0, 0.4, 1.0)),
    ("渲染", Vec4::new(0.4, 0.7, 1.0, 1.0)),
    ("等待", Vec4::new(0.6, 0.6, 0.6, 1.0)),
    ("GPU", Vec4::new(1.0, 0.6, 0.2, 1.0)),
];
/// 数量的曲线：实体、draw call。
const COUNTS: [(&str, Vec4); 2] = [("实体", Vec4::new(1.0, 0.9, 0.3, 1.0)), ("draw", Vec4::new(0.9, 0.5, 1.0, 1.0))];

/// 耗时图的满格（毫秒）：不小于最大值的第一档，超过最后一档时就是最大值。
pub fn scale_ms(max: f32) -> f32 {
    const STEPS: [f32; 8] = [16.7, 33.3, 50.0, 100.0, 250.0, 500.0, 1000.0, 2000.0];
    STEPS.into_iter().find(|&s| s >= max).unwrap_or(if max.is_finite() { max } else { 2000.0 })
}

/// 数量图的满格：不小于最大值的 1、2、5 × 10ⁿ，至少 10。
pub fn scale_count(max: u32) -> u32 {
    let mut step = 10u32;
    loop {
        for s in [step, step * 2, step * 5] {
            if s >= max {
                return s;
            }
        }
        match step.checked_mul(10) {
            Some(next) if next <= u32::MAX / 5 => step = next,
            _ => return u32::MAX,
        }
    }
}

/// 一条曲线的点（屏幕坐标，y 朝下）：最新的在右边，一格一帧；`None`（没有数）把线断开。
pub fn curve_points(values: &[Option<f32>], full: f32, top_left: Vec2, size: Vec2) -> Vec<Option<Vec2>> {
    let step = size.x / (HISTORY - 1).max(1) as f32;
    let start = top_left.x + size.x - step * values.len().saturating_sub(1) as f32;
    values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let v = v.filter(|v| v.is_finite())?;
            let t = (v / full.max(f32::EPSILON)).clamp(0.0, 1.0);
            Some(Vec2::new(start + step * i as f32, top_left.y + size.y * (1.0 - t)))
        })
        .collect()
}

/// 画在屏幕上的东西换成世界坐标的方块（跟着相机转、缩放）。
struct Screen2World {
    camera: Camera2d,
    size: Vec2,
}

impl Screen2World {
    fn point(&self, p: Vec2) -> Vec2 {
        self.camera.screen_to_world(self.size, p)
    }

    fn rect(&self, top_left: Vec2, size: Vec2, color: Vec4) -> SpriteInstance {
        let position = self.point(top_left + size / 2.0);
        SpriteInstance { position, size: size / self.camera.zoom.max(1e-4), rotation: self.camera.rotation, z: Z, color }
    }

    fn segment(&self, a: Vec2, b: Vec2, color: Vec4) -> SpriteInstance {
        let (a, b) = (self.point(a), self.point(b));
        let width = LINE / self.camera.zoom.max(1e-4);
        let d = b - a;
        SpriteInstance { position: (a + b) / 2.0, size: Vec2::new(d.length() + width, width), rotation: d.y.atan2(d.x), z: Z, color }
    }

    fn curve(&self, points: &[Option<Vec2>], color: Vec4, out: &mut SpriteInstances) {
        for pair in points.windows(2) {
            if let [Some(a), Some(b)] = *pair {
                out.push(SpriteImage::Texture(TextureId::WHITE), self.segment(a, b, color));
            }
        }
    }
}

/// 两张图的满格：耗时（毫秒）、数量。
pub fn graph_scales(samples: &[PerfSample]) -> (f32, u32) {
    let ms = samples.iter().map(|s| s.frame_ms.max(s.update_ms).max(s.render_ms).max(s.wait_ms).max(s.gpu_ms.unwrap_or(0.0))).fold(0.0, f32::max);
    let count = samples.iter().map(|s| s.entities.max(s.draws)).max().unwrap_or(0);
    (scale_ms(ms), scale_count(count))
}

/// 图例：每条线的颜色名和满格。
pub fn legend(ms: f32, count: u32) -> String {
    format!("上：白 帧  绿 更新  蓝 渲染  灰 等待  橙 GPU（满格 {ms:.0} ms）\n下：黄 实体  紫 draw（满格 {count}）")
}

pub(crate) fn perf_graph(
    mut commands: Commands,
    graph: Res<PerfGraph>,
    history: Option<Res<PerfHistory>>,
    screen: Res<Screen>,
    cameras: Query<&Camera2d>,
    instances: Option<ResMut<SpriteInstances>>,
    mut texts: Query<(Entity, &mut Text2d), With<PerfGraphText>>,
) {
    if !graph.0 {
        for (entity, _) in &texts {
            commands.entity(entity).despawn();
        }
        return;
    }
    // 没装 2D 渲染、不是 EcsGame 跑的（测试里只装文字）时不画
    let (Some(history), Some(mut instances)) = (history, instances) else { return };
    let samples: Vec<PerfSample> = history.samples().copied().collect();
    let (ms, count) = graph_scales(&samples);
    let view = Screen2World { camera: cameras.iter().next().copied().unwrap_or_default(), size: screen.size };
    let lower = TOP_LEFT + Vec2::new(0.0, GRAPH_SIZE.y + GAP);
    let white = SpriteImage::Texture(TextureId::WHITE);
    instances.push(white, view.rect(TOP_LEFT, GRAPH_SIZE, BACK));
    instances.push(white, view.rect(lower, GRAPH_SIZE, BACK));
    let time_series: [fn(&PerfSample) -> Option<f32>; 5] = [|s| Some(s.frame_ms), |s| Some(s.update_ms), |s| Some(s.render_ms), |s| Some(s.wait_ms), |s| s.gpu_ms];
    for (value, (_, color)) in time_series.iter().zip(TIMES) {
        let values: Vec<Option<f32>> = samples.iter().map(value).collect();
        view.curve(&curve_points(&values, ms, TOP_LEFT, GRAPH_SIZE), color, &mut instances);
    }
    let count_series: [fn(&PerfSample) -> u32; 2] = [|s| s.entities, |s| s.draws];
    for (value, (_, color)) in count_series.iter().zip(COUNTS) {
        let values: Vec<Option<f32>> = samples.iter().map(|s| Some(value(s) as f32)).collect();
        view.curve(&curve_points(&values, count as f32, lower, GRAPH_SIZE), color, &mut instances);
    }
    let text = legend(ms, count);
    match texts.single_mut() {
        Ok((_, mut shown)) => {
            if shown.text != text {
                shown.text = text;
            }
        }
        Err(_) => {
            let at = lower + Vec2::new(0.0, GRAPH_SIZE.y + 4.0);
            commands.spawn((Text2d::new(text).with_size(11.0).with_color(Vec4::new(0.8, 0.8, 0.8, 1.0)).with_anchor(Anchor::TopLeft), Transform2d::from_xy(at.x, at.y), ScreenText, PerfGraphText));
        }
    }
}

#[cfg(test)]
mod tests {
    use nge_ecs::{App, PostUpdate};
    use nge_render2d::Render2dPlugin;

    use super::*;

    #[test]
    fn scales_round_up_to_the_next_step() {
        assert_eq!(scale_ms(0.0), 16.7);
        assert_eq!(scale_ms(16.8), 33.3);
        assert_eq!(scale_ms(120.0), 250.0);
        assert_eq!(scale_ms(5000.0), 5000.0);
        assert_eq!(scale_ms(f32::NAN), 2000.0);
        assert_eq!(scale_count(0), 10);
        assert_eq!(scale_count(11), 20);
        assert_eq!(scale_count(41), 50);
        assert_eq!(scale_count(51), 100);
        assert_eq!(scale_count(160_001), 200_000);
        assert_eq!(scale_count(u32::MAX), u32::MAX);
    }

    #[test]
    fn curves_end_at_the_right_edge_and_break_at_missing_values() {
        let p = curve_points(&[Some(0.0), None, Some(10.0), Some(40.0)], 20.0, Vec2::new(10.0, 100.0), Vec2::new(119.0 * 2.0, 50.0));
        assert_eq!(p[3], Some(Vec2::new(10.0 + 238.0, 100.0)), "超过满格的顶到上边");
        assert_eq!(p[2], Some(Vec2::new(10.0 + 236.0, 125.0)));
        assert_eq!(p[1], None);
        assert_eq!(p[0], Some(Vec2::new(10.0 + 232.0, 150.0)), "0 在下边");
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(Render2dPlugin).init_resource::<PerfGraph>().add_systems(PostUpdate, perf_graph);
        app
    }

    fn legend_text(app: &mut App) -> Vec<String> {
        app.world_mut().query_filtered::<&Text2d, With<PerfGraphText>>().iter(app.world()).map(|t| t.text.clone()).collect()
    }

    #[test]
    fn the_graph_draws_a_segment_per_frame_pair_and_skips_missing_gpu_time() {
        let mut app = app();
        for i in 0..4 {
            let gpu_ms = (i >= 2).then_some(3.0);
            app.world_mut().resource_mut::<PerfHistory>().push(PerfSample { frame_ms: 16.0, update_ms: 2.0, render_ms: 3.0, wait_ms: 11.0, gpu_ms, entities: 30, draws: 4 });
        }
        app.tick(Default::default(), 0.016);
        assert!(app.world().resource::<SpriteInstances>().is_empty(), "默认关着");
        assert!(legend_text(&mut app).is_empty());
        app.world_mut().resource_mut::<PerfGraph>().0 = true;
        app.tick(Default::default(), 0.016);
        // 两块底 + 四条耗时 × 3 段 + GPU 1 段 + 两条数量 × 3 段
        let n = app.world().resource::<SpriteInstances>().len();
        assert_eq!(n, 2 + 4 * 3 + 1 + 2 * 3);
        assert_eq!(legend_text(&mut app), [legend(16.7, 50)]);
        app.world_mut().resource_mut::<PerfGraph>().0 = false;
        app.tick(Default::default(), 0.016);
        assert!(app.world().resource::<SpriteInstances>().is_empty() && legend_text(&mut app).is_empty());
    }

    #[test]
    fn segments_follow_a_zoomed_camera() {
        let view = Screen2World { camera: Camera2d { position: Vec2::new(100.0, 0.0), zoom: 2.0, rotation: 0.0 }, size: Vec2::new(200.0, 100.0) };
        let s = view.segment(Vec2::new(100.0, 50.0), Vec2::new(120.0, 50.0), Vec4::ONE);
        assert_eq!(s.position, Vec2::new(105.0, 0.0));
        assert_eq!(s.size, Vec2::new(10.0 + LINE / 2.0, LINE / 2.0));
        assert_eq!(s.rotation, 0.0);
    }
}
