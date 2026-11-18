//! 基础 3D：网格、深度缓冲、透视相机、简化 PBR、方向光 / 点光源 / 聚光灯、阴影（含级联）、glTF（含骨骼动画）、调试画线。

mod animation;
mod animator;
mod assets;
mod billboard_gpu;
mod billboards;
mod camera;
mod collect;
mod fog;
mod gizmos;
mod gltf_load;
mod gltf_morph;
mod graph_pose;
mod ibl;
mod light;
mod line_gpu;
mod mesh;
mod morph;
mod renderer;
mod shadow;
mod shadow_soft;
mod shadow_views;
mod sky;
mod sky_gpu;

pub use animation::{Animation, Channel, Interpolation, Node, Property, Skeleton, Skin, Trs};
pub use animator::{Animator, Fade, Pose};
pub use billboards::{Billboard, Billboards, Blend, FULL_UV, Facing, RibbonPoint};
pub use assets::{Assets3d, Material, MaterialId, MeshId, Model, ModelId, Part};
pub use nge_assets::{AssetPlugin, AssetServer, AssetSource, Assets, Image, ImageId, MAX_TEXTURE_SIDE, MemorySource};
pub use camera::{Camera3d, Transform3d};
pub use gizmos::{CIRCLE_SEGMENTS, Gizmos3d, Lines};
pub use gltf_load::{Gltf, GltfModel, gltf_model};
pub use graph_pose::mask;
pub use light::{AmbientLight, DirectionalLight, MAX_DIRECTIONAL_LIGHTS, MAX_POINT_LIGHTS, MAX_SPOT_LIGHTS, PointLight, SpotLight};
pub use mesh::{Influence, MAX_MORPH_TARGETS, Mesh, MorphTarget, Vertex};
pub use morph::MorphWeights;
pub use renderer::{Mesh3d, Model3d, Render3dPlugin};
pub use fog::{Fog, FogMode, HeightFog};
pub use nge_assets::CubeImage;
pub use shadow::{ShadowFilter, Shadows};
pub use shadow_soft::ShadowPriority;
pub use sky::{Sky, SkyGradient, SkySource};
pub use sky_gpu::SkyBake;

pub mod prelude {
    pub use crate::{
        AmbientLight, Animator, AssetServer, Assets, Assets3d, Camera3d, GltfModel, Image, DirectionalLight, Material, Mesh, Mesh3d, Model3d, PointLight, Render3dPlugin, SpotLight, Transform3d,
    };
    pub use glam::{Quat, Vec3};
}
