//! 方片和带子（3D 粒子、拖尾），见 docs/particles3d.md、docs/particles3d-more.md。每帧在 CPU 上展开成三角形。

use std::ops::Range;

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

/// 方片的朝向。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Facing {
    /// 朝着相机。
    #[default]
    Camera,
    /// 长边沿速度：宽 `size`，长 `size + |velocity| × stretch`；速度是 0 或正对相机时退成 `Camera`。不看 `rotation`。
    Velocity { velocity: Vec3, stretch: f32 },
    /// 躺在 xz 平面里，`rotation` 绕 y 转。
    Horizontal,
    /// 本地 xy 平面的方片乘这个旋转，`rotation` 绕本地 z 转。
    Fixed(Quat),
}

/// 整张贴图的 uv 范围。
pub const FULL_UV: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// 一个方片。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Billboard {
    pub position: Vec3,
    /// 边长（世界单位）。
    pub size: f32,
    /// 在方片平面里转的角度（弧度）。
    pub rotation: f32,
    /// 线性 RGBA，乘上贴图。
    pub color: Vec4,
    pub facing: Facing,
    /// 贴图上的范围 `[u0, v0, u1, v1]`（序列帧的这一帧）。
    pub uv: [f32; 4],
    /// 下一帧的范围，按 `frame_mix` 和这一帧混合。
    pub next_uv: [f32; 4],
    pub frame_mix: f32,
    /// 软化距离（世界单位）：离后面的不透明表面不到这么远时淡出；0 不软化。
    pub soft: f32,
    /// 受环境光和方向光影响。
    pub lit: bool,
}

impl Default for Billboard {
    fn default() -> Self {
        Self { position: Vec3::ZERO, size: 1.0, rotation: 0.0, color: Vec4::ONE, facing: Facing::Camera, uv: FULL_UV, next_uv: FULL_UV, frame_mix: 0.0, soft: 0.0, lit: false }
    }
}

/// 带子上的一个点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RibbonPoint {
    pub position: Vec3,
    pub width: f32,
    pub color: Vec4,
}

/// 这一帧要画的方片和带子，每帧 `PreUpdate` 清空（留着容量）。
#[derive(Resource, Debug, Default)]
pub struct Billboards {
    quads: Vec<(ImageId, Blend, Billboard)>,
    /// 每条带子是 `points` 里的一段。
    ribbons: Vec<(ImageId, Blend, Range<usize>)>,
    points: Vec<RibbonPoint>,
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
        let start = self.points.len();
        self.points.extend(points);
        if self.points.len() - start >= 2 {
            self.ribbons.push((texture, blend, start..self.points.len()));
        } else {
            self.points.truncate(start);
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
        self.points.clear();
    }
}

pub(crate) fn clear_billboards(mut billboards: ResMut<Billboards>) {
    billboards.clear();
}

/// 和 `billboards.wgsl` 的顶点输入一一对应。`params`：帧混合、软化距离、受光（1 / 0）、空。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct BillboardVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub uv2: [f32; 2],
    pub color: [f32; 4],
    pub normal: [f32; 3],
    pub params: [f32; 4],
}

/// 一次 draw：连续的一段顶点，同一张贴图、同一种混合。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Batch {
    pub texture: ImageId,
    pub blend: Blend,
    pub vertices: Range<u32>,
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

#[derive(Debug, Clone, Copy)]
enum Item {
    Quad(usize),
    Ribbon(usize),
}

/// 每帧展开用的临时数组，渲染器留着复用。
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    items: Vec<(ImageId, Blend, f32, Item)>,
    lengths: Vec<f32>,
}

/// 展开成三角形：透明的从远往近，叠加的按贴图分组画在后面；相邻同贴图同混合的合成一次 draw。返回有没有要软化的方片。
pub(crate) fn build(billboards: &Billboards, view: View, scratch: &mut Scratch, vertices: &mut Vec<BillboardVertex>, batches: &mut Vec<Batch>) -> bool {
    vertices.clear();
    batches.clear();
    let items = &mut scratch.items;
    items.clear();
    items.extend(billboards.quads.iter().enumerate().map(|(i, (t, b, q))| (*t, *b, view.depth(q.position), Item::Quad(i))));
    for (i, (t, b, range)) in billboards.ribbons.iter().enumerate() {
        let points = &billboards.points[range.clone()];
        let center = points.iter().map(|p| p.position).sum::<Vec3>() / points.len() as f32;
        items.push((*t, *b, view.depth(center), Item::Ribbon(i)));
    }
    // 透明在前、从远往近；叠加在后、按贴图（稳定排序，同一张贴图里保持原来的顺序）
    items.sort_by(|a, b| match (a.1, b.1) {
        (Blend::Alpha, Blend::Alpha) => b.2.total_cmp(&a.2),
        (Blend::Alpha, Blend::Add) => std::cmp::Ordering::Less,
        (Blend::Add, Blend::Alpha) => std::cmp::Ordering::Greater,
        (Blend::Add, Blend::Add) => a.0.cmp(&b.0),
    });
    let mut soft = false;
    for &(texture, blend, _, item) in items.iter() {
        let start = vertices.len() as u32;
        match item {
            Item::Quad(i) => {
                let q = &billboards.quads[i].2;
                soft |= q.soft > 0.0;
                quad(q, view, vertices);
            }
            Item::Ribbon(i) => ribbon(&billboards.points[billboards.ribbons[i].2.clone()], view, &mut scratch.lengths, vertices),
        }
        let end = vertices.len() as u32;
        match batches.last_mut() {
            Some(last) if last.texture == texture && last.blend == blend && last.vertices.end == start => last.vertices.end = end,
            _ if end > start => batches.push(Batch { texture, blend, vertices: start..end }),
            _ => {}
        }
    }
    soft
}

/// 方片平面里的右、上和法线（右 × 上 = 法线），还没乘半边长；拉长的已经乘好了，不再转。
fn axes(q: &Billboard, view: View) -> (Vec3, Vec3, Vec3, bool) {
    let camera = (view.rotation * Vec3::X, view.rotation * Vec3::Y, view.rotation * Vec3::Z, false);
    match q.facing {
        Facing::Camera => camera,
        Facing::Horizontal => (Vec3::X, Vec3::NEG_Z, Vec3::Y, false),
        Facing::Fixed(r) => (r * Vec3::X, r * Vec3::Y, r * Vec3::Z, false),
        Facing::Velocity { velocity, stretch } => {
            let Some(along) = velocity.try_normalize() else { return camera };
            // 法线取指向相机的方向里垂直于速度的那部分
            let Some(normal) = along.cross(view.to_eye(q.position).cross(along)).try_normalize() else { return camera };
            let length = q.size + velocity.length() * stretch.max(0.0);
            (along.cross(normal) * q.size, along * length, normal, true)
        }
    }
}

fn quad(q: &Billboard, view: View, out: &mut Vec<BillboardVertex>) {
    let (right, up, normal, stretched) = axes(q, view);
    let (r, u) = if stretched {
        (right / 2.0, up / 2.0)
    } else {
        let (sin, cos) = q.rotation.sin_cos();
        let half = q.size / 2.0;
        // 先在方片平面里转，再展开
        ((right * cos + up * sin) * half, (up * cos - right * sin) * half)
    };
    let params = [q.frame_mix, q.soft.max(0.0), if q.lit { 1.0 } else { 0.0 }, 0.0];
    let ([u0, v0, u1, v1], [n0, m0, n1, m1]) = (q.uv, q.next_uv);
    let corner = |x: f32, y: f32, uv: [f32; 2], uv2: [f32; 2]| BillboardVertex {
        position: (q.position + r * x + u * y).into(),
        uv,
        uv2,
        color: q.color.into(),
        normal: normal.into(),
        params,
    };
    let (a, b, c, d) = (corner(-1.0, 1.0, [u0, v0], [n0, m0]), corner(1.0, 1.0, [u1, v0], [n1, m0]), corner(1.0, -1.0, [u1, v1], [n1, m1]), corner(-1.0, -1.0, [u0, v1], [n0, m1]));
    out.extend([a, d, c, a, c, b]);
}

fn ribbon(points: &[RibbonPoint], view: View, lengths: &mut Vec<f32>, out: &mut Vec<BillboardVertex>) {
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
    lengths.clear();
    let mut total = 0.0;
    for (i, p) in points.iter().enumerate() {
        total += if i == 0 { 0.0 } else { p.position.distance(points[i - 1].position) };
        lengths.push(total);
    }
    let normal: [f32; 3] = (view.rotation * Vec3::Z).into();
    let vertex = |position: Vec3, uv: [f32; 2], color: Vec4| BillboardVertex { position: position.into(), uv, uv2: uv, color: color.into(), normal, params: [0.0; 4] };
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
        Billboard { position: Vec3::new(0.0, 0.0, z), size: 2.0, ..Default::default() }
    }

    fn built(b: &Billboards, view: View) -> (Vec<BillboardVertex>, Vec<Batch>) {
        let (mut v, mut batches) = (Vec::new(), Vec::new());
        build(b, view, &mut Scratch::default(), &mut v, &mut batches);
        (v, batches)
    }

    fn extent(p: &[Vec3]) -> (Vec3, Vec3) {
        p.iter().fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), q| (lo.min(*q), hi.max(*q)))
    }

    #[test]
    fn velocity_facing_stretches_along_the_velocity() {
        let mut b = Billboards::default();
        let moving = |velocity: Vec3| Billboard { size: 1.0, rotation: 1.0, facing: Facing::Velocity { velocity, stretch: 0.5 }, ..Default::default() };
        b.push(ImageId::WHITE, Blend::Add, moving(Vec3::new(4.0, 0.0, 0.0)));
        let (v, _) = built(&b, view());
        let p = positions(&v);
        let (lo, hi) = extent(&p);
        // 长 1 + 4 × 0.5 = 3，宽 1，转角不起作用
        assert!((lo - Vec3::new(-1.5, -0.5, 0.0)).length() < 1e-5 && (hi - Vec3::new(1.5, 0.5, 0.0)).length() < 1e-5, "{lo} {hi}");
        // 贴图的上边（v = 0）在前头
        assert!(v.iter().filter(|v| v.uv[1] == 0.0).all(|v| v.position[0] > 0.0));
        assert!(v.iter().all(|v| (Vec3::from(v.normal) - Vec3::Z).length() < 1e-5), "法线朝相机");
        // 速度是 0、正对相机飞：退成朝相机的方片（转角照用），不出 NaN
        for velocity in [Vec3::ZERO, Vec3::new(0.0, 0.0, -3.0)] {
            b.clear();
            b.push(ImageId::WHITE, Blend::Add, moving(velocity));
            let fallback = positions(&built(&b, view()).0);
            b.clear();
            b.push(ImageId::WHITE, Blend::Add, Billboard { size: 1.0, rotation: 1.0, ..Default::default() });
            let camera = positions(&built(&b, view()).0);
            assert!(fallback.iter().zip(&camera).all(|(a, c)| a.is_finite() && (*a - *c).length() < 1e-5), "{velocity}");
        }
        // 慢的短：速度 0.2 时长 1.1
        b.clear();
        b.push(ImageId::WHITE, Blend::Add, moving(Vec3::new(0.0, 0.2, 0.0)));
        let (lo, hi) = extent(&positions(&built(&b, view()).0));
        assert!(((hi - lo).y - 1.1).abs() < 1e-5 && ((hi - lo).x - 1.0).abs() < 1e-5, "{lo} {hi}");
    }

    #[test]
    fn horizontal_and_fixed_facings_ignore_the_camera() {
        let mut b = Billboards::default();
        b.push(ImageId::WHITE, Blend::Alpha, Billboard { position: Vec3::new(1.0, 2.0, 3.0), size: 2.0, facing: Facing::Horizontal, ..Default::default() });
        let (v, _) = built(&b, view());
        let (lo, hi) = extent(&positions(&v));
        assert!((lo - Vec3::new(0.0, 2.0, 2.0)).length() < 1e-5 && (hi - Vec3::new(2.0, 2.0, 4.0)).length() < 1e-5, "躺在地上 {lo} {hi}");
        assert!(v.iter().all(|v| Vec3::from(v.normal) == Vec3::Y));
        // 水平的转 90°：左上角（uv 0, 0）从 (-x, -z) 转到 (-x, +z)
        b.clear();
        b.push(ImageId::WHITE, Blend::Alpha, Billboard { size: 2.0, rotation: std::f32::consts::FRAC_PI_2, facing: Facing::Horizontal, ..Default::default() });
        let (v, _) = built(&b, view());
        assert!((Vec3::from(v[0].position) - Vec3::new(-1.0, 0.0, -1.0)).length() > 1.0, "转了 {:?}", v[0].position);
        // 固定朝向：绕 y 转 90°，方片在 yz 平面里，相机转到哪都一样
        b.clear();
        let q = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        b.push(ImageId::WHITE, Blend::Alpha, Billboard { size: 2.0, facing: Facing::Fixed(q), ..Default::default() });
        for v in [view(), View { rotation: Quat::from_rotation_x(0.7), eye: Vec4::new(0.0, 5.0, 5.0, 1.0) }] {
            let (vertices, _) = built(&b, v);
            assert!(vertices.iter().all(|v| v.position[0].abs() < 1e-5 && (Vec3::from(v.normal) - Vec3::X).length() < 1e-5));
        }
    }

    #[test]
    fn frames_soft_and_lit_go_into_the_vertices() {
        let mut b = Billboards::default();
        b.push(ImageId::WHITE, Blend::Alpha, Billboard { uv: [0.25, 0.5, 0.5, 0.75], next_uv: [0.5, 0.5, 0.75, 0.75], frame_mix: 0.3, soft: 2.0, lit: true, ..Default::default() });
        let (mut v, mut batches, mut scratch) = (Vec::new(), Vec::new(), Scratch::default());
        assert!(build(&b, view(), &mut scratch, &mut v, &mut batches), "有软粒子");
        // 左上、右下
        assert_eq!((v[0].uv, v[2].uv), ([0.25, 0.5], [0.5, 0.75]));
        assert_eq!((v[0].uv2, v[2].uv2), ([0.5, 0.5], [0.75, 0.75]));
        assert!(v.iter().all(|v| v.params == [0.3, 2.0, 1.0, 0.0]));
        b.clear();
        b.push(ImageId::WHITE, Blend::Alpha, Billboard::default());
        b.ribbon(ImageId::WHITE, Blend::Alpha, [RibbonPoint { position: Vec3::ZERO, width: 1.0, color: Vec4::ONE }, RibbonPoint { position: Vec3::X, width: 1.0, color: Vec4::ONE }]);
        assert!(!build(&b, view(), &mut scratch, &mut v, &mut batches), "没有软粒子");
        assert!(v.iter().all(|v| v.params == [0.0; 4] && v.uv == v.uv2), "默认整张贴图、不混、不软、不受光");
        assert_eq!(v[0].uv, [0.0, 0.0]);
    }

    #[test]
    fn buffers_are_reused_from_frame_to_frame() {
        let mut b = Billboards::default();
        let (mut v, mut batches, mut scratch) = (Vec::new(), Vec::new(), Scratch::default());
        let fill = |b: &mut Billboards, n: u32| {
            for i in 0..n {
                b.push(crate::ImageId::new(i % 3), Blend::Alpha, quad_at(i as f32));
                let point = |x: f32| RibbonPoint { position: Vec3::new(x, i as f32, 0.0), width: 1.0, color: Vec4::ONE };
                b.ribbon(ImageId::WHITE, Blend::Add, (0..8).map(|k| point(k as f32)));
            }
        };
        let capacities = |b: &Billboards, s: &Scratch, v: &Vec<BillboardVertex>, batches: &Vec<Batch>| {
            [b.quads.capacity(), b.ribbons.capacity(), b.points.capacity(), s.items.capacity(), s.lengths.capacity(), v.capacity(), batches.capacity()]
        };
        fill(&mut b, 50);
        build(&b, view(), &mut scratch, &mut v, &mut batches);
        let first = capacities(&b, &scratch, &v, &batches);
        assert!(first.iter().all(|c| *c > 0));
        // 少的一帧也不缩、不重新分配
        for n in [50, 5, 50] {
            b.clear();
            fill(&mut b, n);
            build(&b, view(), &mut scratch, &mut v, &mut batches);
            assert_eq!(capacities(&b, &scratch, &v, &batches), first, "容量不变");
        }
        assert_eq!((b.quads(), b.ribbons()), (50, 50));
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
