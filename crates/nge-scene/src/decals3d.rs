//! 3D 贴地光环与贴花（`scene.decals3d`）：内置 `ring` / `rune` / `pulse` / `shadow` 程序化纹理，
//! 支持跟随 3D 模型、自动贴合 `NavMesh3d` 地形起伏、旋转与呼吸脉冲动画，并复用 `Billboards` 自动合批绘制。

use std::collections::HashMap;
use std::f32::consts::TAU;

use bevy_ecs::prelude::*;
use glam::{Quat, UVec2, Vec2, Vec3, Vec4};
use nge_ai::NavMesh3d;
use nge_assets::{Assets, Image, ImageId};
use nge_ecs::Time;
use nge_render3d::{Billboard, Billboards, Blend, Facing, Transform3d};
use serde::Deserialize;

use crate::nodes::ModelNode;
use crate::protocol::{Color, Scene};
use crate::sync::{Declared, SceneId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecalBlend {
    Alpha,
    Add,
}

/// `scene.decals3d` 的一项：3D 贴地光环、法阵、技能预警圈或脚底投影。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decal3dNode {
    pub id: String,
    /// 内置图案：`"ring"`（光环）| `"rune"`（古代法阵）| `"pulse"`（扩散波纹）| `"shadow"`（柔和脚底影）。默认 `"ring"`。
    #[serde(default = "default_kind")]
    pub kind: String,
    /// 跟随目标 3D 模型的 id；不写时使用 `(x, y, z)` 固定世界坐标。
    pub follow: Option<String>,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub z: f32,
    /// 贴花直径（世界单位），默认 40。
    #[serde(default = "default_size")]
    pub size: f32,
    /// 颜色（`#rrggbb` / `#rrggbbaa`）；不写时 `shadow` 用半透黑，其余用纯白。
    pub color: Option<Color>,
    /// 自转速度（度 / 秒），默认 0。
    #[serde(default)]
    pub spin: f32,
    /// 呼吸脉冲缩放幅度（0–1，如 0.12 表示 ±12%），默认 0。
    #[serde(default)]
    pub pulse: f32,
    /// 混合方式：`"add"` 或 `"alpha"`；不写时 `shadow` 用 `"alpha"`，其余用 `"add"`。
    pub blend: Option<DecalBlend>,
    /// 是否按 `NavMesh3d` 自动采样地面高度贴地（默认 true）。
    #[serde(default = "yes")]
    pub ground: bool,
    /// 离地微抬高度（默认 2.5，防止与地形 Z-fighting）。
    #[serde(default = "default_lift")]
    pub lift: f32,
}

fn default_kind() -> String {
    "ring".into()
}
fn default_size() -> f32 {
    40.0
}
fn default_lift() -> f32 {
    2.5
}
fn yes() -> bool {
    true
}

/// 缓存按需生成的内置贴花纹理，避免重复上传。
#[derive(Resource, Default)]
pub(crate) struct DecalTextures {
    cache: HashMap<String, ImageId>,
}

impl DecalTextures {
    fn get_or_create(&mut self, kind: &str, images: &mut Assets<Image>) -> ImageId {
        if let Some(&id) = self.cache.get(kind) {
            return id;
        }
        let side = 128u32;
        let rgba = match kind {
            "rune" => generate_rune(side),
            "pulse" => generate_pulse(side),
            "shadow" => generate_shadow(side),
            _ => generate_ring(side),
        };
        let id = images.add_rgba(UVec2::splat(side), rgba).unwrap_or(ImageId::WHITE);
        self.cache.insert(kind.to_owned(), id);
        id
    }
}

fn sample_grid(side: u32, mut f: impl FnMut(f32, f32, f32, f32) -> [f32; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity((side * side * 4) as usize);
    let inv = 1.0 / (side as f32 - 1.0);
    for py in 0..side {
        let ny = (py as f32 * inv) * 2.0 - 1.0;
        for px in 0..side {
            let nx = (px as f32 * inv) * 2.0 - 1.0;
            let r = (nx * nx + ny * ny).sqrt();
            let ang = ny.atan2(nx);
            let [cr, cg, cb, ca] = f(nx, ny, r, ang);
            for c in [cr, cg, cb, ca] {
                out.push((c.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
    }
    out
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn generate_ring(side: u32) -> Vec<u8> {
    sample_grid(side, |_, _, r, _| {
        let outer = 1.0 - smoothstep(0.03, 0.085, (r - 0.84).abs());
        let inner = (1.0 - smoothstep(0.0, 0.05, (r - 0.68).abs())) * 0.45;
        let fill = (1.0 - smoothstep(0.0, 0.86, r)) * 0.14;
        let a = (outer + inner + fill).clamp(0.0, 1.0);
        [1.0, 1.0, 1.0, a]
    })
}

fn generate_pulse(side: u32) -> Vec<u8> {
    sample_grid(side, |_, _, r, _| {
        let band1 = 1.0 - smoothstep(0.02, 0.09, (r - 0.86).abs());
        let band2 = (1.0 - smoothstep(0.02, 0.08, (r - 0.56).abs())) * 0.65;
        let band3 = (1.0 - smoothstep(0.02, 0.07, (r - 0.28).abs())) * 0.35;
        let a = (band1 + band2 + band3).clamp(0.0, 1.0);
        [1.0, 1.0, 1.0, a]
    })
}

fn generate_rune(side: u32) -> Vec<u8> {
    sample_grid(side, |nx, ny, r, ang| {
        let outer = 1.0 - smoothstep(0.018, 0.052, (r - 0.90).abs());
        let mid = (1.0 - smoothstep(0.015, 0.042, (r - 0.72).abs())) * 0.85;
        // 8 方位符文刻度
        let spokes = ((ang * 4.0).cos().abs()).powf(18.0) * smoothstep(0.70, 0.74, r) * (1.0 - smoothstep(0.87, 0.91, r));
        // 旋转双正方形形成八角星法阵
        let sq1 = nx.abs().max(ny.abs());
        let rx = (nx + ny) * 0.70710677;
        let ry = (nx - ny) * 0.70710677;
        let sq2 = rx.abs().max(ry.abs());
        let star1 = (1.0 - smoothstep(0.015, 0.042, (sq1 - 0.51).abs())) * 0.7;
        let star2 = (1.0 - smoothstep(0.015, 0.042, (sq2 - 0.51).abs())) * 0.7;
        let glow = (1.0 - smoothstep(0.0, 0.92, r)) * 0.12;
        let a = (outer + mid + spokes * 0.9 + star1 + star2 + glow).clamp(0.0, 1.0);
        [1.0, 1.0, 1.0, a]
    })
}

fn generate_shadow(side: u32) -> Vec<u8> {
    sample_grid(side, |_, _, r, _| {
        let a = (1.0 - smoothstep(0.15, 0.95, r)).powf(1.6);
        [1.0, 1.0, 1.0, a]
    })
}

/// `PostUpdate`（在物理、挂点与导航之后）：把 `scene.decals3d` 提交到 `Billboards` 渲染。
pub(crate) fn draw_decals3d(
    time: Res<Time>,
    scene: Res<Scene>,
    navmesh: Res<NavMesh3d>,
    mut images: ResMut<Assets<Image>>,
    mut textures: ResMut<DecalTextures>,
    mut billboards: ResMut<Billboards>,
    models: Query<(&SceneId, &Transform3d), With<Declared<ModelNode>>>,
) {
    if scene.decals3d.is_empty() {
        return;
    }
    let t = time.elapsed as f32;
    for decal in &scene.decals3d {
        if !(decal.size.is_finite() && decal.size > 0.0) {
            continue;
        }
        let mut pos = if let Some(target_id) = decal.follow.as_deref() {
            let Some((_, target_t)) = models.iter().find(|(id, _)| id.0 == target_id) else { continue };
            target_t.translation + Vec3::new(decal.x, decal.y, decal.z)
        } else {
            Vec3::new(decal.x, decal.y, decal.z)
        };
        let pulse_scale = if decal.pulse != 0.0 { 1.0 + (t * 3.2).sin() * decal.pulse.clamp(-0.9, 0.9) } else { 1.0 };
        let final_size = decal.size * pulse_scale;
        let mut facing = Facing::Horizontal;

        if decal.ground && !navmesh.is_empty() {
            let r = (final_size * 0.45).max(4.0);
            let sample = |ox: f32, oz: f32| navmesh.sample_height(Vec2::new(pos.x + ox, pos.z + oz), pos.y);
            let h0 = sample(0.0, 0.0).unwrap_or(pos.y);
            let h_px = sample(r, 0.0).unwrap_or(h0);
            let h_nx = sample(-r, 0.0).unwrap_or(h0);
            let h_pz = sample(0.0, r).unwrap_or(h0);
            let h_nz = sample(0.0, -r).unwrap_or(h0);

            // 按左右/前后高度差估算局部坡面法线，让贴花平行于斜坡而不是水平切进山体
            let sx = (h_nx - h_px) / (2.0 * r);
            let sz = (h_nz - h_pz) / (2.0 * r);
            let normal = Vec3::new(sx, 1.0, sz).normalize();

            // 9 点（中心 + 4 正向 + 4 对角）检查曲面凹陷处是否高于贴花平面，避免边缘被地形裁切
            let d = r * 0.70710677;
            let mut extra_lift = 0.0f32;
            for (ox, oz, h) in [
                (0.0, 0.0, h0),
                (r, 0.0, h_px),
                (-r, 0.0, h_nx),
                (0.0, r, h_pz),
                (0.0, -r, h_nz),
                (d, d, sample(d, d).unwrap_or(h0)),
                (-d, d, sample(-d, d).unwrap_or(h0)),
                (d, -d, sample(d, -d).unwrap_or(h0)),
                (-d, -d, sample(-d, -d).unwrap_or(h0)),
            ] {
                let plane_y = h0 - (sx * ox + sz * oz);
                extra_lift = extra_lift.max(h - plane_y);
            }

            pos.y = h0 + extra_lift + decal.lift;
            let q = Quat::from_rotation_arc(Vec3::Y, normal) * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
            facing = Facing::Fixed(q);
        } else {
            pos.y += decal.lift;
        }

        let tex = textures.get_or_create(&decal.kind, &mut images);
        let is_shadow = decal.kind == "shadow";
        let blend = match decal.blend {
            Some(DecalBlend::Alpha) => Blend::Alpha,
            Some(DecalBlend::Add) => Blend::Add,
            None => if is_shadow { Blend::Alpha } else { Blend::Add },
        };
        let color = decal.color.map_or_else(
            || if is_shadow { Vec4::new(0.0, 0.0, 0.0, 0.52) } else { Vec4::ONE },
            |c| c.0,
        );
        let rotation = (decal.spin.to_radians() * t) % TAU;

        billboards.push(
            tex,
            blend,
            Billboard {
                position: pos,
                size: final_size,
                rotation,
                color,
                facing,
                ..Default::default()
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{app, frame};

    #[test]
    fn decals3d_push_horizontal_billboards_and_follow_models() {
        let mut app = app();
        frame(&mut app, Some(json!({
            "models": [{ "id": "hero", "model": "asset:/@t/crate.glb", "x": 10, "y": 5, "z": -3 }],
            "decals3d": [
                { "id": "ring", "kind": "rune", "follow": "hero", "size": 50, "spin": 30, "color": "#45e8ff" },
                { "id": "shadow", "kind": "shadow", "follow": "hero", "size": 30 },
            ]
        })));
        let billboards = app.world().resource::<nge_render3d::Billboards>();
        assert_eq!(billboards.quads(), 2, "两个 3D 贴花应提交为 2 个水平方片");
    }
}
