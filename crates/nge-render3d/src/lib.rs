//! 基础 3D：网格、深度缓冲、透视相机、简化 PBR、方向光 / 点光源、glTF。

mod assets;
mod camera;
mod gltf_load;
mod light;
mod mesh;
mod renderer;

pub use assets::{Assets3d, ImageId, MAX_TEXTURE_SIDE, Material, MaterialId, MeshId, Model, ModelId, Part};
pub use camera::{Camera3d, Transform3d};
pub use light::{AmbientLight, DirectionalLight, MAX_DIRECTIONAL_LIGHTS, MAX_POINT_LIGHTS, PointLight};
pub use mesh::{Mesh, Vertex};
pub use renderer::{Mesh3d, Model3d, Render3dPlugin};

pub mod prelude {
    pub use crate::{
        AmbientLight, Assets3d, Camera3d, DirectionalLight, Material, Mesh, Mesh3d, Model3d, PointLight, Render3dPlugin, Transform3d,
    };
    pub use glam::{Quat, Vec3};
}
