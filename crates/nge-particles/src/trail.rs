//! 拖尾（见 docs/particles3d.md）：跟着实体走，留下一条朝着相机的带子，越老越细、越淡。

use std::collections::VecDeque;

use bevy_ecs::prelude::*;
use glam::{Vec3, Vec4};
use nge_assets::ImageId;
use nge_ecs::Time;
use nge_render3d::{Billboards, Blend, RibbonPoint, Transform3d};

/// 拖尾参数。`width` / `color` 是 `(新, 旧)`，按点的年龄插值。
#[derive(Component, Debug, Clone, PartialEq)]
#[require(TrailPoints)]
pub struct Trail3d {
    /// 一个点留多久（秒）。
    pub lifetime: f32,
    pub width: (f32, f32),
    /// 线性 RGBA。
    pub color: (Vec4, Vec4),
    /// 相邻两个点至少隔多远；不到时最新的点只是跟着实体挪。
    pub min_distance: f32,
    /// 最多记多少个点，多了丢最老的。
    pub max_points: usize,
    pub texture: ImageId,
    pub blend: Blend,
}

impl Default for Trail3d {
    fn default() -> Self {
        Self {
            lifetime: 0.5,
            width: (0.2, 0.0),
            color: (Vec4::ONE, Vec4::new(1.0, 1.0, 1.0, 0.0)),
            min_distance: 0.0,
            max_points: 64,
            texture: ImageId::WHITE,
            blend: Blend::Alpha,
        }
    }
}

/// 拖尾记下的点（世界坐标、年龄），最新的在前。随 [`Trail3d`] 自动加上。
#[derive(Component, Debug, Clone, Default)]
pub struct TrailPoints {
    points: VecDeque<(Vec3, f32)>,
}

impl TrailPoints {
    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// 从新到旧的位置。
    pub fn positions(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.points.iter().map(|(p, _)| *p)
    }

    /// 推进 `dt` 秒，实体现在在 `at`。
    fn step(&mut self, trail: &Trail3d, at: Vec3, dt: f32) {
        for (_, age) in &mut self.points {
            *age += dt;
        }
        // 最新的点不算老：它代表实体现在的位置
        let lifetime = trail.lifetime.max(0.0);
        while self.points.len() > 1 && self.points.back().is_some_and(|(_, age)| *age >= lifetime) {
            self.points.pop_back();
        }
        // 实体没动：最新的点还在实体上，只把它的年龄归零，不叠一个重合的点（重合的点算不出带子的方向）。
        // 最新的点离上一个点还不够远时，它跟着挪到实体上；够远了就留下，再开一个新点
        let near = match (self.points.front(), self.points.get(1)) {
            (Some((front, _)), _) if front.distance(at) <= f32::EPSILON => true,
            (Some((front, _)), Some((previous, _))) => front.distance(*previous) <= trail.min_distance,
            _ => false,
        };
        match self.points.front_mut() {
            Some(front) if near => *front = (at, 0.0),
            _ => self.points.push_front((at, 0.0)),
        }
        self.points.truncate(trail.max_points.max(2));
    }

    /// 清掉记下的点，比如实体瞬移之后。
    pub fn clear(&mut self) {
        self.points.clear();
    }

    fn ribbon<'a>(&'a self, trail: &'a Trail3d) -> impl Iterator<Item = RibbonPoint> + 'a {
        let lifetime = trail.lifetime.max(f32::EPSILON);
        // 点数到了上限时尾巴也收到 `width.1` / `color.1`，不会在截断处突然断掉
        let cap = trail.max_points.max(2) as f32 - 1.0;
        self.points.iter().enumerate().map(move |(i, (position, age))| {
            let t = (age / lifetime).max(i as f32 / cap).clamp(0.0, 1.0);
            RibbonPoint { position: *position, width: trail.width.0 + (trail.width.1 - trail.width.0) * t, color: trail.color.0.lerp(trail.color.1, t) }
        })
    }
}

pub(crate) fn trails(time: Res<Time>, mut billboards: ResMut<Billboards>, mut trails: Query<(&Trail3d, &mut TrailPoints, &Transform3d)>) {
    for (trail, mut points, transform) in &mut trails {
        points.step(trail, transform.translation, time.delta);
        billboards.ribbon(trail.texture, trail.blend, points.ribbon(trail));
    }
}

#[cfg(test)]
mod tests {
    use nge_core::Viewport;
    use nge_ecs::App;
    use nge_render3d::Render3dPlugin;

    use super::*;
    use crate::Particles3dPlugin;

    const VIEWPORT: Viewport = Viewport { size: [100.0, 100.0], pixels: [100, 100] };

    #[test]
    fn records_where_the_entity_went_and_forgets_old_points() {
        let trail = Trail3d { lifetime: 0.25, ..Default::default() };
        let mut points = TrailPoints::default();
        for x in 0..4 {
            points.step(&trail, Vec3::new(x as f32, 0.0, 0.0), 0.1);
        }
        // 年龄：3 → 0，2 → 0.1，1 → 0.2，0 → 0.3（过了 0.25 丢掉）
        assert_eq!(points.positions().collect::<Vec<_>>(), [Vec3::new(3.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)]);
        // 不动了：旧的点慢慢老掉，尾巴缩回去，只剩实体所在的一个点
        let mut still = points.clone();
        for _ in 0..5 {
            still.step(&trail, Vec3::new(3.0, 0.0, 0.0), 0.1);
        }
        assert_eq!(still.len(), 1);
        // 寿命比一帧还短：上一帧的位置留着，最新的点不算老，还能画出一小段
        let short = Trail3d { lifetime: 0.01, ..Default::default() };
        let mut brief = TrailPoints::default();
        brief.step(&short, Vec3::ZERO, 0.1);
        brief.step(&short, Vec3::X, 0.1);
        assert_eq!(brief.positions().collect::<Vec<_>>(), [Vec3::X, Vec3::ZERO]);
        let p: Vec<RibbonPoint> = points.ribbon(&Trail3d { width: (1.0, 0.0), color: (Vec4::ONE, Vec4::ZERO), ..trail.clone() }).collect();
        assert_eq!(p[0].width, 1.0, "最新的最宽");
        assert!((p[2].width - 0.2).abs() < 1e-5 && (p[2].color.x - 0.2).abs() < 1e-5, "0.2 秒 / 0.25 秒：{:?}", p[2]);
    }

    #[test]
    fn min_distance_moves_the_head_instead_of_adding_points() {
        let trail = Trail3d { min_distance: 1.0, lifetime: 10.0, ..Default::default() };
        let mut points = TrailPoints::default();
        for i in 0..10 {
            points.step(&trail, Vec3::new(i as f32 * 0.3, 0.0, 0.0), 0.016);
        }
        let p: Vec<Vec3> = points.positions().collect();
        assert_eq!(p[0], Vec3::new(2.7, 0.0, 0.0), "最新的点总在实体上");
        assert!(p.windows(2).skip(1).all(|w| w[0].distance(w[1]) >= 1.0 - 1e-5), "中间的点隔开至少 1：{p:?}");
        assert!(p.len() >= 3 && p.len() < 10, "{p:?}");
        let capped = Trail3d { max_points: 3, lifetime: 10.0, ..Default::default() };
        let mut points = TrailPoints::default();
        for i in 0..10 {
            points.step(&capped, Vec3::splat(i as f32), 0.016);
        }
        assert_eq!(points.len(), 3);
    }

    #[test]
    fn stopping_keeps_the_newest_end_wide() {
        let trail = Trail3d { width: (1.0, 0.0), lifetime: 1.0, ..Default::default() };
        let mut points = TrailPoints::default();
        for x in 0..3 {
            points.step(&trail, Vec3::new(x as f32, 0.0, 0.0), 0.1);
        }
        for _ in 0..3 {
            points.step(&trail, Vec3::new(2.0, 0.0, 0.0), 0.1);
        }
        assert_eq!(points.positions().collect::<Vec<_>>(), [Vec3::new(2.0, 0.0, 0.0), Vec3::X, Vec3::ZERO], "停下不叠重合的点");
        assert_eq!(points.ribbon(&trail).next().unwrap().width, 1.0, "最新一头还是最宽");
        points.clear();
        assert!(points.is_empty());
    }

    #[test]
    fn the_tail_narrows_by_the_point_cap_too() {
        let trail = Trail3d { width: (1.0, 0.0), lifetime: 100.0, max_points: 3, ..Default::default() };
        let mut points = TrailPoints::default();
        for x in 0..5 {
            points.step(&trail, Vec3::new(x as f32, 0.0, 0.0), 0.01);
        }
        let widths: Vec<f32> = points.ribbon(&trail).map(|p| p.width).collect();
        assert_eq!(widths, [1.0, 0.5, 0.0]);
    }

    #[test]
    fn trails_become_ribbons_every_frame() {
        let mut app = App::new();
        app.add_plugins(Render3dPlugin).add_plugins(Particles3dPlugin);
        let entity = app.world_mut().spawn((Trail3d::default(), Transform3d::default())).id();
        app.tick(VIEWPORT, 0.016);
        assert_eq!(app.world().resource::<Billboards>().ribbons(), 0, "一个点画不出带子");
        app.world_mut().get_mut::<Transform3d>(entity).unwrap().translation = Vec3::X;
        app.tick(VIEWPORT, 0.016);
        assert_eq!(app.world().resource::<Billboards>().ribbons(), 1);
        assert_eq!(app.world().get::<TrailPoints>(entity).unwrap().len(), 2);
    }
}
