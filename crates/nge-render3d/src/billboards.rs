//! 朝着相机的方片和带子（3D 粒子、拖尾），见 docs/particles3d.md。每帧在 CPU 上展开成三角形。

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use glam::{Quat, Vec3, Vec4};
use nge_assets::ImageId;

/// 混合方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Blend {
    /// 普通透明：从远往近画。
    #[default]
    Alpha,
    /// 叠加（火、光点）：不用排序。
    Add,
}

/// 一个朝着相机的方片。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Billboard {
    pub position: Vec3,
    /// 边长（世界单位）。
    pub size: f32,
    /// 绕视线转的角度（弧度）。
    pub rotation: f32,
    /// 线性 RGBA，乘上贴图。
    pub color: Vec4,
}

/// 带子上的一个点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RibbonPoint {
    pub position: Vec3,
    pub width: f32,
    pub color: Vec4,
}

/// 这一帧要画的方片和带子，每帧 `PreUpdate` 清空。
#[derive(Resource, Debug, Default)]
pub struct Billboards {
    quads: Vec<(ImageId, Blend, Billboard)>,
    ribbons: Vec<(ImageId, Blend, Vec<RibbonPoint>)>,
}

impl Billboards {
    pub fn push(&mut self, texture: ImageId, blend: Blend, quad: Billboard) {
        self.quads.push((texture, blend, quad));
    }

    pub fn extend(&mut self, texture: ImageId, blend: Blend, quads: impl IntoIterator<Item = Billboard>) {
        self.quads.extend(quads.into_iter().map(|q| (texture, blend, q)));
    }

    /// 一条带子；不到两个点的不画。
    pub fn ribbon(&mut self, texture: ImageId, blend: Blend, points: impl IntoIterator<Item = RibbonPoint>) {
        let points: Vec<RibbonPoint> = points.into_iter().collect();
        if points.len() >= 2 {
            self.ribbons.push((texture, blend, points));
        }
    }

    /// 方片的个数。
    pub fn quads(&self) -> usize {
        self.quads.len()
    }

    pub fn is_empty(&self) -> bool {
        self.quads.is_empty() && self.ribbons.is_empty()
    }

    /// 带子的条数。
    pub fn ribbons(&self) -> usize {
        self.ribbons.len()
    }

    pub fn clear(&mut self) {
        self.quads.clear();
        self.ribbons.clear();
    }
}

pub(crate) fn clear_billboards(mut billboards: ResMut<Billboards>) {
    billboards.clear();
}

/// 和 `billboards.wgsl` 的顶点输入一一对应。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct BillboardVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// 一次 draw：连续的一段顶点，同一张贴图、同一种混合。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Batch {
    pub texture: ImageId,
    pub blend: Blend,
    pub vertices: std::ops::Range<u32>,
}

/// 相机：位置、朝向（看向 -Z），透视时 `eye` 的 w 是 1、正交时是 0（和 `Camera3d::eye` 一样）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct View {
    pub rotation: Quat,
    pub eye: Vec4,
}

impl View {
    /// 沿相机朝向的远近（大的远），只拿来排序：差一个常数不影响，透视和正交都一样算。
    fn depth(&self, p: Vec3) -> f32 {
        p.dot(self.rotation * Vec3::NEG_Z)
    }

    /// 从 `p` 指向相机的方向。
    fn to_eye(&self, p: Vec3) -> Vec3 {
        match self.eye.w {
            0.0 => self.eye.truncate(),
            _ => self.eye.truncate() - p,
        }
    }
}

enum Item<'a> {
    Quad(&'a Billboard),
    Ribbon(&'a [RibbonPoint]),
}

/// 展开成三角形：透明的从远往近，叠加的按贴图分组画在后面；相邻同贴图同混合的合成一次 draw。
pub(crate) fn build(billboards: &Billboards, view: View, vertices: &mut Vec<BillboardVertex>, batches: &mut Vec<Batch>) {
    vertices.clear();
    batches.clear();
    let mut items: Vec<(ImageId, Blend, f32, Item)> = billboards.quads.iter().map(|(t, b, q)| (*t, *b, view.depth(q.position), Item::Quad(q))).collect();
    for (t, b, points) in &billboards.ribbons {
        let center = points.iter().map(|p| p.position).sum::<Vec3>() / points.len() as f32;
        items.push((*t, *b, view.depth(center), Item::Ribbon(points)));
    }
    // 透明在前、从远往近；叠加在后、按贴图（稳定排序，同一张贴图里保持原来的顺序）
    items.sort_by(|a, b| match (a.1, b.1) {
        (Blend::Alpha, Blend::Alpha) => b.2.total_cmp(&a.2),
        (Blend::Alpha, Blend::Add) => std::cmp::Ordering::Less,
        (Blend::Add, Blend::Alpha) => std::cmp::Ordering::Greater,
        (Blend::Add, Blend::Add) => a.0.cmp(&b.0),
    });
    let (right, up) = (view.rotation * Vec3::X, view.rotation * Vec3::Y);
    for (texture, blend, _, item) in items {
        let start = vertices.len() as u32;
        match item {
            Item::Quad(q) => quad(q, right, up, vertices),
            Item::Ribbon(points) => ribbon(points, view, vertices),
        }
        let end = vertices.len() as u32;
        match batches.last_mut() {
            Some(last) if last.texture == texture && last.blend == blend && last.vertices.end == start => last.vertices.end = end,
            _ if end > start => batches.push(Batch { texture, blend, vertices: start..end }),
            _ => {}
        }
    }
}

fn vertex(position: Vec3, uv: [f32; 2], color: Vec4) -> BillboardVertex {
    BillboardVertex { position: position.into(), uv, color: color.into() }
}

fn quad(q: &Billboard, right: Vec3, up: Vec3, out: &mut Vec<BillboardVertex>) {
    let (sin, cos) = q.rotation.sin_cos();
    let half = q.size / 2.0;
    // 先在相机平面里转，再展开
    let (r, u) = ((right * cos + up * sin) * half, (up * cos - right * sin) * half);
    let corner = |x: f32, y: f32, uv| vertex(q.position + r * x + u * y, uv, q.color);
    let (a, b, c, d) = (corner(-1.0, 1.0, [0.0, 0.0]), corner(1.0, 1.0, [1.0, 0.0]), corner(1.0, -1.0, [1.0, 1.0]), corner(-1.0, -1.0, [0.0, 1.0]));
    out.extend([a, d, c, a, c, b]);
}

fn ribbon(points: &[RibbonPoint], view: View, out: &mut Vec<BillboardVertex>) {
    let last = points.len() - 1;
    // 切线取前后两边第一个不重合的点，重合的点也有方向
    let distinct = |i: usize, mut j: usize, step: fn(usize) -> Option<usize>| {
        while points[j].position == points[i].position {
            match step(j).filter(|&k| k <= last) {
                Some(k) => j = k,
                None => break,
            }
        }
        points[j].position
    };
    let side = |i: usize| {
        let p = &points[i];
        let tangent = distinct(i, i, |j| j.checked_add(1)) - distinct(i, i, |j| j.checked_sub(1));
        view.to_eye(p.position).cross(tangent).normalize_or_zero() * (p.width.max(0.0) / 2.0)
    };
    // u 按走过的长度算，点疏密不匀时贴图不挤
    let mut lengths = Vec::with_capacity(points.len());
    let mut total = 0.0;
    for (i, p) in points.iter().enumerate() {
        total += if i == 0 { 0.0 } else { p.position.distance(points[i - 1].position) };
        lengths.push(total);
    }
    let mut previous = None;
    for i in 0..=last {
        let p = &points[i];
        let s = side(i);
        let u = if total > 0.0 { lengths[i] / total } else { i as f32 / last as f32 };
        let edge = (vertex(p.position + s, [u, 0.0], p.color), vertex(p.position - s, [u, 1.0], p.color));
        if let Some((a, b)) = previous {
            let (c, d) = edge;
            out.extend([a, b, d, a, d, c]);
        }
        previous = Some(edge);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认相机：在 (0, 0, 10) 看向 -Z。
    fn view() -> View {
        View { rotation: Quat::IDENTITY, eye: Vec4::new(0.0, 0.0, 10.0, 1.0) }
    }

    fn quad_at(z: f32) -> Billboard {
        Billboard { position: Vec3::new(0.0, 0.0, z), size: 2.0, rotation: 0.0, color: Vec4::ONE }
    }

    fn built(b: &Billboards, view: View) -> (Vec<BillboardVertex>, Vec<Batch>) {
        let (mut v, mut batches) = (Vec::new(), Vec::new());
        build(b, view, &mut v, &mut batches);
        (v, batches)
    }

    fn positions(v: &[BillboardVertex]) -> Vec<Vec3> {
        v.iter().map(|v| Vec3::from(v.position)).collect()
    }

    #[test]
    fn quads_face_the_camera_and_turn_around_the_view_axis() {
        let mut b = Billboards::default();
        b.push(ImageId::WHITE, Blend::Alpha, Billboard { position: Vec3::new(1.0, 2.0, 3.0), ..quad_at(0.0) });
        let (v, batches) = built(&b, view());
        assert_eq!(v.len(), 6);
        assert_eq!(batches, [Batch { texture: ImageId::WHITE, blend: Blend::Alpha, vertices: 0..6 }]);
        let p = positions(&v);
        assert!(p.iter().all(|p| p.z == 3.0), "在相机平面里：{p:?}");
        assert_eq!((p[0], p[2]), (Vec3::new(0.0, 3.0, 3.0), Vec3::new(2.0, 1.0, 3.0)), "左上、右下");
        assert_eq!((v[0].uv, v[2].uv), ([0.0, 0.0], [1.0, 1.0]));
        // 相机绕 y 转 90°（看向 -X）：方片在 yz 平面里
        let side = View { rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), eye: Vec4::new(10.0, 0.0, 0.0, 1.0) };
        assert!(positions(&built(&b, side).0).iter().all(|p| (p.x - 1.0).abs() < 1e-5));
        // 转 90°：左上角跑到左下
        b.clear();
        b.push(ImageId::WHITE, Blend::Alpha, Billboard { rotation: std::f32::consts::FRAC_PI_2, ..quad_at(0.0) });
        let p = positions(&built(&b, view()).0);
        assert!((p[0] - Vec3::new(-1.0, -1.0, 0.0)).length() < 1e-5, "{:?}", p[0]);
    }

    #[test]
    fn alpha_goes_far_to_near_and_additive_after_it_grouped_by_texture() {
        let (a, c) = (ImageId::WHITE, crate::ImageId::new(3));
        let mut b = Billboards::default();
        b.push(a, Blend::Add, quad_at(0.0));
        b.push(a, Blend::Alpha, quad_at(5.0));
        b.push(c, Blend::Add, quad_at(1.0));
        b.push(a, Blend::Alpha, quad_at(-5.0));
        b.push(a, Blend::Add, quad_at(2.0));
        b.push(c, Blend::Alpha, quad_at(0.0));
        let (v, batches) = built(&b, view());
        let order: Vec<(ImageId, Blend, f32)> = batches.iter().map(|b| (b.texture, b.blend, v[b.vertices.start as usize].position[2])).collect();
        assert_eq!(order, [(a, Blend::Alpha, -5.0), (c, Blend::Alpha, 0.0), (a, Blend::Alpha, 5.0), (a, Blend::Add, 0.0), (c, Blend::Add, 1.0)]);
        assert_eq!(batches[3].vertices.len(), 12, "两个叠加的同贴图方片合成一次");
        // 相机反过来（在 -10 看向 +Z）时透明的顺序也反过来
        let back = View { rotation: Quat::from_rotation_y(std::f32::consts::PI), eye: Vec4::new(0.0, 0.0, -10.0, 1.0) };
        let (v, batches) = built(&b, back);
        assert_eq!(v[batches[0].vertices.start as usize].position[2], 5.0);
        // 正交：只看朝向
        let ortho = View { rotation: Quat::IDENTITY, eye: Vec4::new(0.0, 0.0, 1.0, 0.0) };
        let (v, batches) = built(&b, ortho);
        assert_eq!(v[batches[0].vertices.start as usize].position[2], -5.0);
    }

    #[test]
    fn ribbons_spread_sideways_to_the_camera() {
        let mut b = Billboards::default();
        let point = |x: f32, width: f32| RibbonPoint { position: Vec3::new(x, 0.0, 0.0), width, color: Vec4::ONE };
        b.ribbon(ImageId::WHITE, Blend::Alpha, [point(0.0, 2.0), point(1.0, 2.0), point(3.0, 0.0)]);
        b.ribbon(ImageId::WHITE, Blend::Alpha, [point(0.0, 2.0)]);
        assert_eq!(b.ribbons(), 1, "一个点的不算");
        let (v, batches) = built(&b, view());
        assert_eq!((v.len(), batches.len()), (12, 1), "三个点两段，每段两个三角形");
        let p = positions(&v);
        // 沿 x 走、相机在 +z：两边是 ±y
        assert_eq!((p[0], p[1]), (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, -1.0, 0.0)));
        assert_eq!(p[2], Vec3::new(1.0, -1.0, 0.0), "中间的点按前后两个点的切线");
        assert!(p[6..].iter().any(|q| *q == Vec3::new(3.0, 0.0, 0.0)), "宽 0 的尾巴收成一个点");
        // u 按走过的长度：中间的点走了 1 / 3
        assert_eq!((v[0].uv, v[2].uv, v[7].uv), ([0.0, 0.0], [1.0 / 3.0, 1.0], [1.0 / 3.0, 1.0]));
        // 拐弯：中间的点按前后两个点的切线（斜 45°）展开（正交相机，指向相机的方向处处一样）
        b.clear();
        let at = |x: f32, y: f32| RibbonPoint { position: Vec3::new(x, y, 0.0), width: 2.0, color: Vec4::ONE };
        b.ribbon(ImageId::WHITE, Blend::Alpha, [at(0.0, 0.0), at(1.0, 0.0), at(1.0, 1.0)]);
        let ortho = View { rotation: Quat::IDENTITY, eye: Vec4::new(0.0, 0.0, 1.0, 0.0) };
        let p = positions(&built(&b, ortho).0);
        let side = p[2] - Vec3::new(1.0, 0.0, 0.0);
        assert!((side - Vec3::new(1.0, -1.0, 0.0).normalize()).length() < 1e-5, "{side}");
        // 透视：离视线远的点，往相机那边偏
        b.clear();
        b.ribbon(ImageId::WHITE, Blend::Alpha, [at(0.0, 5.0), at(1.0, 5.0)]);
        let p = positions(&built(&b, view()).0);
        let side = p[0] - Vec3::new(0.0, 5.0, 0.0);
        assert!((side - Vec3::new(0.0, 10.0, 5.0).normalize()).length() < 1e-5, "侧向和指向相机的方向垂直：{side}");
        // 带子正对着相机走（切线和视线平行）：不展开，不出 NaN
        b.clear();
        b.ribbon(ImageId::WHITE, Blend::Alpha, [RibbonPoint { position: Vec3::ZERO, ..point(0.0, 2.0) }, RibbonPoint { position: Vec3::Z, ..point(0.0, 2.0) }]);
        assert!(positions(&built(&b, view()).0).iter().all(|p| p.is_finite() && p.x == 0.0 && p.y == 0.0));
    }

    #[test]
    fn coincident_points_take_the_direction_of_their_neighbours() {
        let mut b = Billboards::default();
        let at = |x: f32, width: f32| RibbonPoint { position: Vec3::new(x, 0.0, 0.0), width, color: Vec4::ONE };
        // 最新的两个点重合（实体刚停下）、宽是负的：都还按 x 方向展开，负宽当 0
        b.ribbon(ImageId::WHITE, Blend::Alpha, [at(2.0, 2.0), at(2.0, 2.0), at(1.0, -4.0)]);
        let p = positions(&built(&b, view()).0);
        assert_eq!((p[0], p[1]), (Vec3::new(2.0, -1.0, 0.0), Vec3::new(2.0, 1.0, 0.0)));
        assert_eq!(p[3], Vec3::new(2.0, -1.0, 0.0), "重合的第二个点往后找");
        assert!(p.iter().any(|q| *q == Vec3::new(1.0, 0.0, 0.0)), "负宽收成一个点：{p:?}");
        // 全都重合：不展开，不出 NaN
        b.clear();
        b.ribbon(ImageId::WHITE, Blend::Alpha, [at(2.0, 2.0), at(2.0, 2.0)]);
        assert!(positions(&built(&b, view()).0).iter().all(|q| *q == Vec3::new(2.0, 0.0, 0.0)));
    }

    #[test]
    fn ribbons_sort_by_their_middle_with_the_quads() {
        let mut b = Billboards::default();
        let point = |z: f32| RibbonPoint { position: Vec3::new(0.0, 0.0, z), width: 1.0, color: Vec4::ONE };
        b.push(ImageId::WHITE, Blend::Alpha, quad_at(0.0));
        b.ribbon(crate::ImageId::new(2), Blend::Alpha, [point(1.0), point(-9.0)]);
        let (_, batches) = built(&b, view());
        assert_eq!(batches.iter().map(|b| b.texture).collect::<Vec<_>>(), [crate::ImageId::new(2), ImageId::WHITE], "带子中点在 -4，比方片远（虽然第一个点更近）");
        b.clear();
        assert!(b.is_empty());
    }
}
