//! 3D 资源：网格、材质、模型。只增不减，GPU 侧按需上传。贴图在 `nge_assets` 的 `Assets<Image>` 里（和 2D 共用）。

use bevy_ecs::prelude::Resource;
use glam::{Mat4, Vec3, Vec4};
use nge_assets::ImageId;

use crate::animation::{Animation, Skeleton};
use crate::mesh::{MAX_MORPH_TARGETS, Mesh};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeshId(pub(crate) u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MaterialId(pub(crate) u32);

impl MaterialId {
    /// 内置的默认材质（白色、非金属、粗糙度 0.5）。
    pub const DEFAULT: Self = Self(0);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelId(pub(crate) u32);

/// 3D 材质着色风格。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum MaterialStyle {
    /// 标准 Cook-Torrance PBR。
    #[default]
    Pbr,
    /// 卡通分阶着色 + 边缘光。
    Toon,
    /// 动态水波法线 + 菲涅尔高光。
    Water,
    /// 风吹顶点摆动 + 包裹漫反射与透光。
    Foliage,
}

impl MaterialStyle {
    pub(crate) fn as_u32(self) -> u32 {
        match self {
            Self::Pbr => 0,
            Self::Toon => 1,
            Self::Water => 2,
            Self::Foliage => 3,
        }
    }
}

/// 挂在模型或网格实体上，覆盖其材质的着色风格（不修改共享的 [`Material`] 资源）。
#[derive(bevy_ecs::prelude::Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MaterialStyleOverride(pub MaterialStyle);

/// glTF metallic-roughness 材质（完整 PBR 贴图 + 可选风格化着色）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// 线性 RGBA，和贴图相乘。
    pub base_color: Vec4,
    /// sRGB 基础色贴图。
    pub base_color_texture: Option<ImageId>,
    /// 线性切线空间法线贴图。
    pub normal_texture: Option<ImageId>,
    /// 法线贴图强度缩放（glTF 默认 1.0）。
    pub normal_scale: f32,
    pub metallic: f32,
    pub roughness: f32,
    /// 线性金属度/粗糙度贴图（glTF 规范：G 通道 = 粗糙度，B 通道 = 金属度）。
    pub metallic_roughness_texture: Option<ImageId>,
    /// 线性环境光遮蔽贴图（R 通道）。
    pub occlusion_texture: Option<ImageId>,
    /// 环境光遮蔽强度（glTF 默认 1.0）。
    pub occlusion_strength: f32,
    /// 线性 RGB，直接加到结果上。
    pub emissive: Vec3,
    /// sRGB 自发光贴图，与 `emissive` 相乘。
    pub emissive_texture: Option<ImageId>,
    /// 着色风格（默认标准 PBR）。
    pub style: MaterialStyle,
    /// alpha 低于它的像素丢弃（glTF 的 MASK）；`None` 表示不透明。
    pub alpha_cutoff: Option<f32>,
    /// 关掉背面剔除。
    pub double_sided: bool,
    /// 半透明（glTF 的 BLEND）：按 alpha 和后面的混合，不写深度、不投影子。
    pub blend: bool,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: Vec4::ONE,
            base_color_texture: None,
            normal_texture: None,
            normal_scale: 1.0,
            metallic: 0.0,
            roughness: 0.5,
            metallic_roughness_texture: None,
            occlusion_texture: None,
            occlusion_strength: 1.0,
            emissive: Vec3::ZERO,
            emissive_texture: None,
            style: MaterialStyle::Pbr,
            alpha_cutoff: None,
            double_sided: false,
            blend: false,
        }
    }
}

/// 模型里的一个部件：`local` 是不播动画时相对模型原点的矩阵（已乘上节点层级）。
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub mesh: MeshId,
    pub material: MaterialId,
    pub local: Mat4,
    /// 挂在节点树（[`Model::skeleton`]）的哪个节点上：播动画时按这个节点这一帧的矩阵画。
    pub node: Option<u32>,
    /// 用哪套蒙皮：网格带关节权重时按关节变形，`local` 和 `node` 不再起作用（glTF 的规定）。
    pub skin: Option<u32>,
    /// 变形目标的默认权重（glTF 节点或网格的 `weights`），没在播动画时用；少的按 0 算。
    pub weights: Vec<f32>,
}

impl Part {
    pub fn new(mesh: MeshId, material: MaterialId, local: Mat4) -> Self {
        Self { mesh, material, local, node: None, skin: None, weights: Vec::new() }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    pub parts: Vec<Part>,
    /// 节点树和蒙皮；没有动画、蒙皮的模型可以留空。
    pub skeleton: Skeleton,
    pub animations: Vec<Animation>,
}

#[derive(Resource)]
pub struct Assets3d {
    pub(crate) meshes: Vec<Mesh>,
    pub(crate) materials: Vec<Material>,
    pub(crate) models: Vec<Model>,
}

impl Default for Assets3d {
    fn default() -> Self {
        Self {
            meshes: Vec::new(),
            materials: vec![Material::default()],
            models: Vec::new(),
        }
    }
}

impl Assets3d {
    /// 空网格、索引不是三的倍数或越界、变形目标的长度和顶点数对不上时返回错误。变形目标超过 [`MAX_MORPH_TARGETS`] 个的丢掉多的（warn）。
    pub fn add_mesh(&mut self, mut mesh: Mesh) -> Result<MeshId, String> {
        if !mesh.skin.is_empty() && mesh.skin.len() != mesh.vertices.len() {
            return Err(format!("网格有 {} 个顶点，关节权重却有 {} 份", mesh.vertices.len(), mesh.skin.len()));
        }
        let n = mesh.vertices.len();
        for (i, t) in mesh.targets.iter().enumerate() {
            if t.positions.len() != n || !(t.normals.is_empty() || t.normals.len() == n) {
                return Err(format!("网格有 {n} 个顶点，变形目标 {i} 却有 {} 个位置差、{} 个法线差", t.positions.len(), t.normals.len()));
            }
        }
        if mesh.targets.len() > MAX_MORPH_TARGETS {
            log::warn!("网格有 {} 个变形目标，只用前 {MAX_MORPH_TARGETS} 个", mesh.targets.len());
            mesh.targets.truncate(MAX_MORPH_TARGETS);
        }
        if mesh.indices.is_empty() || mesh.indices.len() % 3 != 0 {
            return Err(format!("网格有 {} 个索引，要是三的倍数且不为空", mesh.indices.len()));
        }
        if let Some(&bad) = mesh.indices.iter().find(|&&i| i as usize >= mesh.vertices.len()) {
            return Err(format!("索引 {bad} 越界（只有 {} 个顶点）", mesh.vertices.len()));
        }
        self.meshes.push(mesh);
        Ok(MeshId(self.meshes.len() as u32 - 1))
    }

    pub fn add_material(&mut self, material: Material) -> MaterialId {
        self.materials.push(material);
        MaterialId(self.materials.len() as u32 - 1)
    }

    /// 引用了不存在的网格、节点、蒙皮，或者节点树、动画、关节编号不对时返回错误；不存在的材质画的时候退回默认材质。
    pub fn add_model(&mut self, model: Model) -> Result<ModelId, String> {
        model.skeleton.validate(&model.animations)?;
        for part in &model.parts {
            let mesh = self.mesh(part.mesh).ok_or_else(|| format!("模型引用了不存在的网格 {:?}", part.mesh))?;
            if part.node.is_some_and(|n| n as usize >= model.skeleton.nodes.len()) {
                return Err(format!("部件挂在不存在的节点 {:?} 上", part.node));
            }
            if let Some(skin) = part.skin {
                let joints = model.skeleton.skins.get(skin as usize).ok_or_else(|| format!("部件用了不存在的蒙皮 {skin}"))?.joints.len();
                if let Some(bad) = mesh.skin.iter().flat_map(|i| i.joints).find(|&j| j as usize >= joints) {
                    return Err(format!("网格的关节编号 {bad} 越界（蒙皮只有 {joints} 个关节）"));
                }
            }
        }
        self.models.push(model);
        Ok(ModelId(self.models.len() as u32 - 1))
    }

    pub fn mesh(&self, id: MeshId) -> Option<&Mesh> {
        self.meshes.get(id.0 as usize)
    }

    /// 不存在的材质返回默认材质。
    pub fn material(&self, id: MaterialId) -> &Material {
        self.materials.get(id.0 as usize).unwrap_or(&self.materials[0])
    }

    /// 改材质马上生效（每帧都按材质重新填 uniform）。
    pub fn material_mut(&mut self, id: MaterialId) -> Option<&mut Material> {
        self.materials.get_mut(id.0 as usize)
    }

    pub fn model(&self, id: ModelId) -> Option<&Model> {
        self.models.get(id.0 as usize)
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::*;
    use crate::mesh::Vertex;

    fn triangle() -> Mesh {
        Mesh { vertices: vec![Vertex::new(Vec3::ZERO, Vec3::Z, Vec2::ZERO); 3], indices: vec![0, 1, 2], ..Default::default() }
    }

    #[test]
    fn builtins() {
        let assets = Assets3d::default();
        assert_eq!(*assets.material(MaterialId::DEFAULT), Material::default());
        assert_eq!(*assets.material(MaterialId(99)), Material::default(), "不存在的材质退回默认");
    }

    #[test]
    fn meshes_are_validated() {
        let mut assets = Assets3d::default();
        assert!(assets.add_mesh(Mesh::default()).is_err());
        assert!(assets.add_mesh(Mesh { indices: vec![0, 1], ..triangle() }).is_err());
        assert!(assets.add_mesh(Mesh { indices: vec![0, 1, 3], ..triangle() }).is_err());
        let id = assets.add_mesh(triangle()).unwrap();
        assert_eq!(assets.mesh(id), Some(&triangle()));
    }

    #[test]
    fn materials_and_models() {
        let mut assets = Assets3d::default();
        let red = assets.add_material(Material { base_color: Vec4::new(1.0, 0.0, 0.0, 1.0), ..Default::default() });
        assert_ne!(red, MaterialId::DEFAULT);
        assets.material_mut(red).unwrap().metallic = 1.0;
        assert_eq!(assets.material(red).metallic, 1.0);

        let mesh = assets.add_mesh(triangle()).unwrap();
        let part = Part::new(mesh, red, Mat4::IDENTITY);
        let model = assets.add_model(Model { parts: vec![part.clone()], ..Default::default() }).unwrap();
        assert_eq!(assets.model(model).unwrap().parts, [part.clone()]);
        assert!(assets.add_model(Model { parts: vec![Part { mesh: MeshId(42), ..part }], ..Default::default() }).is_err());
    }

    #[test]
    fn skinned_models_are_validated() {
        use crate::animation::{Node, Skeleton, Skin, Trs};
        use crate::mesh::Influence;

        let mut assets = Assets3d::default();
        let influence = |j: u16| Influence { joints: [j, 0, 0, 0], weights: [1.0, 0.0, 0.0, 0.0] };
        assert!(assets.add_mesh(Mesh { skin: vec![influence(0)], ..triangle() }).is_err(), "权重要和顶点一样多");
        let mesh = assets.add_mesh(Mesh { skin: vec![influence(0), influence(0), influence(1)], ..triangle() }).unwrap();
        let skeleton = Skeleton {
            nodes: vec![Node { parent: None, rest: Trs::default() }; 2],
            skins: vec![Skin { joints: vec![0, 1], inverse_bind: vec![Mat4::IDENTITY; 2] }],
            names: vec![],
        };
        let part = Part { node: Some(1), skin: Some(0), ..Part::new(mesh, MaterialId::DEFAULT, Mat4::IDENTITY) };
        let model = |part: Part, skeleton: &Skeleton| Model { parts: vec![part], skeleton: skeleton.clone(), animations: vec![] };
        assert!(assets.add_model(model(part.clone(), &skeleton)).is_ok());
        assert!(assets.add_model(model(Part { node: Some(2), ..part.clone() }, &skeleton)).is_err(), "节点不存在");
        assert!(assets.add_model(model(Part { skin: Some(1), ..part.clone() }, &skeleton)).is_err(), "蒙皮不存在");
        let one_joint = Skeleton { skins: vec![Skin { joints: vec![0], inverse_bind: vec![Mat4::IDENTITY] }], ..skeleton.clone() };
        assert!(assets.add_model(model(part, &one_joint)).unwrap_err().contains("关节编号"));
        let bad_tree = Skeleton { nodes: vec![Node { parent: Some(0), rest: Trs::default() }], skins: vec![], names: vec![] };
        assert!(assets.add_model(model(Part::new(mesh, MaterialId::DEFAULT, Mat4::IDENTITY), &bad_tree)).is_err(), "节点树也要校验");
    }

    #[test]
    fn morph_targets_are_validated_and_capped() {
        use crate::mesh::MorphTarget;

        let mut assets = Assets3d::default();
        let target = |positions: usize, normals: usize| MorphTarget { positions: vec![Vec3::Y; positions], normals: vec![Vec3::X; normals] };
        assert!(assets.add_mesh(Mesh { targets: vec![target(3, 0)], ..triangle() }).is_ok(), "法线差可以为空");
        assert!(assets.add_mesh(Mesh { targets: vec![target(3, 3)], ..triangle() }).is_ok());
        assert!(assets.add_mesh(Mesh { targets: vec![target(3, 0), target(2, 0)], ..triangle() }).unwrap_err().contains("变形目标 1"));
        assert!(assets.add_mesh(Mesh { targets: vec![target(3, 1)], ..triangle() }).is_err(), "法线差要么空要么和顶点一样多");
        let many = assets.add_mesh(Mesh { targets: vec![target(3, 0); MAX_MORPH_TARGETS + 1], ..triangle() }).unwrap();
        assert_eq!(assets.mesh(many).unwrap().targets.len(), MAX_MORPH_TARGETS, "多的丢掉");
    }
}
