//! 3D 资源：网格、材质、模型。只增不减，GPU 侧按需上传。贴图在 `nge_assets` 的 `Assets<Image>` 里（和 2D 共用）。

use bevy_ecs::prelude::Resource;
use glam::{Mat4, Vec3, Vec4};
use nge_assets::ImageId;

use crate::mesh::Mesh;

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

/// glTF metallic-roughness 材质的子集。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// 线性 RGBA，和贴图相乘。
    pub base_color: Vec4,
    /// sRGB 贴图。
    pub base_color_texture: Option<ImageId>,
    pub metallic: f32,
    pub roughness: f32,
    /// 线性 RGB，直接加到结果上。
    pub emissive: Vec3,
    /// alpha 低于它的像素丢弃（glTF 的 MASK）；`None` 表示不透明。
    pub alpha_cutoff: Option<f32>,
    /// 关掉背面剔除。
    pub double_sided: bool,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: Vec4::ONE,
            base_color_texture: None,
            metallic: 0.0,
            roughness: 0.5,
            emissive: Vec3::ZERO,
            alpha_cutoff: None,
            double_sided: false,
        }
    }
}

/// 模型里的一个部件：`local` 是相对模型原点的矩阵（已乘上节点层级）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Part {
    pub mesh: MeshId,
    pub material: MaterialId,
    pub local: Mat4,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    pub parts: Vec<Part>,
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
    /// 空网格、索引不是三的倍数或越界时返回错误。
    pub fn add_mesh(&mut self, mesh: Mesh) -> Result<MeshId, String> {
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

    /// 引用了不存在的网格时返回错误；不存在的材质画的时候退回默认材质。
    pub fn add_model(&mut self, model: Model) -> Result<ModelId, String> {
        if let Some(part) = model.parts.iter().find(|p| self.mesh(p.mesh).is_none()) {
            return Err(format!("模型引用了不存在的网格 {:?}", part.mesh));
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
        Mesh { vertices: vec![Vertex::new(Vec3::ZERO, Vec3::Z, Vec2::ZERO); 3], indices: vec![0, 1, 2] }
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
        let part = Part { mesh, material: red, local: Mat4::IDENTITY };
        let model = assets.add_model(Model { parts: vec![part] }).unwrap();
        assert_eq!(assets.model(model).unwrap().parts, [part]);
        assert!(assets.add_model(Model { parts: vec![Part { mesh: MeshId(42), ..part }] }).is_err());
    }
}
