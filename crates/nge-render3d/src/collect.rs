//! 每帧从 World 里收集相机、灯光和要画的 draw；不透明的按管线、贴图、网格排（便于 GPU Instancing 合批），半透明的按到相机的距离由远到近排。
//! 收集用的临时列表放在 [`Scratch`] 里，每帧清空复用。

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};

use nge_assets::ImageId;

use crate::animator::{Pose, padded};
use crate::assets::{Assets3d, MaterialId, MaterialStyle, MaterialStyleOverride, MeshId};
use crate::billboards::View;
use crate::camera::{Camera3d, Transform3d};
use crate::light::{AmbientLight, DirectionalLight, Globals, PointLight, SpotLight};
use crate::mesh::MAX_MORPH_TARGETS;
use crate::morph::{MorphWeights, pack_weights};
use crate::renderer::{Mesh3d, Model3d};
use crate::shader3d::MaterialShader3d;
use crate::shadow::CameraView;
use crate::shadow_soft::{Candidate, ShadowPriority};

/// 材质的 5 张贴图：[base_color, normal, metallic_roughness, occlusion, emissive]。
pub(crate) type MaterialTextures = [ImageId; 5];

/// DrawRaw.skin[1] 标志位。
pub(crate) const FLAG_NORMAL: u32 = 1;
pub(crate) const FLAG_MR: u32 = 2;
pub(crate) const FLAG_AO: u32 = 4;
pub(crate) const FLAG_EMISSIVE: u32 = 8;
pub(crate) const FLAG_INSTANCED: u32 = 16;

/// 把两个 [0, 16) 范围内的浮点缩放因子压进一个 u32（1.0 精确等于 4096）。
pub(crate) fn pack_scales(normal_scale: f32, occlusion_strength: f32) -> u32 {
    let pack = |v: f32| (v.clamp(0.0, 15.999) * 4096.0).round() as u32;
    (pack(normal_scale) & 0xffff) | ((pack(occlusion_strength) & 0xffff) << 16)
}

/// GPU 实例化每个实例的变换矩阵（和着色器里的 `Instance` 一一对应）。
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct InstanceRaw {
    pub model: [[f32; 4]; 4],
    pub normal: [[f32; 4]; 4],
}

/// 和着色器里的 `Draw` 一一对应。
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct DrawRaw {
    pub model: [[f32; 4]; 4],
    pub normal: [[f32; 4]; 4],
    pub base_color: [f32; 4],
    pub params: [f32; 4],
    /// w > 0：半透明。
    pub emissive: [f32; 4],
    /// x：关节矩阵起点；y：贴图与实例化标志位；z：材质风格（0 PBR / 1 Toon / 2 Water / 3 Foliage）；w：打包的 normal_scale 与 occlusion_strength。
    pub skin: [u32; 4],
    /// 变形差起点、目标数、顶点数、实例化缓冲起点（FLAG_INSTANCED 时有效）。
    pub morph: [u32; 4],
    pub weights: [[f32; 4]; 2],
    /// 自定义 3D 着色器参数（来自 MaterialShader3d::params）。
    pub custom: [f32; 4],
}

#[derive(Clone)]
pub(crate) struct Draw {
    pub mesh: MeshId,
    /// base_color 贴图（排序主键之一，兼容原有行为）。
    pub texture: ImageId,
    /// 全部 5 张材质贴图（bind group 缓存键）。
    pub textures: MaterialTextures,
    pub material: MaterialId,
    /// 0–2 按剔除方式，蒙皮的再加 3，半透明的再加 6。
    pub pipeline: usize,
    /// 可选自定义 3D 着色器名（在 Shaders3d 里查找）。
    pub shader: Option<String>,
    /// 蒙皮的：关节矩阵在这一帧关节缓冲里的（起点, 个数）。
    pub joints: Option<(u32, u32)>,
    pub blend: bool,
    /// 半透明的：包围盒中心到相机的线性视深（见 [`blend_depth`]），排序用。
    pub depth: f32,
    pub raw: DrawRaw,
}

impl Draw {
    /// 两个不透明静态 draw 能否合进同一个 GPU Instancing 批次。
    pub(crate) fn can_batch_with(&self, other: &Self) -> bool {
        !self.blend
            && !other.blend
            && self.joints.is_none()
            && other.joints.is_none()
            && self.raw.morph[1] == 0
            && other.raw.morph[1] == 0
            && self.pipeline == other.pipeline
            && self.shader == other.shader
            && self.mesh == other.mesh
            && self.textures == other.textures
            && self.raw.base_color.map(f32::to_bits) == other.raw.base_color.map(f32::to_bits)
            && self.raw.params.map(f32::to_bits) == other.raw.params.map(f32::to_bits)
            && self.raw.emissive.map(f32::to_bits) == other.raw.emissive.map(f32::to_bits)
            && self.raw.custom.map(f32::to_bits) == other.raw.custom.map(f32::to_bits)
            && (self.raw.skin[1] & !FLAG_INSTANCED) == (other.raw.skin[1] & !FLAG_INSTANCED)
            && self.raw.skin[2] == other.raw.skin[2]
            && self.raw.skin[3] == other.raw.skin[3]
    }
}

/// 不透明的在前，按管线、自定义着色器、贴图、网格、材质排（减少切换并便于静态合批）；半透明的在后，由远到近。
pub(crate) fn sort_draws(draws: &mut [Draw]) {
    draws.sort_by(|a, b| {
        a.blend.cmp(&b.blend).then_with(|| {
            if a.blend {
                b.depth.total_cmp(&a.depth).then((a.pipeline, a.texture).cmp(&(b.pipeline, b.texture)))
            } else {
                (a.pipeline, a.shader.as_deref(), a.texture, a.textures, a.mesh.0, a.material.0, a.raw.skin[2])
                    .cmp(&(b.pipeline, b.shader.as_deref(), b.texture, b.textures, b.mesh.0, b.material.0, b.raw.skin[2]))
            }
        })
    });
}

/// 世界包围盒中心沿相机前方向到相机的距离，越大越远；在相机背后的是负的。
/// 透视、正交都适用（不经过投影，中心在相机背后时顺序也不会反）。
pub(crate) fn blend_depth(eye: &Transform3d, (lo, hi): (Vec3, Vec3)) -> f32 {
    ((lo + hi) * 0.5 - eye.translation).dot(eye.rotation * Vec3::NEG_Z)
}

/// 一个要画的部件：网格、材质、模型矩阵、蒙皮的关节范围、变形权重、可选风格覆盖、自定义着色器与参数。
type Item = (MeshId, MaterialId, Mat4, Option<(u32, u32)>, [f32; MAX_MORPH_TARGETS], Option<MaterialStyle>, Option<String>, [f32; 4]);

/// 每帧收集用的临时列表，清空后复用，不每帧分配。
#[derive(Default)]
pub(crate) struct Scratch {
    dirs: Vec<DirectionalLight>,
    points: Vec<(PointLight, Vec3, crate::light::Rank)>,
    spots: Vec<(SpotLight, Vec3, Vec3, crate::light::Rank)>,
    items: Vec<Item>,
    /// 这一帧开了阴影的聚光灯、点光源（按优先级挑之前）。
    pub candidates: Vec<Candidate>,
}

/// 没有相机实体时，按默认相机放在 (0, 0, 5) 看向原点。
/// 画 3D 用哪个相机：第一个 `Camera3d`，没有时是 (0, 0, 5) 朝 -z 的默认相机。
pub fn main_camera(first: Option<(&Camera3d, &Transform3d)>) -> (Camera3d, Transform3d) {
    first.map(|(c, t)| (*c, *t)).unwrap_or_else(|| (Camera3d::default(), Transform3d::from_xyz(0.0, 0.0, 5.0)))
}

pub(crate) fn globals(world: &mut World, size: Vec2, scratch: &mut Scratch) -> ((Globals, bool), View, CameraView) {
    let (camera, eye) = main_camera(world.query::<(&Camera3d, &Transform3d)>().iter(world).next());
    let aspect = size.x / size.y.max(1.0);
    let view_proj = camera.view_proj(&eye, aspect);
    let ambient = *world.resource::<AmbientLight>();
    let elapsed = world.get_resource::<nge_ecs::Time>().map_or(0.0, |t| t.elapsed as f32);
    let Scratch { dirs, points, spots, candidates, .. } = scratch;
    let priority = |p: Option<&ShadowPriority>| p.map_or(0, |p| p.0);
    dirs.clear();
    dirs.extend(world.query::<&DirectionalLight>().iter(world).copied());
    points.clear();
    points.extend(world.query::<(Entity, &PointLight, &Transform3d, Option<&ShadowPriority>)>().iter(world).map(|(e, l, t, p)| (*l, t.translation, (priority(p), e.to_bits()))));
    spots.clear();
    spots.extend(
        world.query::<(Entity, &SpotLight, &Transform3d, Option<&ShadowPriority>)>().iter(world).map(|(e, l, t, p)| (*l, t.translation, t.rotation * Vec3::NEG_Z, (priority(p), e.to_bits()))),
    );
    let view = View { rotation: eye.rotation, eye: camera.eye(&eye) };
    let (mut g, dropped) = Globals::new(
        view_proj,
        camera.eye(&eye),
        ambient,
        dirs.iter(),
        points.iter().map(|(l, p, k)| (l, *p, *k)),
        spots.iter().map(|(l, p, d, k)| (l, *p, *d, *k)),
        candidates,
    );
    // ambient.w 存经过的秒数，给水波 / 树叶摆动着色器用
    g.ambient[3] = elapsed;
    ((g, dropped), view, CameraView { camera, eye, aspect })
}

/// 收集这一帧的 draw；蒙皮的部件把关节矩阵追加到 `joints`。
pub(crate) fn collect(world: &mut World, draws: &mut Vec<Draw>, joints: &mut Vec<Mat4>, scratch: &mut Scratch) {
    draws.clear();
    joints.clear();
    let items = &mut scratch.items;
    items.clear();
    for (mesh, t, weights, style, shader) in world.query::<(&Mesh3d, &Transform3d, Option<&MorphWeights>, Option<&MaterialStyleOverride>, Option<&MaterialShader3d>)>().iter(world) {
        items.push((mesh.mesh, mesh.material, t.matrix(), None, padded(weights.map_or(&[][..], |w| &w.0)), style.map(|s| s.0), shader.map(|s| s.name.clone()), shader.map_or([0.0; 4], |s| s.params.into())));
    }
    // 先建查询再借资源（建查询要 &mut World）
    let mut models = world.query::<(&Model3d, &Transform3d, Option<&Pose>, Has<crate::Animator>, Has<nge_anim::AnimGraph>, Option<&MaterialStyleOverride>, Option<&MaterialShader3d>)>();
    let assets = world.resource::<Assets3d>();
    for (model, t, pose, animator, graph, style_override, shader) in models.iter(world) {
        let playing = animator || graph;
        let Some(model) = assets.model(model.0) else { continue };
        let m = t.matrix();
        let skeleton = &model.skeleton;
        // 单独去掉 Animator（或状态机）时 Pose 还在，不再按它画
        let pose = pose.filter(|p| playing && p.nodes.len() == skeleton.nodes.len() && p.weights.len() == skeleton.nodes.len());
        let posed = pose.map(|p| &p.nodes[..]);
        // 在播动画时按这一帧的权重，不然用部件的默认权重
        let weights = |p: &crate::Part| match (p.node, pose) {
            (Some(n), Some(pose)) => pose.weights[n as usize],
            _ => padded(&p.weights),
        };
        let skinned = |p: &crate::Part| p.skin.is_some() && assets.mesh(p.mesh).is_some_and(|mesh| !mesh.skin.is_empty());
        // 蒙皮的部件要每个节点的矩阵，没在播动画时用不播动画的姿势
        let rest;
        let nodes = match posed {
            None if model.parts.iter().any(skinned) => {
                rest = skeleton.rest();
                Some(&rest[..])
            }
            nodes => nodes,
        };
        let shader_name = shader.map(|s| s.name.clone());
        let custom = shader.map_or([0.0; 4], |s| s.params.into());
        for p in &model.parts {
            match (p.skin, nodes) {
                (Some(skin), Some(nodes)) if skinned(p) => {
                    let start = joints.len();
                    skeleton.joints(skin as usize, nodes, joints);
                    items.push((p.mesh, p.material, m, Some((start as u32, (joints.len() - start) as u32)), weights(p), style_override.map(|s| s.0), shader_name.clone(), custom));
                }
                _ => {
                    let local = match (p.node, posed) {
                        (Some(n), Some(nodes)) => nodes[n as usize],
                        _ => p.local,
                    };
                    items.push((p.mesh, p.material, m * local, None, weights(p), style_override.map(|s| s.0), shader_name.clone(), custom));
                }
            }
        }
    }
    for (mesh, material, model, skin, weights, style_override, shader, custom) in items.iter() {
        let (mesh, material, model, skin, style_override, custom) = (*mesh, *material, *model, *skin, *style_override, *custom);
        // 缩放为 0 的物体看不见，也没有法线矩阵
        let det = model.determinant();
        let inverse = model.inverse();
        if det == 0.0 || !inverse.is_finite() {
            continue;
        }
        // 行列式为负（镜像）时三角形绕序反了，单面材质改剔正面，着色器里也按此判断正反面
        let mirrored = det < 0.0;
        let m = assets.material(material);
        let cull = if m.double_sided { 2 } else { usize::from(mirrored) };
        let base_tex = m.base_color_texture.unwrap_or(ImageId::WHITE);
        let normal_tex = m.normal_texture.unwrap_or(ImageId::WHITE);
        let mr_tex = m.metallic_roughness_texture.unwrap_or(ImageId::WHITE);
        let ao_tex = m.occlusion_texture.unwrap_or(ImageId::WHITE);
        let emissive_tex = m.emissive_texture.unwrap_or(ImageId::WHITE);
        let mut flags = 0u32;
        if m.normal_texture.is_some() {
            flags |= FLAG_NORMAL;
        }
        if m.metallic_roughness_texture.is_some() {
            flags |= FLAG_MR;
        }
        if m.occlusion_texture.is_some() {
            flags |= FLAG_AO;
        }
        if m.emissive_texture.is_some() {
            flags |= FLAG_EMISSIVE;
        }
        let style = style_override.unwrap_or(m.style).as_u32();
        draws.push(Draw {
            mesh,
            texture: base_tex,
            textures: [base_tex, normal_tex, mr_tex, ao_tex, emissive_tex],
            material,
            pipeline: cull + 3 * usize::from(skin.is_some()) + 6 * usize::from(m.blend),
            shader: shader.clone(),
            joints: skin,
            blend: m.blend,
            depth: 0.0,
            raw: DrawRaw {
                model: model.to_cols_array_2d(),
                normal: inverse.transpose().to_cols_array_2d(),
                base_color: m.base_color.into(),
                params: [m.metallic, m.roughness, m.alpha_cutoff.unwrap_or(-1.0), if mirrored { 1.0 } else { 0.0 }],
                emissive: m.emissive.extend(if m.blend { 1.0 } else { 0.0 }).into(),
                skin: [skin.map_or(0, |s| s.0), flags, style, pack_scales(m.normal_scale, m.occlusion_strength)],
                // 渲染前按网格填
                morph: [0; 4],
                weights: pack_weights(weights),
                custom,
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use nge_assets::{Assets, Image};

    use super::*;

    #[test]
    fn draw_layout_matches_shader() {
        assert_eq!(size_of::<DrawRaw>(), 64 * 2 + 16 * 8);
        assert_eq!(size_of::<InstanceRaw>(), 64 * 2);
    }

    #[test]
    fn pack_scales_preserves_unit_and_clamps() {
        let p = pack_scales(1.0, 0.5);
        let n = (p & 0xffff) as f32 / 4096.0;
        let ao = ((p >> 16) & 0xffff) as f32 / 4096.0;
        assert_eq!((n, ao), (1.0, 0.5));
    }

    fn draw(pipeline: usize, texture: ImageId, blend: bool, depth: f32) -> Draw {
        Draw {
            mesh: MeshId(0),
            texture,
            textures: [texture, ImageId::WHITE, ImageId::WHITE, ImageId::WHITE, ImageId::WHITE],
            material: MaterialId::DEFAULT,
            pipeline,
            shader: None,
            joints: None,
            blend,
            depth,
            raw: DrawRaw::zeroed(),
        }
    }

    #[test]
    fn opaque_draws_first_then_blended_far_to_near() {
        let mut images = Assets::<Image>::with_white();
        let other = images.add_rgba(glam::UVec2::ONE, vec![0; 4]).unwrap();
        let white = ImageId::WHITE;
        let input = vec![
            draw(6, white, true, 0.3),
            draw(1, other, false, 0.2),
            draw(9, other, true, 0.9),
            draw(0, other, false, 0.8),
            draw(0, white, false, 0.1),
            draw(7, white, true, 0.5),
        ];
        let key = |d: &Draw| (d.pipeline, d.texture, d.blend, d.depth.to_bits());
        let expected = vec![(0, white, false, 0.1), (0, other, false, 0.8), (1, other, false, 0.2), (9, other, true, 0.9), (7, white, true, 0.5), (6, white, true, 0.3)];
        let expected: Vec<_> = expected.into_iter().map(|(p, t, b, z)| (p, t, b, f32::to_bits(z))).collect();
        // 实体顺序不影响结果
        for reversed in [false, true] {
            let mut draws = input.clone();
            if reversed {
                draws.reverse();
            }
            sort_draws(&mut draws);
            assert_eq!(draws.iter().map(key).collect::<Vec<_>>(), expected, "reversed={reversed}");
        }
    }

    #[test]
    fn static_opaque_draws_with_same_mesh_and_material_can_batch() {
        let a = draw(0, ImageId::WHITE, false, 0.0);
        let b = draw(0, ImageId::WHITE, false, 0.0);
        assert!(a.can_batch_with(&b));
        let mut diff_mesh = draw(0, ImageId::WHITE, false, 0.0);
        diff_mesh.mesh = MeshId(1);
        assert!(!a.can_batch_with(&diff_mesh));
        let mut skinned = draw(3, ImageId::WHITE, false, 0.0);
        skinned.joints = Some((0, 4));
        assert!(!a.can_batch_with(&skinned));
        let blended = draw(6, ImageId::WHITE, true, 0.5);
        assert!(!a.can_batch_with(&blended));
    }

    fn cube_at(z: f32) -> (Vec3, Vec3) {
        (Vec3::new(-1.0, -1.0, z - 0.5), Vec3::new(1.0, 1.0, z + 0.5))
    }

    #[test]
    fn blend_depth_grows_with_distance() {
        let eye = Transform3d::from_xyz(0.0, 0.0, 5.0);
        let (near, far) = (blend_depth(&eye, cube_at(1.5)), blend_depth(&eye, cube_at(-2.5)));
        assert!(far > near, "远 {far}，近 {near}");
        assert!((near - 3.5).abs() < 1e-5 && (far - 7.5).abs() < 1e-5, "沿前方向的线性距离：{near} {far}");
        // 斜着看：只算前方向上的分量
        let turned = Transform3d::from_xyz(0.0, 0.0, 0.0).looking_at(Vec3::X, Vec3::Y);
        assert!((blend_depth(&turned, (Vec3::new(4.0, 3.0, -9.0), Vec3::new(4.0, 3.0, -9.0))) - 4.0).abs() < 1e-5);
    }

    /// 透视相机：中心在相机背后的半透明物体比前面的近，排在后面画（z / w 在背后会反号，顺序就反了）。
    #[test]
    fn blended_objects_behind_the_camera_sort_as_nearest() {
        let eye = Transform3d::from_xyz(0.0, 0.0, 5.0);
        let (behind, front) = (cube_at(7.0), cube_at(0.0));
        let mut draws = vec![draw(6, ImageId::WHITE, true, blend_depth(&eye, behind)), draw(7, ImageId::WHITE, true, blend_depth(&eye, front))];
        sort_draws(&mut draws);
        assert_eq!(draws.iter().map(|d| d.pipeline).collect::<Vec<_>>(), [7, 6], "先画前面远的，再画背后近的");
    }
}
