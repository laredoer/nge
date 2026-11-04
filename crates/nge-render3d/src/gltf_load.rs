//! glTF 2.0 加载：`.glb` 或内嵌 data URI 的 `.gltf`，展开默认场景的节点树成一个 [`Model`]。

use std::collections::HashMap;

use glam::{Mat4, UVec2, Vec2, Vec3, Vec4};
use gltf::image::Format;
use gltf::material::AlphaMode;
use gltf::mesh::Mode;

use crate::assets::{Assets3d, ImageId, Material, MaterialId, MeshId, Model, ModelId, Part};
use crate::mesh::{Mesh, Vertex};

/// 节点树的最大深度，防止病态文件把栈撑爆。
const MAX_DEPTH: usize = 256;

impl Assets3d {
    /// 只读三角形图元、base color 贴图和材质系数（见 `docs/m4-3d.md`），其余忽略并记 warn。
    /// 失败时已经加进来的贴图、网格和材质都会撤掉。
    pub fn load_gltf(&mut self, bytes: &[u8]) -> Result<ModelId, String> {
        let marks = (self.images.len(), self.meshes.len(), self.materials.len());
        let result = self.load_gltf_inner(bytes);
        if result.is_err() {
            self.images.truncate(marks.0);
            self.meshes.truncate(marks.1);
            self.materials.truncate(marks.2);
        }
        result
    }

    fn load_gltf_inner(&mut self, bytes: &[u8]) -> Result<ModelId, String> {
        let (doc, buffers, images) = gltf::import_slice(bytes).map_err(|e| format!("glTF 解析失败: {e}"))?;
        let scene = doc.default_scene().or_else(|| doc.scenes().next()).ok_or("glTF 里没有场景")?;
        let mut loader = Loader {
            assets: self,
            buffers: &buffers,
            images: &images,
            visited: vec![false; doc.nodes().len()],
            meshes: HashMap::new(),
            materials: HashMap::new(),
            default_material: None,
            textures: HashMap::new(),
            parts: Vec::new(),
        };
        for node in scene.nodes() {
            loader.node(&node, Mat4::IDENTITY, 0)?;
        }
        let parts = loader.parts;
        self.add_model(Model { parts })
    }
}

struct Loader<'a> {
    assets: &'a mut Assets3d,
    buffers: &'a [gltf::buffer::Data],
    images: &'a [gltf::image::Data],
    /// glTF 要求节点构成树：一个节点只能出现一次，出现第二次就是成环或被多个父节点共用。
    visited: Vec<bool>,
    /// (glTF 网格, 图元) → 网格；同一个网格被多个节点引用时只存一份。
    meshes: HashMap<(usize, usize), MeshId>,
    materials: HashMap<usize, MaterialId>,
    /// 没指定材质的图元用 glTF 规范的默认材质（白色、metallic 1、roughness 1）。
    default_material: Option<MaterialId>,
    textures: HashMap<usize, ImageId>,
    parts: Vec<Part>,
}

impl Loader<'_> {
    fn node(&mut self, node: &gltf::Node, parent: Mat4, depth: usize) -> Result<(), String> {
        if depth >= MAX_DEPTH {
            return Err(format!("glTF 节点层级超过 {MAX_DEPTH} 层"));
        }
        if std::mem::replace(&mut self.visited[node.index()], true) {
            return Err(format!("glTF 节点 {} 出现了不止一次（成环或被多个父节点共用）", node.index()));
        }
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        if let Some(mesh) = node.mesh() {
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
                self.parts.push(Part { mesh: mesh_id, material, local: world });
            }
        }
        for child in node.children() {
            self.node(&child, world, depth + 1)?;
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
        let vertices = positions
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                let n = normals.as_ref().and_then(|n| n.get(i)).copied().unwrap_or(Vec3::ZERO);
                Vertex::new(p, n, uvs.get(i).copied().unwrap_or(Vec2::ZERO))
            })
            .collect();
        let mut mesh = Mesh { vertices, indices };
        if normals.is_none() {
            mesh.compute_normals();
        }
        Ok(mesh)
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
            AlphaMode::Opaque => None,
            AlphaMode::Mask => Some(m.alpha_cutoff().unwrap_or(0.5)),
            AlphaMode::Blend => {
                log::warn!("glTF 材质 {index} 是 BLEND 模式，暂按不透明处理");
                None
            }
        };
        let id = self.assets.add_material(Material {
            base_color: Vec4::from(pbr.base_color_factor()),
            base_color_texture: texture,
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            emissive: Vec3::from(m.emissive_factor()),
            alpha_cutoff,
            double_sided: m.double_sided(),
        });
        self.materials.insert(index, id);
        id
    }

    /// 解码失败或格式不支持时返回 None（材质退回不带贴图）。
    fn texture(&mut self, image: usize) -> Option<ImageId> {
        if let Some(&id) = self.textures.get(&image) {
            return Some(id);
        }
        let data = self.images.get(image)?;
        let rgba = match data.format {
            Format::R8G8B8A8 => data.pixels.clone(),
            Format::R8G8B8 => data.pixels.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            Format::R8G8 => data.pixels.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
            Format::R8 => data.pixels.iter().flat_map(|&p| [p, p, p, 255]).collect(),
            other => {
                log::warn!("glTF 贴图 {image} 的格式 {other:?} 不支持，忽略");
                return None;
            }
        };
        match self.assets.add_rgba(UVec2::new(data.width, data.height), rgba) {
            Ok(id) => {
                self.textures.insert(image, id);
                Some(id)
            }
            Err(e) => {
                log::warn!("glTF 贴图 {image}: {e}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use glam::{Mat4, Quat, UVec2, Vec3, Vec4};

    use crate::assets::{Assets3d, ImageId, MaterialId};

    const TEST_GLB: &[u8] = include_bytes!("../tests/data/test.glb");

    #[test]
    fn flattens_default_scene_with_hierarchy() {
        let mut assets = Assets3d::default();
        let model = assets.load_gltf(TEST_GLB).unwrap();
        let parts = &assets.model(model).unwrap().parts;
        assert_eq!(parts.len(), 2, "非默认场景的节点和线段图元都不算");

        let quad = Mat4::from_translation(Vec3::X) * Mat4::from_scale_rotation_translation(Vec3::splat(2.0), Quat::from_rotation_y(FRAC_PI_2), Vec3::ZERO);
        assert!(parts[0].local.abs_diff_eq(quad, 1e-5), "{}", parts[0].local);
        assert!(parts[1].local.abs_diff_eq(Mat4::from_translation(Vec3::new(0.0, 3.0, 0.0)), 1e-6));
    }

    #[test]
    fn reads_geometry_and_fills_missing_attributes() {
        let mut assets = Assets3d::default();
        let model = assets.load_gltf(TEST_GLB).unwrap();
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
        let mut assets = Assets3d::default();
        let model = assets.load_gltf(TEST_GLB).unwrap();
        let parts = assets.model(model).unwrap().parts.clone();
        let textured = *assets.material(parts[0].material);
        assert_ne!(parts[0].material, MaterialId::DEFAULT);
        assert_eq!((textured.metallic, textured.roughness), (0.2, 0.7));
        assert_eq!((textured.alpha_cutoff, textured.double_sided), (Some(0.3), true));
        let image = textured.base_color_texture.unwrap();
        assert_ne!(image, ImageId::WHITE);
        assert_eq!(assets.image_size(image), Some(UVec2::new(2, 2)));
        assert_eq!(&assets.images[image.0 as usize].rgba[..8], &[255, 0, 0, 255, 0, 255, 0, 255], "第一行：红、绿");

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
        let mut assets = Assets3d::default();
        let cycle = nodes_only("[0]", r#"[{"children":[1]},{"children":[0]}]"#);
        assert!(assets.load_gltf(&cycle).unwrap_err().contains("节点"));
        let shared = nodes_only("[0,1]", r#"[{"children":[2]},{"children":[2]},{}]"#);
        assert!(assets.load_gltf(&shared).unwrap_err().contains("节点"));
    }

    #[test]
    fn rejects_too_deep_trees() {
        let chain = (0..300).map(|i| if i < 299 { format!(r#"{{"children":[{}]}}"#, i + 1) } else { "{}".into() }).collect::<Vec<_>>().join(",");
        let deep = nodes_only("[0]", &format!("[{chain}]"));
        assert!(Assets3d::default().load_gltf(&deep).unwrap_err().contains("层"));
    }

    #[test]
    fn failed_load_leaves_assets_untouched() {
        // 第一个图元正常（会先加进网格和材质），第二个只有两个索引
        const HALF_BAD: &str = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0},"material":0},{"attributes":{"POSITION":0},"indices":1}]}],"materials":[{}],"buffers":[{"byteLength":40,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAABAA=="}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":4}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]},{"bufferView":1,"componentType":5123,"count":2,"type":"SCALAR"}]}"#;
        let mut assets = Assets3d::default();
        let before = (assets.images.len(), assets.meshes.len(), assets.materials.len(), assets.models.len());
        assert!(assets.load_gltf(HALF_BAD.as_bytes()).unwrap_err().contains("索引"));
        assert_eq!((assets.images.len(), assets.meshes.len(), assets.materials.len(), assets.models.len()), before);
    }

    #[test]
    fn primitives_without_material_use_gltf_defaults() {
        const NO_MATERIAL: &str = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],"buffers":[{"byteLength":36,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAA"}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}]}"#;
        let mut assets = Assets3d::default();
        let model = assets.load_gltf(NO_MATERIAL.as_bytes()).unwrap();
        let part = assets.model(model).unwrap().parts[0];
        let m = *assets.material(part.material);
        assert_eq!((m.base_color, m.metallic, m.roughness), (Vec4::ONE, 1.0, 1.0), "glTF 规范的默认材质");
    }

    #[test]
    fn rejects_garbage() {
        let mut assets = Assets3d::default();
        assert!(assets.load_gltf(b"not a gltf").is_err());
        assert!(assets.load_gltf(&TEST_GLB[..TEST_GLB.len() / 2]).is_err());
    }
}
