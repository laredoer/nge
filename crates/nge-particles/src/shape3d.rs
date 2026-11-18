//! 发射形状（见 docs/particles3d-more.md）：在本地坐标里取出生的位置和那里的法线。

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use glam::{Quat, Vec3};
use nge_assets::Handle;
use nge_render3d::{Assets3d, Gltf, ModelId};

use crate::rng::Rng;

/// 粒子在哪出生（发射器的本地坐标，随实体旋转，不随缩放）。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum EmitShape {
    /// 发射器的位置。
    #[default]
    Point,
    /// 球面上（`surface`）或球体里均匀。
    Sphere { radius: f32, surface: bool },
    /// 本地 +Y 那半个球。
    Hemisphere { radius: f32, surface: bool },
    /// `size` 是整个长宽高；表面按面积选面。
    Box { size: Vec3, surface: bool },
    /// 本地 xz 平面上 `inner` 到 `radius` 的圆环：`inner` 为 0 是圆盘，等于 `radius` 是一圈线。
    Circle { radius: f32, inner: f32 },
    /// 底面半径 `radius` 的圆盘上出生，越靠边越往外偏，最外圈偏 `angle`（弧度）；`radius` 为 0 时在半角 `angle` 的锥里散开。
    Cone { radius: f32, angle: f32 },
    /// 网格表面，按三角形面积加权。
    Mesh(MeshSource),
}

/// 发射方向怎么定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmitDirection {
    /// `direction` 加 `spread` 的锥（和形状无关）。
    #[default]
    Cone,
    /// 沿出生处的法线，再在 `spread` 的锥里散开；点没有法线，用 `direction`。
    Normal,
    /// 球面上均匀，不看 `spread`。
    Random,
}

/// 网格表面从哪来。
#[derive(Debug, Clone, PartialEq)]
pub enum MeshSource {
    /// 自己给的三角形。
    Surface(Arc<MeshSurface>),
    /// `Assets3d` 里的模型（乘部件的 `local` 和 `scale`）。
    Model { model: ModelId, scale: Vec3 },
    /// glTF 加载完后展开成模型再取。
    Gltf { gltf: Handle<Gltf>, scale: Vec3 },
}

/// 网格表面：三角形（位置、法线）和面积的前缀和。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MeshSurface {
    triangles: Vec<[(Vec3, Vec3); 3]>,
    /// 到第 i 个三角形为止（含）的面积和。
    areas: Vec<f32>,
}

impl MeshSurface {
    /// 三个顶点各带法线；面积是 0 或不是有限数的三角形丢掉。
    pub fn new(triangles: impl IntoIterator<Item = [(Vec3, Vec3); 3]>) -> Self {
        let mut surface = Self::default();
        let mut total = 0.0;
        for t in triangles {
            let area = (t[1].0 - t[0].0).cross(t[2].0 - t[0].0).length() / 2.0;
            if area > 0.0 && area.is_finite() {
                total += area;
                surface.triangles.push(t);
                surface.areas.push(total);
            }
        }
        surface
    }

    /// 模型所有部件的三角形（乘部件的 `local` 和 `scale`）。没有这个模型时是空的。
    pub fn from_model(assets: &Assets3d, model: ModelId, scale: Vec3) -> Self {
        let Some(model) = assets.model(model) else { return Self::default() };
        let triangles = model.parts.iter().filter_map(|part| Some((part, assets.mesh(part.mesh)?))).flat_map(|(part, mesh)| {
            let normal_matrix = part.local.inverse().transpose();
            let vertex = move |i: u32| {
                let v = mesh.vertices[i as usize];
                // 位置是先 local 再缩放，法线就是先乘 local 的逆转置再除以缩放
                let n = (normal_matrix.transform_vector3(Vec3::from(v.normal)) / scale).normalize_or_zero();
                (part.local.transform_point3(Vec3::from(v.position)) * scale, n)
            };
            mesh.indices.chunks_exact(3).map(move |t| [vertex(t[0]), vertex(t[1]), vertex(t[2])])
        });
        Self::new(triangles)
    }

    pub fn area(&self) -> f32 {
        self.areas.last().copied().unwrap_or(0.0)
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    fn sample(&self, rng: &mut Rng) -> Option<(Vec3, Vec3)> {
        let total = self.area();
        if self.triangles.is_empty() {
            return None;
        }
        let pick = rng.next_f32() * total;
        let i = self.areas.partition_point(|a| *a <= pick).min(self.triangles.len() - 1);
        let [a, b, c] = self.triangles[i];
        // 三角形里均匀：sqrt 法
        let (r1, r2) = (rng.next_f32().sqrt(), rng.next_f32());
        let (wa, wb, wc) = (1.0 - r1, r1 * (1.0 - r2), r1 * r2);
        let position = a.0 * wa + b.0 * wb + c.0 * wc;
        let normal = (a.1 * wa + b.1 * wb + c.1 * wc).try_normalize().unwrap_or_else(|| (b.0 - a.0).cross(c.0 - a.0).normalize_or_zero());
        Some((position, normal))
    }
}

/// 负数、NaN 当 0。
fn size(x: f32) -> f32 {
    if x.is_nan() { 0.0 } else { x.max(0.0) }
}

/// 球面上均匀的单位向量。
pub(crate) fn unit_sphere(rng: &mut Rng) -> Vec3 {
    let y = rng.next_f32() * 2.0 - 1.0;
    let r = (1.0 - y * y).max(0.0).sqrt();
    let phi = rng.next_f32() * TAU;
    Vec3::new(r * phi.cos(), y, r * phi.sin())
}

/// 在以 `axis` 为轴、半角 `spread` 的锥形里按立体角均匀取一个方向；`axis` 是零向量时当成 +Y。
pub(crate) fn cone(rng: &mut Rng, axis: Vec3, spread: f32) -> Vec3 {
    // NaN 当成全方向
    let spread = if spread.is_nan() { PI } else { spread.clamp(0.0, PI) };
    // cosθ 在 [cos(spread), 1] 里均匀取，立体角就是均匀的
    let cos = 1.0 - rng.next_f32() * (1.0 - spread.cos());
    let sin = (1.0 - cos * cos).max(0.0).sqrt();
    let phi = rng.next_f32() * TAU;
    let local = Vec3::new(sin * phi.cos(), cos, sin * phi.sin());
    let axis = axis.try_normalize().unwrap_or(Vec3::Y);
    Quat::from_rotation_arc(Vec3::Y, axis) * local
}

/// 本地坐标里的（出生位置，法线）；点的法线是 0。网格没有三角形（`surface` 是空的或还没有）时是 `None`。
pub(crate) fn sample(shape: &EmitShape, rng: &mut Rng, surface: Option<&MeshSurface>) -> Option<(Vec3, Vec3)> {
    Some(match *shape {
        EmitShape::Point => (Vec3::ZERO, Vec3::ZERO),
        EmitShape::Sphere { radius, surface } | EmitShape::Hemisphere { radius, surface } => {
            let mut n = unit_sphere(rng);
            if matches!(shape, EmitShape::Hemisphere { .. }) {
                n.y = n.y.abs();
            }
            // 体积均匀：半径按立方根取
            let r = size(radius) * if surface { 1.0 } else { rng.next_f32().cbrt() };
            (n * r, n)
        }
        EmitShape::Box { size: s, surface } => {
            let half = Vec3::new(size(s.x), size(s.y), size(s.z)) / 2.0;
            let mut p = Vec3::new(rng.next_f32() * 2.0 - 1.0, rng.next_f32() * 2.0 - 1.0, rng.next_f32() * 2.0 - 1.0) * half;
            if !surface {
                return Some((p, p.normalize_or_zero()));
            }
            // 三对面按面积选（yz、xz、xy），再随机选正反
            let areas = [half.y * half.z, half.x * half.z, half.x * half.y];
            let total = areas.iter().sum::<f32>();
            let mut pick = rng.next_f32() * total;
            let axis = (0..3).find(|&i| {
                pick -= areas[i];
                pick < 0.0
            });
            let sign = if rng.next_f32() < 0.5 { -1.0 } else { 1.0 };
            let Some(axis) = axis.filter(|_| total > 0.0) else { return Some((p, p.normalize_or_zero())) };
            p[axis] = half[axis] * sign;
            let mut n = Vec3::ZERO;
            n[axis] = sign;
            (p, n)
        }
        EmitShape::Circle { radius, inner } => {
            let outer = size(radius);
            let inner = size(inner).min(outer);
            // 面积均匀：r² 在 [inner², outer²] 里均匀
            let r = (inner * inner + (outer * outer - inner * inner) * rng.next_f32()).sqrt();
            let phi = rng.next_f32() * TAU;
            let n = Vec3::new(phi.cos(), 0.0, phi.sin());
            (n * r, n)
        }
        EmitShape::Cone { radius, angle } => {
            let radius = size(radius);
            let angle = size(angle).min(PI);
            if radius == 0.0 {
                return Some((Vec3::ZERO, cone(rng, Vec3::Y, angle)));
            }
            let k = rng.next_f32().sqrt();
            let phi = rng.next_f32() * TAU;
            let out = Vec3::new(phi.cos(), 0.0, phi.sin());
            let tilt = angle * k;
            (out * radius * k, Vec3::Y * tilt.cos() + out * tilt.sin())
        }
        EmitShape::Mesh(_) => surface?.sample(rng)?,
    })
}

/// 世界坐标里的发射方向：`axis` 是转过的 `direction`，`normal` 是转过的出生处法线。
pub(crate) fn direction(mode: EmitDirection, rng: &mut Rng, axis: Vec3, spread: f32, normal: Vec3) -> Vec3 {
    match mode {
        EmitDirection::Cone => cone(rng, axis, spread),
        EmitDirection::Normal if normal != Vec3::ZERO => cone(rng, normal, spread),
        EmitDirection::Normal => cone(rng, axis, spread),
        EmitDirection::Random => unit_sphere(rng),
    }
}

/// 模型、glTF 的网格表面，按（来源，缩放）缓存；加载完前没有。
#[derive(bevy_ecs::prelude::Resource, Debug, Default)]
pub struct MeshSurfaces(std::collections::HashMap<MeshKey, Option<Arc<MeshSurface>>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum MeshKey {
    Model(ModelId, [u32; 3]),
    Gltf(Handle<Gltf>, [u32; 3]),
}

fn key(source: &MeshSource) -> Option<MeshKey> {
    let bits = |s: Vec3| [s.x.to_bits(), s.y.to_bits(), s.z.to_bits()];
    match source {
        MeshSource::Surface(_) => None,
        MeshSource::Model { model, scale } => Some(MeshKey::Model(*model, bits(*scale))),
        MeshSource::Gltf { gltf, scale } => Some(MeshKey::Gltf(*gltf, bits(*scale))),
    }
}

impl MeshSurfaces {
    /// 这个来源的表面；还没加载、加载失败是 `None`。
    pub fn get(&self, source: &MeshSource) -> Option<Arc<MeshSurface>> {
        match source {
            MeshSource::Surface(s) => Some(s.clone()),
            _ => self.0.get(&key(source)?)?.clone(),
        }
    }

    /// 加载完了但没有三角形（或加载失败）：不用再等。
    pub fn failed(&self, source: &MeshSource) -> bool {
        key(source).is_some_and(|k| matches!(self.0.get(&k), Some(None)))
    }

    /// 缓存了几个来源（包括失败的）。
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// 发射器（包括子发射器）里用到的模型、glTF 表面还没缓存的，能取就取（`PostUpdate`，模拟之前）；没人用的从缓存里清掉。
/// 模型编号还不存在时不缓存，下一帧再试。
pub(crate) fn resolve_surfaces(world: &mut bevy_ecs::prelude::World, mut used: bevy_ecs::prelude::Local<Vec<MeshKey>>, mut emitters: bevy_ecs::prelude::Local<Option<bevy_ecs::query::QueryState<&crate::ParticleEmitter3d>>>) {
    fn collect(e: &crate::ParticleEmitter3d, out: &mut Vec<MeshKey>) {
        if let EmitShape::Mesh(source) = &e.shape
            && let Some(k) = key(source)
            && !out.contains(&k)
        {
            out.push(k);
        }
        for sub in &e.sub_emitters {
            collect(&sub.emitter, out);
        }
    }
    used.clear();
    let emitters = emitters.get_or_insert_with(|| world.query());
    for e in emitters.iter(world) {
        collect(e, &mut used);
    }
    if used.is_empty() && world.resource::<MeshSurfaces>().is_empty() {
        return;
    }
    world.resource_mut::<MeshSurfaces>().0.retain(|k, _| used.contains(k));
    for &k in used.iter() {
        if world.resource::<MeshSurfaces>().0.contains_key(&k) {
            continue;
        }
        let (model, scale) = match k {
            MeshKey::Model(model, scale) => (Some(model), scale),
            MeshKey::Gltf(gltf, scale) => match nge_render3d::gltf_model(world, gltf) {
                None => continue,
                Some(model) => (model, scale),
            },
        };
        let scale = Vec3::new(f32::from_bits(scale[0]), f32::from_bits(scale[1]), f32::from_bits(scale[2]));
        // 模型编号还没有：可能是之后才加进来的，不当失败
        if model.is_some_and(|m| world.resource::<Assets3d>().model(m).is_none()) {
            continue;
        }
        let surface = model.map(|m| MeshSurface::from_model(world.resource::<Assets3d>(), m, scale)).filter(|s| !s.is_empty());
        if surface.is_none() {
            log::warn!("3D 粒子的网格发射形状 {k:?} 没有三角形，不发射");
        }
        world.resource_mut::<MeshSurfaces>().0.insert(k, surface.map(Arc::new));
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;

    const N: usize = 4000;

    fn points(shape: &EmitShape, surface: Option<&MeshSurface>) -> Vec<(Vec3, Vec3)> {
        let mut rng = Rng::new(11);
        (0..N).map(|_| sample(shape, &mut rng, surface).unwrap()).collect()
    }

    fn fraction(p: &[(Vec3, Vec3)], f: impl Fn(&(Vec3, Vec3)) -> bool) -> f32 {
        p.iter().filter(|x| f(x)).count() as f32 / p.len() as f32
    }

    #[test]
    fn spheres_on_the_shell_or_filling_the_volume() {
        let shell = points(&EmitShape::Sphere { radius: 2.0, surface: true }, None);
        assert!(shell.iter().all(|(p, n)| (p.length() - 2.0).abs() < 1e-4 && (*n - *p / 2.0).length() < 1e-4), "球面上，法线往外");
        assert!(shell.iter().map(|(p, _)| *p).sum::<Vec3>().length() / (N as f32) < 0.1, "各方向均匀");
        let ball = points(&EmitShape::Sphere { radius: 2.0, surface: false }, None);
        assert!(ball.iter().all(|(p, _)| p.length() <= 2.0 + 1e-4));
        // 体积均匀：半径一半以内是 1/8
        assert!((fraction(&ball, |(p, _)| p.length() < 1.0) - 0.125).abs() < 0.03);
        let half = points(&EmitShape::Hemisphere { radius: 1.0, surface: true }, None);
        assert!(half.iter().all(|(p, _)| p.y >= 0.0) && fraction(&half, |(p, _)| p.y > 0.5) > 0.3, "上半球");
        let half = points(&EmitShape::Hemisphere { radius: 1.0, surface: false }, None);
        assert!(half.iter().all(|(p, _)| p.y >= 0.0 && p.length() <= 1.0 + 1e-4));
    }

    #[test]
    fn boxes_pick_faces_by_area() {
        let size = Vec3::new(4.0, 1.0, 1.0);
        let shell = points(&EmitShape::Box { size, surface: true }, None);
        let on_face = |p: Vec3| (p.x.abs() - 2.0).abs() < 1e-4 || (p.y.abs() - 0.5).abs() < 1e-4 || (p.z.abs() - 0.5).abs() < 1e-4;
        assert!(shell.iter().all(|(p, _)| on_face(*p) && p.abs().cmple(size / 2.0 + 1e-4).all()));
        // ±x 两面面积各 1，总面积 18
        assert!((fraction(&shell, |(p, _)| (p.x.abs() - 2.0).abs() < 1e-4) - 2.0 / 18.0).abs() < 0.02);
        assert!(shell.iter().filter(|(p, _)| (p.x - 2.0).abs() < 1e-4).all(|(_, n)| *n == Vec3::X), "法线是那个面的朝向");
        let solid = points(&EmitShape::Box { size, surface: false }, None);
        assert!(solid.iter().all(|(p, _)| p.abs().cmple(size / 2.0).all()));
        assert!(fraction(&solid, |(p, _)| p.x.abs() < 1.0) > 0.4);
    }

    #[test]
    fn circles_fill_the_ring_evenly_and_point_outwards() {
        let ring = points(&EmitShape::Circle { radius: 2.0, inner: 1.0 }, None);
        assert!(ring.iter().all(|(p, _)| p.y == 0.0 && (1.0 - 1e-4..=2.0 + 1e-4).contains(&p.length())));
        // 面积均匀：一半面积在 sqrt((1 + 4) / 2) 以内
        assert!((fraction(&ring, |(p, _)| p.length() < 2.5f32.sqrt()) - 0.5).abs() < 0.03);
        assert!(ring.iter().all(|(p, n)| (*n - p.normalize()).length() < 1e-4), "法线在平面里往外");
        let line = points(&EmitShape::Circle { radius: 2.0, inner: 2.0 }, None);
        assert!(line.iter().all(|(p, _)| (p.length() - 2.0).abs() < 1e-4), "inner = radius 是一圈线");
        let disc = points(&EmitShape::Circle { radius: 2.0, inner: 5.0 }, None);
        assert!(disc.iter().all(|(p, _)| (p.length() - 2.0).abs() < 1e-4), "inner 超过 radius 当一圈线");
    }

    #[test]
    fn cones_tilt_outwards_with_the_distance_from_the_middle() {
        let angle = 30f32.to_radians();
        let cone = points(&EmitShape::Cone { radius: 1.0, angle }, None);
        assert!(cone.iter().all(|(p, _)| p.y == 0.0 && p.length() <= 1.0 + 1e-4));
        for (p, n) in &cone {
            let tilt = n.angle_between(Vec3::Y);
            assert!((tilt - angle * p.length()).abs() < 1e-3, "离中心 {} 偏 {tilt}", p.length());
            if p.length() > 0.1 {
                assert!(n.dot(*p) > 0.0, "往外偏");
            }
        }
        // 底面半径 0：在锥里散开，偏角不超过 angle
        let point = points(&EmitShape::Cone { radius: 0.0, angle }, None);
        assert!(point.iter().all(|(p, n)| *p == Vec3::ZERO && n.angle_between(Vec3::Y) <= angle + 1e-4));
        assert!(point.iter().any(|(_, n)| n.angle_between(Vec3::Y) > angle * 0.9));
    }

    #[test]
    fn bad_sizes_collapse_to_a_point() {
        for shape in [EmitShape::Sphere { radius: -1.0, surface: true }, EmitShape::Sphere { radius: f32::NAN, surface: false }, EmitShape::Box { size: Vec3::NAN, surface: true }, EmitShape::Circle { radius: f32::NAN, inner: -1.0 }] {
            assert!(points(&shape, None).iter().all(|(p, n)| *p == Vec3::ZERO && n.is_finite()), "{shape:?}");
        }
        assert!(points(&EmitShape::Point, None).iter().all(|(p, n)| *p == Vec3::ZERO && *n == Vec3::ZERO), "点没有法线");
    }

    #[test]
    fn meshes_pick_triangles_by_area() {
        let tri = |a: Vec3, b: Vec3, c: Vec3| [(a, Vec3::Y), (b, Vec3::Y), (c, Vec3::Y)];
        // 面积 1 和 3 的两个三角形，中间一个面积 0 的
        let surface = MeshSurface::new([
            tri(Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0)),
            tri(Vec3::ZERO, Vec3::X, Vec3::new(2.0, 0.0, 0.0)),
            tri(Vec3::new(10.0, 0.0, 0.0), Vec3::new(12.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 3.0)),
        ]);
        assert!((surface.area() - 4.0).abs() < 1e-5);
        let p = points(&EmitShape::Mesh(MeshSource::Surface(std::sync::Arc::new(surface.clone()))), Some(&surface));
        assert!((fraction(&p, |(p, _)| p.x < 5.0) - 0.25).abs() < 0.03, "按面积");
        assert!(p.iter().all(|(p, n)| *n == Vec3::Y && p.y == 0.0));
        assert!(p.iter().filter(|(p, _)| p.x < 5.0).all(|(p, _)| p.x + 2.0 * p.z <= 2.0 + 1e-4 && p.z >= 0.0), "在三角形里");
        assert!(p.iter().filter(|(p, _)| p.x >= 5.0).all(|(p, _)| (p.x - 10.0) / 2.0 + p.z / 3.0 <= 1.0 + 1e-4), "在三角形里");
        let mut rng = Rng::new(1);
        let shape = EmitShape::Mesh(MeshSource::Surface(Default::default()));
        assert!(sample(&shape, &mut rng, Some(&MeshSurface::default())).is_none(), "没有三角形不发射");
        assert!(sample(&shape, &mut rng, None).is_none(), "还没加载不发射");
    }

    #[test]
    fn mesh_normals_survive_rotation_and_uneven_scale() {
        use nge_render3d::{MaterialId, Mesh, Model, Part};
        let mut assets = Assets3d::default();
        let mesh = assets.add_mesh(Mesh::plane(2.0)).unwrap();
        let local = glam::Mat4::from_rotation_z(std::f32::consts::FRAC_PI_4);
        let model = assets.add_model(Model { parts: vec![Part::new(mesh, MaterialId::DEFAULT, local)], ..Default::default() }).unwrap();
        let surface = MeshSurface::from_model(&assets, model, Vec3::new(1.0, 4.0, 1.0));
        // 平面先转 45° 再沿 y 拉长 4 倍：沿着平面的方向是 (1, 4, 0) 和 z，法线垂直于它们
        let expected = Vec3::new(-4.0, 1.0, 0.0).normalize();
        let mut rng = Rng::new(5);
        for _ in 0..50 {
            let (_, n) = surface.sample(&mut rng).unwrap();
            assert!((n - expected).length() < 1e-4, "{n}");
        }
    }

    #[test]
    fn directions_follow_the_mode() {
        let mut rng = Rng::new(3);
        let normal = Vec3::new(1.0, 1.0, 0.0).normalize();
        for _ in 0..200 {
            assert!((direction(EmitDirection::Normal, &mut rng, Vec3::Y, 0.0, normal) - normal).length() < 1e-4, "沿法线");
            assert!(direction(EmitDirection::Normal, &mut rng, Vec3::Y, 0.3, normal).angle_between(normal) <= 0.3 + 1e-3, "沿法线再散开");
            assert!(direction(EmitDirection::Cone, &mut rng, Vec3::Z, 0.0, normal).abs_diff_eq(Vec3::Z, 1e-4), "方向锥不看法线");
            assert!(direction(EmitDirection::Normal, &mut rng, Vec3::Z, 0.0, Vec3::ZERO).abs_diff_eq(Vec3::Z, 1e-4), "没有法线用 direction");
        }
        let random: Vec<Vec3> = (0..N).map(|_| direction(EmitDirection::Random, &mut rng, Vec3::Y, 0.0, normal)).collect();
        assert!(random.iter().all(|d| (d.length() - 1.0).abs() < 1e-4));
        assert!(random.iter().sum::<Vec3>().length() / (N as f32) < 0.05, "随机：各方向均匀，不看 spread");
    }
}
