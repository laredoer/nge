//! 粒子碰撞（见 docs/particles3d-more.md）：地面平面和场景碰撞体（rapier 射线），反弹、死亡。

use glam::Vec3;

/// 碰撞参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleCollision {
    /// 地面平面的高度（世界 y）。
    pub ground: Option<f32>,
    /// 和场景的 3D 碰撞体碰（每帧的查询数有上限，见 `ParticleBudget3d::max_queries`）。
    pub scene: bool,
    /// 法向速度反弹系数 0–1。
    pub bounce: f32,
    /// 切向速度损失比例 0–1。
    pub friction: f32,
    /// 碰到就死。
    pub kill: bool,
    /// 粒子半径：停在表面外这么远。
    pub radius: f32,
}

impl Default for ParticleCollision {
    fn default() -> Self {
        Self { ground: None, scene: false, bounce: 0.5, friction: 0.0, kill: false, radius: 0.0 }
    }
}

/// 碰到的地方：已经按半径挪到表面外。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Hit {
    pub point: Vec3,
    pub normal: Vec3,
}

fn unit(x: f32) -> f32 {
    if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) }
}

/// 碰到法线是 `n` 的面后的速度：法向分量反过来乘 `bounce`，切向分量乘 `1 - friction`；已经在离开的不变。
pub(crate) fn bounce(v: Vec3, n: Vec3, bounce: f32, friction: f32) -> Vec3 {
    let into = v.dot(n);
    if into >= 0.0 {
        return v;
    }
    let normal = n * into;
    (v - normal) * (1.0 - unit(friction)) - normal * unit(bounce)
}

/// 从 `from` 走到 `to` 时有没有从上面穿过高 `height` 的地面（粒子半径 `radius`）。
pub(crate) fn ground_hit(from: Vec3, to: Vec3, height: f32, radius: f32) -> Option<Hit> {
    let level = height + radius.max(0.0);
    if !(from.y >= level && to.y <= level && from.y > to.y) {
        return None;
    }
    let k = (from.y - level) / (from.y - to.y);
    Some(Hit { point: from.lerp(to, k), normal: Vec3::Y })
}

/// 场景的射线查询（`nge_physics3d::Physics3d`；测试里换成别的）。
pub(crate) trait Raycast {
    /// 从 `origin` 沿单位向量 `dir` 最远 `max`：碰到的点、面的法线。
    fn raycast(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<(Vec3, Vec3)>;
}

impl Raycast for nge_physics3d::Physics3d {
    fn raycast(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<(Vec3, Vec3)> {
        nge_physics3d::Physics3d::raycast(self, origin, dir, max, Default::default()).map(|h| (h.point, h.normal))
    }
}

/// 场景里从 `from` 走到 `to` 碰到的地方（半径 `radius`）。
pub(crate) fn scene_hit(world: &dyn Raycast, from: Vec3, to: Vec3, radius: f32) -> Option<Hit> {
    let delta = to - from;
    let dir = delta.try_normalize()?;
    let radius = radius.max(0.0);
    let (point, normal) = world.raycast(from, dir, delta.length() + radius)?;
    // 停在表面外半径那么远，至少比碰到的点往回退一点，免得下一帧从里面出发
    Some(Hit { point: point + normal * radius.max(1e-4 * delta.length().max(1.0)), normal })
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;

    #[test]
    fn bounce_flips_the_normal_part_and_rubs_the_rest() {
        let v = bounce(Vec3::new(2.0, -4.0, 0.0), Vec3::Y, 0.5, 0.25);
        assert!((v - Vec3::new(1.5, 2.0, 0.0)).length() < 1e-5, "{v}");
        assert_eq!(bounce(Vec3::new(2.0, -4.0, 0.0), Vec3::Y, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0), "不弹就贴着滑");
        assert_eq!(bounce(Vec3::new(2.0, 4.0, 0.0), Vec3::Y, 0.5, 0.0), Vec3::new(2.0, 4.0, 0.0), "已经离开表面的不动");
        let wall = bounce(Vec3::new(-3.0, 1.0, 0.0), Vec3::X, 1.0, 0.0);
        assert!((wall - Vec3::new(3.0, 1.0, 0.0)).length() < 1e-5);
        assert!(bounce(Vec3::ONE, Vec3::NEG_Y, f32::NAN, f32::NAN).is_finite(), "NaN 的系数当 0");
        assert_eq!(bounce(Vec3::new(1.0, -1.0, 0.0), Vec3::Y, 5.0, 3.0), Vec3::new(0.0, 1.0, 0.0), "反弹系数、摩擦都截到 [0, 1]");
    }

    #[test]
    fn the_ground_is_hit_only_when_crossing_it_from_above() {
        let hit = ground_hit(Vec3::new(0.0, 2.0, 0.0), Vec3::new(4.0, -2.0, 0.0), 0.0, 0.5).unwrap();
        assert!((hit.point - Vec3::new(1.5, 0.5, 0.0)).length() < 1e-5, "停在地面上半径那么高 {:?}", hit.point);
        assert_eq!(hit.normal, Vec3::Y);
        assert!(ground_hit(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 1.0, 0.0), 0.0, 0.5).is_none(), "还没到");
        assert!(ground_hit(Vec3::new(0.0, -2.0, 0.0), Vec3::new(0.0, -3.0, 0.0), 0.0, 0.0).is_none(), "从下面过来的不算");
        assert!(ground_hit(Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 1.0, 0.0), 0.0, 0.0).is_none(), "往上穿的不算");
        assert!(ground_hit(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, -1.0, 0.0), 0.0, 0.0).is_some(), "正好在地面上往下走算");
    }
}
