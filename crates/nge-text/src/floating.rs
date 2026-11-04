//! 飘字：往上飘、渐隐，到时间自动删除（得分、伤害数字）。

use bevy_ecs::prelude::*;
use nge_ecs::Time;
use nge_render2d::Transform2d;

use crate::Text2d;

/// 和 [`Text2d`]、`Transform2d` 放在同一个实体上。透明度从文字原本的 alpha 线性降到 0。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct FloatingText {
    /// 秒。
    pub duration: f32,
    /// 每秒往上飘多少（`Transform2d` 的单位，y 朝上）。
    pub rise: f32,
    age: f32,
    alpha: Option<f32>,
}

impl FloatingText {
    pub fn new(duration: f32, rise: f32) -> Self {
        Self { duration, rise, age: 0.0, alpha: None }
    }
}

impl Default for FloatingText {
    fn default() -> Self {
        Self::new(1.0, 60.0)
    }
}

pub(crate) fn float(mut commands: Commands, time: Res<Time>, mut q: Query<(Entity, &mut FloatingText, &mut Transform2d, &mut Text2d)>) {
    for (entity, mut floating, mut t, mut text) in &mut q {
        let alpha = *floating.alpha.get_or_insert(text.color.w);
        floating.age += time.delta;
        if floating.age >= floating.duration {
            commands.entity(entity).despawn();
            continue;
        }
        t.translation.y += floating.rise * time.delta;
        text.color.w = alpha * (1.0 - floating.age / floating.duration);
    }
}

#[cfg(test)]
mod tests {
    use glam::{Vec2, Vec4};
    use nge_core::Viewport;
    use nge_ecs::App;

    use super::*;
    use crate::TextPlugin;

    const VIEWPORT: Viewport = Viewport { size: [100.0, 100.0], pixels: [100, 100] };

    #[test]
    fn rises_fades_and_despawns() {
        let mut app = App::new();
        app.add_plugins(TextPlugin);
        let entity = app
            .world_mut()
            .spawn((FloatingText::new(1.0, 50.0), Text2d::new("+10").with_color(Vec4::new(1.0, 1.0, 1.0, 0.8)), Transform2d::default()))
            .id();
        app.tick(VIEWPORT, 0.5);
        let world = app.world();
        assert_eq!(world.get::<Transform2d>(entity).unwrap().translation, Vec2::new(0.0, 25.0));
        assert!((world.get::<Text2d>(entity).unwrap().color.w - 0.4).abs() < 1e-6, "从原本的 alpha 降一半");
        app.tick(VIEWPORT, 0.5);
        assert!(app.world().get_entity(entity).is_err());
    }
}
