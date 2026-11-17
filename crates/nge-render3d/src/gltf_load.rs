//! glTF 2.0 加载：`.glb` 或 `.gltf`，展开默认场景的节点树成一个 [`Model`]，带上蒙皮和动画。
//!
//! 走资源层（[`Gltf`]）时外部的 `.bin` 和贴图按相对路径读；[`Assets3d::load_gltf`] 直接给字节时只认内嵌的数据。

use std::collections::HashMap;

use glam::{Mat4, UVec2, Vec2, Vec3, Vec4};
use gltf::animation::util::ReadOutputs;
use gltf::image::Format;
use gltf::material::AlphaMode;
use gltf::mesh::Mode;
use nge_assets::{Asset, Assets, Handle, Image, ImageId, LoadContext};

use crate::animation::{Animation, Channel, Interpolation, Node, Property, Skeleton, Skin, Trs};
use crate::assets::{Assets3d, Material, MaterialId, MeshId, Model, ModelId, Part};
use crate::mesh::{Influence, Mesh, MorphTarget, Vertex};

/// 节点树的最大深度，防止病态文件把栈撑爆。
const MAX_DEPTH: usize = 256;

/// 解析好的 glTF：文档、缓冲区和解码后的贴图（解码失败的贴图是 None）。在资源层的后台线程上解析。
pub struct Gltf {
    doc: gltf::Document,
    buffers: Vec<gltf::buffer::Data>,
    images: Vec<Option<Image>>,
}

impl Gltf {
    /// `read`：按 glTF 里写的相对 uri 读外部文件。
    pub fn parse(bytes: &[u8], read: impl Fn(&str) -> Result<Vec<u8>, String>) -> Result<Self, String> {
        let gltf::Gltf { document: doc, blob } = gltf::Gltf::from_slice(bytes).map_err(|e| format!("glTF 解析失败: {e}"))?;
        let mut blob = blob;
        let mut buffers = Vec::new();
        for buffer in doc.buffers() {
            let data = match buffer.source() {
                gltf::buffer::Source::Uri(uri) if !uri.starts_with("data:") => {
                    let mut bytes = read(&decode_uri(uri)?)?;
                    // 和 gltf 自己的读法一致：补齐到 4 字节
                    bytes.resize(bytes.len().next_multiple_of(4), 0);
                    gltf::buffer::Data(bytes)
                }
                source => gltf::buffer::Data::from_source_and_blob(source, None, &mut blob).map_err(|e| format!("glTF 缓冲区 {}: {e}", buffer.index()))?,
            };
            if data.len() < buffer.length() {
                return Err(format!("glTF 缓冲区 {} 只有 {} 字节，需要 {}", buffer.index(), data.len(), buffer.length()));
            }
            buffers.push(data);
        }
        // gltf 读 bufferView 时直接切片，越界会 panic，先查一遍
        for view in doc.views() {
            let end = view.offset().checked_add(view.length());
            if end.is_none_or(|end| end > buffers[view.buffer().index()].len()) {
                return Err(format!("glTF bufferView {} 越界", view.index()));
            }
        }
        let images = doc
            .images()
            .map(|image| {
                let decoded = match image.source() {
                    gltf::image::Source::Uri { uri, .. } if !uri.starts_with("data:") => decode_uri(uri).and_then(|uri| read(&uri)).and_then(|bytes| Image::decode(&bytes)),
                    // base 只用来读外部文件（上面已经分走），给 None 时 gltf 连 data: 也不解
                    source => gltf::image::Data::from_source(source, Some(std::path::Path::new(".")), &buffers).map_err(|e| e.to_string()).and_then(|data| to_rgba(&data)),
                };
                decoded.map_err(|e| log::warn!("glTF 贴图 {}: {e}，材质改用不带贴图", image.index())).ok()
            })
            .collect();
        Ok(Self { doc, buffers, images })
    }
}

impl std::fmt::Debug for Gltf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gltf").field("nodes", &self.doc.nodes().len()).field("images", &self.images.len()).finish()
    }
}

impl Asset for Gltf {
    fn decode(bytes: Vec<u8>, ctx: &LoadContext) -> Result<Self, String> {
        Gltf::parse(&bytes, |uri| ctx.read_relative(uri))
    }
}

/// glTF 里的 uri 是百分号编码的（空格写成 `%20`）。
fn decode_uri(uri: &str) -> Result<String, String> {
    percent_encoding::percent_decode_str(uri).decode_utf8().map(Into::into).map_err(|e| format!("uri {uri:?} 不合法: {e}"))
}

fn to_rgba(data: &gltf::image::Data) -> Result<Image, String> {
    let rgba = match data.format {
        Format::R8G8B8A8 => data.pixels.clone(),
        Format::R8G8B8 => data.pixels.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        Format::R8G8 => data.pixels.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        Format::R8 => data.pixels.iter().flat_map(|&p| [p, p, p, 255]).collect(),
        other => return Err(format!("格式 {other:?} 不支持")),
    };
    Image::from_rgba(UVec2::new(data.width, data.height), rgba)
}

impl Assets3d {
    /// 直接给字节（通常来自 `include_bytes!`），只认 `.glb` 和内嵌 data URI 的数据。见 [`Self::add_gltf`]。
    pub fn load_gltf(&mut self, images: &mut Assets<Image>, bytes: &[u8]) -> Result<ModelId, String> {
        let gltf = Gltf::parse(bytes, |uri| Err(format!("直接给字节时不读外部文件 {uri}，改用 AssetServer 加载")))?;
        self.add_gltf(images, &gltf)
    }

    /// 只读三角形图元、base color 贴图和材质系数（见 `docs/m4-3d.md`），其余忽略并记 warn。
    /// 贴图放进 `images`（和 2D 共用）。失败时什么都不留下：贴图成功后才加，已经加进来的网格和材质会撤掉。
    pub fn add_gltf(&mut self, images: &mut Assets<Image>, gltf: &Gltf) -> Result<ModelId, String> {
        let marks = (self.meshes.len(), self.materials.len());
        match self.add_gltf_inner(images.len(), gltf) {
            Ok((model, new_images)) => {
                for image in new_images {
                    images.add(image);
                }
                Ok(model)
            }
            Err(e) => {
                self.meshes.truncate(marks.0);
                self.materials.truncate(marks.1);
                Err(e)
            }
        }
    }

    /// `first_image`：新贴图从这个编号开始（返回后按顺序加进贴图仓库，编号就对得上）。
    fn add_gltf_inner(&mut self, first_image: usize, gltf: &Gltf) -> Result<(ModelId, Vec<Image>), String> {
        let doc = &gltf.doc;
        let scene = doc.default_scene().or_else(|| doc.scenes().next()).ok_or("glTF 里没有场景")?;
        let mut loader = Loader {
            assets: self,
            buffers: &gltf.buffers,
            images: &gltf.images,
            visited: vec![false; doc.nodes().len()],
            meshes: HashMap::new(),
            materials: HashMap::new(),
            default_material: None,
            textures: HashMap::new(),
            first_image,
            new_images: Vec::new(),
            parts: Vec::new(),
            nodes: Vec::new(),
            node_index: vec![None; doc.nodes().len()],
            skins: Vec::new(),
        };
        for node in scene.nodes() {
            loader.node(&node, Mat4::IDENTITY, None, 0)?;
        }
        let skins = loader.skins(doc);
        let animations = doc.animations().map(|a| loader.animation(&a)).collect();
        let (mut parts, new_images) = (loader.parts, loader.new_images);
        for part in &mut parts {
            // 用到的蒙皮读不出来时按不蒙皮画
            part.skin = part.skin.and_then(|gltf_skin| loader.skins.iter().position(|&s| s == gltf_skin as usize)).filter(|&i| skins[i].is_some()).map(|i| i as u32);
        }
        // 读不出来的蒙皮留个空位，下标才对得上
        let skins = skins.into_iter().map(|s| s.unwrap_or(Skin { joints: Vec::new(), inverse_bind: Vec::new() })).collect();
        let model = Model { parts, skeleton: Skeleton { nodes: loader.nodes, skins }, animations };
        Ok((self.add_model(model)?, new_images))
    }
}

struct Loader<'a> {
    assets: &'a mut Assets3d,
    buffers: &'a [gltf::buffer::Data],
    images: &'a [Option<Image>],
    /// glTF 要求节点构成树：一个节点只能出现一次，出现第二次就是成环或被多个父节点共用。
    visited: Vec<bool>,
    /// (glTF 网格, 图元) → 网格；同一个网格被多个节点引用时只存一份。
    meshes: HashMap<(usize, usize), MeshId>,
    materials: HashMap<usize, MaterialId>,
    /// 没指定材质的图元用 glTF 规范的默认材质（白色、metallic 1、roughness 1）。
    default_material: Option<MaterialId>,
    textures: HashMap<usize, ImageId>,
    first_image: usize,
    /// 解出来的贴图，整份加载成功后才放进贴图仓库。
    new_images: Vec<Image>,
    /// 部件的 `skin` 先记 glTF 的蒙皮编号，最后换成 `skins` 里的下标。
    parts: Vec<Part>,
    /// 按父在前的顺序展开的节点。
    nodes: Vec<Node>,
    /// glTF 节点编号 → `nodes` 里的下标（不在默认场景里的是 None）。
    node_index: Vec<Option<u32>>,
    /// 用到的 glTF 蒙皮编号，按第一次用到的顺序。
    skins: Vec<usize>,
}

impl Loader<'_> {
    fn node(&mut self, node: &gltf::Node, parent: Mat4, parent_index: Option<u32>, depth: usize) -> Result<(), String> {
        if depth >= MAX_DEPTH {
            return Err(format!("glTF 节点层级超过 {MAX_DEPTH} 层"));
        }
        if std::mem::replace(&mut self.visited[node.index()], true) {
            return Err(format!("glTF 节点 {} 出现了不止一次（成环或被多个父节点共用）", node.index()));
        }
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        let (translation, rotation, scale) = node.transform().decomposed();
        let index = self.nodes.len() as u32;
        let rest = Trs { translation: translation.into(), rotation: crate::animation::rotation(Vec4::from(rotation)), scale: scale.into() };
        self.nodes.push(Node { parent: parent_index, rest });
        self.node_index[node.index()] = Some(index);
        let skin = node.skin().map(|s| s.index());
        if let Some(s) = skin.filter(|s| !self.skins.contains(s)) {
            self.skins.push(s);
        }
        if let Some(mesh) = node.mesh() {
            // 默认变形权重：节点写了用节点的，不然用网格的
            let weights = node.weights().or(mesh.weights()).unwrap_or_default().to_vec();
            for prim in mesh.primitives() {
                if prim.mode() != Mode::Triangles {
                    log::warn!("glTF 网格 {} 的图元模式 {:?} 不支持，跳过", mesh.index(), prim.mode());
                    continue;
                }
                let key = (mesh.index(), prim.index());
                let mesh_id = match self.meshes.get(&key) {
                    Some(&id) => id,
                    None => {
                        let id = self.assets.add_mesh(self.primitive(&prim)?)?;
                        self.meshes.insert(key, id);
                        id
                    }
                };
                let material = match prim.material().index() {
                    Some(i) => self.material(&prim.material(), i),
                    None => *self.default_material.get_or_insert_with(|| {
                        self.assets.add_material(Material { metallic: 1.0, roughness: 1.0, ..Default::default() })
                    }),
                };
                let skinned = skin.filter(|_| self.assets.mesh(mesh_id).is_some_and(|m| !m.skin.is_empty()));
                self.parts.push(Part { node: Some(index), skin: skinned.map(|s| s as u32), weights: weights.clone(), ..Part::new(mesh_id, material, world) });
            }
        }
        for child in node.children() {
            self.node(&child, world, Some(index), depth + 1)?;
        }
        Ok(())
    }

    fn primitive(&self, prim: &gltf::Primitive) -> Result<Mesh, String> {
        let reader = prim.reader(|b| self.buffers.get(b.index()).map(|d| &d.0[..]));
        let positions: Vec<Vec3> = reader.read_positions().ok_or("图元没有 POSITION")?.map(Vec3::from).collect();
        let normals: Option<Vec<Vec3>> = reader.read_normals().map(|n| n.map(Vec3::from).collect());
        let uvs: Vec<Vec2> = reader.read_tex_coords(0).map(|t| t.into_f32().map(Vec2::from).collect()).unwrap_or_default();
        let indices = match reader.read_indices() {
            Some(i) => i.into_u32().collect(),
            None => (0..positions.len() as u32).collect(),
        };
        let vertices: Vec<Vertex> = positions
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                let n = normals.as_ref().and_then(|n| n.get(i)).copied().unwrap_or(Vec3::ZERO);
                Vertex::new(p, n, uvs.get(i).copied().unwrap_or(Vec2::ZERO))
            })
            .collect();
        let skin = match (reader.read_joints(0), reader.read_weights(0)) {
            (Some(joints), Some(weights)) => joints
                .into_u16()
                .zip(weights.into_f32())
                .map(|(joints, weights)| {
                    // 权重加起来应是 1，导出工具常有误差
                    let sum: f32 = weights.iter().sum();
                    let weights = if sum > 0.0 { weights.map(|w| w / sum) } else { weights };
                    Influence { joints, weights }
                })
                .collect(),
            _ => Vec::new(),
        };
        if !skin.is_empty() && skin.len() != positions.len() {
            return Err(format!("图元有 {} 个顶点、{} 份关节权重", positions.len(), skin.len()));
        }
        // 变形目标：没写 POSITION 的位置不变；切线差不支持
        let n = vertices.len();
        let targets = reader
            .read_morph_targets()
            .map(|(positions, normals, _)| MorphTarget {
                positions: positions.map_or_else(|| vec![Vec3::ZERO; n], |p| p.map(Vec3::from).collect()),
                normals: normals.map(|n| n.map(Vec3::from).collect()).unwrap_or_default(),
            })
            .collect();
        let mut mesh = Mesh { vertices, indices, skin, targets };
        if normals.is_none() {
            mesh.compute_normals();
        }
        Ok(mesh)
    }

    /// 用到的蒙皮，按 `self.skins` 的顺序；关节不在默认场景里、逆绑定矩阵数不对的读不出来（None，warn）。
    fn skins(&self, doc: &gltf::Document) -> Vec<Option<Skin>> {
        let skins: Vec<gltf::Skin> = doc.skins().collect();
        self.skins
            .iter()
            .map(|&i| {
                let skin = &skins[i];
                let joints: Option<Vec<u32>> = skin.joints().map(|j| self.node_index[j.index()]).collect();
                let Some(joints) = joints else {
                    log::warn!("glTF 蒙皮 {i} 的关节不在默认场景里，按不蒙皮画");
                    return None;
                };
                let reader = skin.reader(|b| self.buffers.get(b.index()).map(|d| &d.0[..]));
                let inverse_bind: Vec<Mat4> = match reader.read_inverse_bind_matrices() {
                    Some(m) => m.map(|m| Mat4::from_cols_array_2d(&m)).collect(),
                    None => vec![Mat4::IDENTITY; joints.len()],
                };
                if inverse_bind.len() != joints.len() {
                    log::warn!("glTF 蒙皮 {i} 有 {} 个关节、{} 个逆绑定矩阵，按不蒙皮画", joints.len(), inverse_bind.len());
                    return None;
                }
                Some(Skin { joints, inverse_bind })
            })
            .collect()
    }

    /// 没名字的动画按编号叫（"0"、"1"…）。目标不在默认场景里的通道跳过；变形权重通道按每 4 个目标拆成几条。
    fn animation(&self, animation: &gltf::Animation) -> Animation {
        let name = animation.name().map_or_else(|| animation.index().to_string(), str::to_owned);
        let mut channels = Vec::new();
        for (i, channel) in animation.channels().enumerate() {
            let target = channel.target();
            let Some(node) = self.node_index[target.node().index()] else { continue };
            let reader = channel.reader(|b| self.buffers.get(b.index()).map(|d| &d.0[..]));
            let (Some(times), Some(outputs)) = (reader.read_inputs(), reader.read_outputs()) else {
                log::warn!("glTF 动画 {name:?} 的通道 {i} 读不出数据，跳过");
                continue;
            };
            let interpolation = match channel.sampler().interpolation() {
                gltf::animation::Interpolation::Step => Interpolation::Step,
                gltf::animation::Interpolation::Linear => Interpolation::Linear,
                gltf::animation::Interpolation::CubicSpline => Interpolation::CubicSpline,
            };
            let times: Vec<f32> = times.collect();
            let tracks: Vec<(Property, Vec<Vec4>)> = match outputs {
                ReadOutputs::Translations(v) => vec![(Property::Translation, v.map(|v| Vec3::from(v).extend(0.0)).collect())],
                ReadOutputs::Rotations(v) => vec![(Property::Rotation, v.into_f32().map(Vec4::from).collect())],
                ReadOutputs::Scales(v) => vec![(Property::Scale, v.map(|v| Vec3::from(v).extend(0.0)).collect())],
                ReadOutputs::MorphTargetWeights(v) => {
                    let per_key = if interpolation == Interpolation::CubicSpline { 3 } else { 1 };
                    let flat: Vec<f32> = v.into_f32().collect();
                    match crate::gltf_morph::split_weights(&flat, times.len(), per_key) {
                        Ok(segments) => segments.into_iter().map(|(s, values)| (Property::Weights(s), values)).collect(),
                        Err(bad) => {
                            log::warn!("glTF 动画 {name:?} 的变形权重通道 {i}：{bad}，跳过");
                            continue;
                        }
                    }
                }
            };
            for (property, values) in tracks {
                let channel = Channel { node, property, interpolation, times: times.clone(), values };
                // 坏通道只丢这一条，不让整个模型加载失败
                match channel.problem(self.nodes.len()) {
                    Some(bad) => log::warn!("glTF 动画 {name:?} 的通道 {i}：{bad}，跳过"),
                    None => channels.push(channel),
                }
            }
        }
        let duration = channels.iter().filter_map(|c| c.times.last().copied()).fold(0.0, f32::max);
        Animation { name, duration, channels }
    }

    fn material(&mut self, m: &gltf::Material, index: usize) -> MaterialId {
        if let Some(&id) = self.materials.get(&index) {
            return id;
        }
        let pbr = m.pbr_metallic_roughness();
        let texture = pbr.base_color_texture().and_then(|info| {
            if info.tex_coord() != 0 {
                log::warn!("glTF 材质 {index} 用了第 {} 套 uv，只支持第 0 套", info.tex_coord());
            }
            self.texture(info.texture().source().index())
        });
        let alpha_cutoff = match m.alpha_mode() {
            AlphaMode::Opaque | AlphaMode::Blend => None,
            AlphaMode::Mask => Some(m.alpha_cutoff().unwrap_or(0.5)),
        };
        let id = self.assets.add_material(Material {
            base_color: Vec4::from(pbr.base_color_factor()),
            base_color_texture: texture,
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            emissive: Vec3::from(m.emissive_factor()),
            alpha_cutoff,
            double_sided: m.double_sided(),
            blend: matches!(m.alpha_mode(), AlphaMode::Blend),
        });
        self.materials.insert(index, id);
        id
    }

    /// 解码失败时返回 None（材质退回不带贴图）。
    fn texture(&mut self, image: usize) -> Option<ImageId> {
        if let Some(&id) = self.textures.get(&image) {
            return Some(id);
        }
        let decoded = self.images.get(image)?.clone()?;
        let id = Handle::new((self.first_image + self.new_images.len()) as u32);
        self.new_images.push(decoded);
        self.textures.insert(image, id);
        Some(id)
    }
}

/// 一个还在加载的 glTF 模型：加载完后系统把它换成 [`crate::Model3d`]，加载失败时去掉这个组件。
#[derive(bevy_ecs::prelude::Component, Debug, Clone, Copy)]
pub struct GltfModel(pub Handle<Gltf>);

/// 同一个 glTF 只展开一次，多个实体共用一个模型。
#[derive(bevy_ecs::prelude::Resource, Default)]
pub(crate) struct GltfModels(HashMap<Handle<Gltf>, Option<ModelId>>);

/// `PreUpdate`，收回加载结果之后。
pub(crate) fn resolve_gltf_models(world: &mut bevy_ecs::prelude::World) {
    use bevy_ecs::prelude::*;
    use nge_assets::LoadState;

    let mut waiting = world.query_filtered::<(Entity, &GltfModel), Without<crate::Model3d>>();
    let waiting: Vec<(Entity, Handle<Gltf>)> = waiting.iter(world).map(|(e, g)| (e, g.0)).collect();
    for (entity, handle) in waiting {
        let model = match world.resource::<GltfModels>().0.get(&handle) {
            Some(&model) => model,
            None => match world.resource::<Assets<Gltf>>().state(handle) {
                LoadState::Loading => continue,
                LoadState::Failed(_) => None,
                LoadState::Loaded => {
                    let model = world.resource_scope(|world, mut assets: Mut<Assets3d>| {
                        world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
                            let gltf = world.resource::<Assets<Gltf>>().get(handle).expect("刚确认过已加载");
                            assets.add_gltf(&mut images, gltf)
                        })
                    });
                    model.map_err(|e| log::warn!("glTF {handle:?} 展开失败: {e}")).ok()
                }
            },
        };
        world.resource_mut::<GltfModels>().0.insert(handle, model);
        let mut entity = world.entity_mut(entity);
        match model {
            Some(model) => entity.insert(crate::Model3d(model)),
            None => entity.remove::<GltfModel>(),
        };
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use glam::{Mat4, Quat, UVec2, Vec3, Vec4};

    use nge_assets::{Assets, Image, ImageId};

    use crate::assets::{Assets3d, MaterialId};

    const TEST_GLB: &[u8] = include_bytes!("../tests/data/test.glb");

    #[test]
    fn flattens_default_scene_with_hierarchy() {
        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        let model = assets.load_gltf(&mut images, TEST_GLB).unwrap();
        let parts = &assets.model(model).unwrap().parts;
        assert_eq!(parts.len(), 2, "非默认场景的节点和线段图元都不算");

        let quad = Mat4::from_translation(Vec3::X) * Mat4::from_scale_rotation_translation(Vec3::splat(2.0), Quat::from_rotation_y(FRAC_PI_2), Vec3::ZERO);
        assert!(parts[0].local.abs_diff_eq(quad, 1e-5), "{}", parts[0].local);
        assert!(parts[1].local.abs_diff_eq(Mat4::from_translation(Vec3::new(0.0, 3.0, 0.0)), 1e-6));
    }

    #[test]
    fn reads_geometry_and_fills_missing_attributes() {
        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        let model = assets.load_gltf(&mut images, TEST_GLB).unwrap();
        let parts = assets.model(model).unwrap().parts.clone();
        let quad = assets.mesh(parts[0].mesh).unwrap();
        assert_eq!((quad.vertices.len(), quad.indices.len()), (4, 6));
        assert_eq!(quad.vertices[0].uv, [0.0, 1.0]);
        let tri = assets.mesh(parts[1].mesh).unwrap();
        assert_eq!(tri.indices, [0, 1, 2], "没有索引时按顺序生成");
        assert!(tri.vertices.iter().all(|v| v.normal == [0.0, 0.0, 1.0]), "没有法线时按面法线计算");
        assert!(tri.vertices.iter().all(|v| v.uv == [0.0, 0.0]));
    }

    #[test]
    fn reads_materials_and_textures() {
        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        let model = assets.load_gltf(&mut images, TEST_GLB).unwrap();
        let parts = assets.model(model).unwrap().parts.clone();
        let textured = *assets.material(parts[0].material);
        assert_ne!(parts[0].material, MaterialId::DEFAULT);
        assert_eq!((textured.metallic, textured.roughness), (0.2, 0.7));
        assert_eq!((textured.alpha_cutoff, textured.double_sided), (Some(0.3), true));
        let image = textured.base_color_texture.unwrap();
        assert_ne!(image, ImageId::WHITE);
        assert_eq!(images.size(image), Some(UVec2::new(2, 2)));
        assert_eq!(&images.get(image).unwrap().rgba[..8], &[255, 0, 0, 255, 0, 255, 0, 255], "第一行：红、绿");

        let plain = *assets.material(parts[1].material);
        assert_eq!(plain.base_color, Vec4::new(0.5, 0.5, 0.5, 1.0), "glTF 的系数已经是线性值");
        assert_eq!(plain.emissive, Vec3::X);
        assert_eq!((plain.base_color_texture, plain.alpha_cutoff, plain.double_sided), (None, None, false));
    }

    /// 只有节点、没有网格的 .gltf（JSON）。
    fn nodes_only(scene: &str, nodes: &str) -> Vec<u8> {
        format!(r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":{scene}}}],"nodes":{nodes}}}"#).into_bytes()
    }

    #[test]
    fn rejects_cycles_and_shared_nodes() {
        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        let cycle = nodes_only("[0]", r#"[{"children":[1]},{"children":[0]}]"#);
        assert!(assets.load_gltf(&mut images, &cycle).unwrap_err().contains("节点"));
        let shared = nodes_only("[0,1]", r#"[{"children":[2]},{"children":[2]},{}]"#);
        assert!(assets.load_gltf(&mut images, &shared).unwrap_err().contains("节点"));
    }

    #[test]
    fn rejects_too_deep_trees() {
        let chain = (0..300).map(|i| if i < 299 { format!(r#"{{"children":[{}]}}"#, i + 1) } else { "{}".into() }).collect::<Vec<_>>().join(",");
        let deep = nodes_only("[0]", &format!("[{chain}]"));
        assert!(Assets3d::default().load_gltf(&mut Assets::<Image>::with_white(), &deep).unwrap_err().contains("层"));
    }

    #[test]
    fn failed_load_leaves_assets_untouched() {
        // 第一个图元正常（会先加进网格和材质），第二个只有两个索引
        const HALF_BAD: &str = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0},"material":0},{"attributes":{"POSITION":0},"indices":1}]}],"materials":[{}],"buffers":[{"byteLength":40,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAABAA=="}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":4}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]},{"bufferView":1,"componentType":5123,"count":2,"type":"SCALAR"}]}"#;
        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        let before = (images.len(), assets.meshes.len(), assets.materials.len(), assets.models.len());
        assert!(assets.load_gltf(&mut images, HALF_BAD.as_bytes()).unwrap_err().contains("索引"));
        assert_eq!((images.len(), assets.meshes.len(), assets.materials.len(), assets.models.len()), before);
    }

    #[test]
    fn primitives_without_material_use_gltf_defaults() {
        const NO_MATERIAL: &str = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],"buffers":[{"byteLength":36,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAA"}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}]}"#;
        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        let model = assets.load_gltf(&mut images, NO_MATERIAL.as_bytes()).unwrap();
        let part = assets.model(model).unwrap().parts[0].clone();
        let m = *assets.material(part.material);
        assert_eq!((m.base_color, m.metallic, m.roughness), (Vec4::ONE, 1.0, 1.0), "glTF 规范的默认材质");
    }

    #[test]
    fn blend_alpha_mode_reads_as_blend() {
        let gltf = |mode: &str| format!(r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"material":0}}]}}],"materials":[{{"alphaMode":"{mode}"}}],"buffers":[{{"byteLength":36,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAA"}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}}]}}"#);
        for (mode, blend, cutoff) in [("BLEND", true, None), ("OPAQUE", false, None), ("MASK", false, Some(0.5))] {
            let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
            let model = assets.load_gltf(&mut images, gltf(mode).as_bytes()).unwrap();
            let m = *assets.material(assets.model(model).unwrap().parts[0].material);
            assert_eq!((m.blend, m.alpha_cutoff), (blend, cutoff), "{mode}");
        }
    }

    #[test]
    fn rejects_garbage() {
        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        assert!(assets.load_gltf(&mut images, b"not a gltf").is_err());
        assert!(assets.load_gltf(&mut images, &TEST_GLB[..TEST_GLB.len() / 2]).is_err());
    }

    const EXTERNAL: &str = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0},"material":0}]}],"materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}],"textures":[{"source":0}],"images":[{"uri":"../tex/brick%201.png"}],"buffers":[{"byteLength":36,"uri":"mesh%20a.bin"}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}]}"#;
    const TRIANGLE_BIN: [u8; 36] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 63, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 63, 0, 0, 0, 0];

    fn png_2x1() -> Vec<u8> {
        let mut png = Vec::new();
        image::RgbaImage::from_raw(2, 1, vec![9, 8, 7, 255, 1, 2, 3, 255])
            .unwrap()
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png
    }

    #[test]
    fn external_files_resolve_through_the_reader() {
        let png = png_2x1();
        let asked = std::cell::RefCell::new(Vec::new());
        let gltf = super::Gltf::parse(EXTERNAL.as_bytes(), |uri| {
            asked.borrow_mut().push(uri.to_owned());
            match uri {
                "mesh a.bin" => Ok(TRIANGLE_BIN.to_vec()),
                "../tex/brick 1.png" => Ok(png.clone()),
                _ => Err(format!("{uri} 不存在")),
            }
        })
        .unwrap();
        assert_eq!(*asked.borrow(), ["mesh a.bin", "../tex/brick 1.png"], "百分号编码要先解开");
        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        let model = assets.add_gltf(&mut images, &gltf).unwrap();
        let part = assets.model(model).unwrap().parts[0].clone();
        assert_eq!(assets.mesh(part.mesh).unwrap().vertices.len(), 3);
        let texture = assets.material(part.material).base_color_texture.unwrap();
        assert_eq!(images.get(texture).unwrap().rgba, [9, 8, 7, 255, 1, 2, 3, 255]);
    }

    #[test]
    fn missing_external_buffer_fails_but_missing_image_only_drops_the_texture() {
        assert!(super::Gltf::parse(EXTERNAL.as_bytes(), |uri| Err(format!("{uri} 不存在"))).is_err());
        assert!(super::Gltf::parse(EXTERNAL.as_bytes(), |_| Ok(vec![0; 8])).unwrap_err().contains("只有 8 字节"));
        let gltf = super::Gltf::parse(EXTERNAL.as_bytes(), |uri| if uri.ends_with(".bin") { Ok(TRIANGLE_BIN.to_vec()) } else { Err("没有".into()) }).unwrap();
        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        let model = assets.add_gltf(&mut images, &gltf).unwrap();
        let part = assets.model(model).unwrap().parts[0].clone();
        assert_eq!(assets.material(part.material).base_color_texture, None);
        assert_eq!(images.len(), 1, "没有新贴图");
    }

    /// 内嵌数据的三角形；`extra` 插进 JSON 顶层。
    fn triangle_with(byte_length: usize, extra: &str) -> String {
        format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"material":0}}]}}],"materials":[{{"pbrMetallicRoughness":{{"baseColorTexture":{{"index":0}}}}}}],"textures":[{{"source":0}}],"buffers":[{{"byteLength":{byte_length},"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAA"}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}}],{extra}}}"#
        )
    }

    #[test]
    fn short_or_overrun_buffers_fail_without_panicking() {
        let views = r#""bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36}],"images":[{"uri":"x.png"}]"#;
        let short = triangle_with(100, views);
        assert!(super::Gltf::parse(short.as_bytes(), |_| Err("不读外部文件".into())).unwrap_err().contains("字节"));
        let overrun = triangle_with(
            36,
            r#""bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":20,"byteLength":100}],"images":[{"bufferView":1,"mimeType":"image/png"}]"#,
        );
        assert!(super::Gltf::parse(overrun.as_bytes(), |_| Err("不读外部文件".into())).unwrap_err().contains("越界"));
    }

    fn base64(bytes: &[u8]) -> String {
        const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        bytes
            .chunks(3)
            .flat_map(|c| {
                let n = u32::from(c[0]) << 16 | u32::from(*c.get(1).unwrap_or(&0)) << 8 | u32::from(*c.get(2).unwrap_or(&0));
                (0..4).map(move |i| if i <= c.len() { ABC[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' })
            })
            .collect()
    }

    #[test]
    fn data_uri_images_decode() {
        let images = format!(r#""bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}}],"images":[{{"uri":"data:image/png;base64,{}"}}]"#, base64(&png_2x1()));
        let gltf = super::Gltf::parse(triangle_with(36, &images).as_bytes(), |_| Err("不读外部文件".into())).unwrap();
        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        let model = assets.add_gltf(&mut images, &gltf).unwrap();
        let texture = assets.material(assets.model(model).unwrap().parts[0].material).base_color_texture.unwrap();
        assert_eq!(images.get(texture).unwrap().rgba, [9, 8, 7, 255, 1, 2, 3, 255]);
    }

    /// 两节骨头的三角形：glTF 节点 1 是根（子节点 2、0），2 是挂在上方 1 处的关节，0 是网格。
    /// 动画 wave：关节绕 z 线性转 90°，根按 STEP 平移。
    fn skinned_gltf() -> String {
        let mut bin: Vec<u8> = Vec::new();
        let f = |bin: &mut Vec<u8>, v: &[f32]| bin.extend(v.iter().flat_map(|x| x.to_le_bytes()));
        f(&mut bin, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 2.0, 0.0]);
        bin.extend([0u16, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0].iter().flat_map(|x| x.to_le_bytes()));
        f(&mut bin, &[1.0, 0.0, 0.0, 0.0, 2.0, 2.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        f(&mut bin, &Mat4::IDENTITY.to_cols_array());
        f(&mut bin, &Mat4::from_translation(Vec3::NEG_Y).to_cols_array());
        f(&mut bin, &[0.0, 1.0]);
        f(&mut bin, &Quat::IDENTITY.to_array());
        f(&mut bin, &Quat::from_rotation_z(FRAC_PI_2).to_array());
        f(&mut bin, &[0.0, 0.0, 0.0, 5.0, 0.0, 0.0]);
        assert_eq!(bin.len(), 300);
        let views = [(0, 36), (36, 24), (60, 48), (108, 128), (236, 8), (244, 32), (276, 24)]
            .map(|(o, l)| format!(r#"{{"buffer":0,"byteOffset":{o},"byteLength":{l}}}"#))
            .join(",");
        format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[1]}}],
            "nodes":[{{"mesh":0,"skin":0}},{{"children":[2,0]}},{{"translation":[0,1,0]}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"JOINTS_0":1,"WEIGHTS_0":2}}}}]}}],
            "skins":[{{"joints":[1,2],"inverseBindMatrices":3}}],
            "animations":[{{"name":"wave","channels":[{{"sampler":0,"target":{{"node":2,"path":"rotation"}}}},{{"sampler":1,"target":{{"node":1,"path":"translation"}}}}],
              "samplers":[{{"input":4,"output":5}},{{"input":4,"output":6,"interpolation":"STEP"}}]}},{{"channels":[],"samplers":[]}}],
            "buffers":[{{"byteLength":300,"uri":"data:application/octet-stream;base64,{}"}}],
            "bufferViews":[{views}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,2,0]}},
              {{"bufferView":1,"componentType":5123,"count":3,"type":"VEC4"}},
              {{"bufferView":2,"componentType":5126,"count":3,"type":"VEC4"}},
              {{"bufferView":3,"componentType":5126,"count":2,"type":"MAT4"}},
              {{"bufferView":4,"componentType":5126,"count":2,"type":"SCALAR","min":[0],"max":[1]}},
              {{"bufferView":5,"componentType":5126,"count":2,"type":"VEC4"}},
              {{"bufferView":6,"componentType":5126,"count":2,"type":"VEC3"}}]}}"#,
            base64(&bin)
        )
    }

    #[test]
    fn reads_skins_and_animations() {
        use crate::animation::{Interpolation, Property};

        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        let model = assets.load_gltf(&mut images, skinned_gltf().as_bytes()).unwrap();
        let model = assets.model(model).unwrap().clone();
        let parents: Vec<_> = model.skeleton.nodes.iter().map(|n| n.parent).collect();
        assert_eq!(parents, [None, Some(0), Some(0)], "按父在前展开：根、关节、网格");
        assert_eq!(model.skeleton.nodes[1].rest.translation, Vec3::Y);
        let skin = &model.skeleton.skins[0];
        assert_eq!(skin.joints, [0, 1], "关节换成展开后的下标");
        assert_eq!(skin.inverse_bind[1], Mat4::from_translation(Vec3::NEG_Y));
        assert_eq!((model.parts[0].node, model.parts[0].skin), (Some(2), Some(0)));
        let mesh = assets.mesh(model.parts[0].mesh).unwrap();
        assert_eq!(mesh.skin[1].joints, [0, 1, 0, 0]);
        assert_eq!(mesh.skin[1].weights, [0.5, 0.5, 0.0, 0.0], "权重归一化");

        let [wave, unnamed] = &model.animations[..] else { panic!("{:?}", model.animations) };
        assert_eq!((wave.name.as_str(), wave.duration, unnamed.name.as_str()), ("wave", 1.0, "1"));
        let rotate = &wave.channels[0];
        assert_eq!((rotate.node, rotate.property, rotate.interpolation), (1, Property::Rotation, Interpolation::Linear));
        assert_eq!(rotate.values[1], Vec4::from(Quat::from_rotation_z(FRAC_PI_2)));
        let shift = &wave.channels[1];
        assert_eq!((shift.node, shift.property, shift.interpolation), (0, Property::Translation, Interpolation::Step));
        assert_eq!(shift.values[1], Vec4::new(5.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn bad_channels_are_dropped_not_the_model() {
        // 平移通道改成三次样条：每帧要三个值，只有一个
        let json = skinned_gltf().replace(r#""interpolation":"STEP""#, r#""interpolation":"CUBICSPLINE""#);
        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        let model = assets.load_gltf(&mut images, json.as_bytes()).unwrap();
        let channels = &assets.model(model).unwrap().animations[0].channels;
        assert_eq!(channels.len(), 1, "只丢坏的那条");
        assert_eq!(channels[0].property, crate::animation::Property::Rotation);
    }

    #[test]
    fn skins_with_joints_outside_the_scene_are_dropped() {
        // 关节 2 不在场景里（场景只有节点 0）
        let json = skinned_gltf().replace(r#""scenes":[{"nodes":[1]}]"#, r#""scenes":[{"nodes":[0]}]"#);
        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        let model = assets.load_gltf(&mut images, json.as_bytes()).unwrap();
        let model = assets.model(model).unwrap();
        assert_eq!(model.parts[0].skin, None, "按不蒙皮画");
        assert!(model.animations[0].channels.is_empty(), "目标不在场景里的通道跳过");
    }

    #[test]
    fn direct_bytes_refuse_external_files() {
        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        assert!(assets.load_gltf(&mut images, EXTERNAL.as_bytes()).unwrap_err().contains("AssetServer"));
    }
}
