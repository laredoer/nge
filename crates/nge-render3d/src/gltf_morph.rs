//! glTF 的变形：动画的 `weights` 通道拆段。目标和默认权重在 gltf_load 里跟着图元、节点读。

use glam::Vec4;

use crate::mesh::MAX_MORPH_TARGETS;

/// 把 `weights` 通道的输出（每个关键帧 `per_key` 组、每组一个目标一个权重）按每 4 个目标拆成一段，
/// 返回（段, 这段的值）；值的排法和平移、旋转的一样（三次样条每帧入切线、值、出切线三个）。超过 [`MAX_MORPH_TARGETS`] 的段丢掉。
pub(crate) fn split_weights(flat: &[f32], keys: usize, per_key: usize) -> Result<Vec<(u8, Vec<Vec4>)>, String> {
    let groups = keys * per_key;
    if groups == 0 || flat.is_empty() || flat.len() % groups != 0 {
        return Err(format!("{keys} 个关键帧配了 {} 个权重", flat.len()));
    }
    let targets = flat.len() / groups;
    if targets > MAX_MORPH_TARGETS {
        log::warn!("变形权重通道有 {targets} 个目标，只用前 {MAX_MORPH_TARGETS} 个");
    }
    let segments = targets.min(MAX_MORPH_TARGETS).div_ceil(4);
    Ok((0..segments)
        .map(|s| {
            let values = flat
                .chunks_exact(targets)
                .map(|group| {
                    let mut v = [0.0; 4];
                    for (i, w) in v.iter_mut().enumerate() {
                        *w = group.get(s * 4 + i).copied().filter(|_| s * 4 + i < targets).unwrap_or(0.0);
                    }
                    Vec4::from(v)
                })
                .collect();
            (s as u8, values)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use glam::{Vec3, Vec4};
    use nge_assets::{Assets, Image};

    use super::*;
    use crate::animation::{Interpolation, Property};
    use crate::assets::Assets3d;

    #[test]
    fn linear_weights_split_every_four_targets() {
        // 6 个目标、2 个关键帧
        let flat: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let split = split_weights(&flat, 2, 1).unwrap();
        assert_eq!(split.len(), 2);
        assert_eq!(split[0], (0, vec![Vec4::new(0.0, 1.0, 2.0, 3.0), Vec4::new(6.0, 7.0, 8.0, 9.0)]));
        assert_eq!(split[1], (1, vec![Vec4::new(4.0, 5.0, 0.0, 0.0), Vec4::new(10.0, 11.0, 0.0, 0.0)]), "不满 4 个补 0");
    }

    #[test]
    fn cubic_weights_keep_tangents_in_order() {
        // 5 个目标、2 个关键帧：每帧入切线 5 个、值 5 个、出切线 5 个
        let flat: Vec<f32> = (0..30).map(|i| i as f32).collect();
        let split = split_weights(&flat, 2, 3).unwrap();
        let first: Vec<f32> = split[0].1.iter().map(|v| v.x).collect();
        assert_eq!(first, [0.0, 5.0, 10.0, 15.0, 20.0, 25.0], "第一帧的入切线、值、出切线，再第二帧的");
        let fifth: Vec<f32> = split[1].1.iter().map(|v| v.x).collect();
        assert_eq!(fifth, [4.0, 9.0, 14.0, 19.0, 24.0, 29.0], "第 5 个目标在第 1 段");
        assert!(split[1].1.iter().all(|v| v.y == 0.0));
    }

    #[test]
    fn extra_targets_and_bad_lengths() {
        let split = split_weights(&[1.0; 10], 1, 1).unwrap();
        assert_eq!(split.iter().map(|s| s.0).collect::<Vec<_>>(), [0, 1], "第 9、10 个目标丢掉");
        assert!(split_weights(&[1.0; 7], 2, 1).is_err());
        assert!(split_weights(&[], 2, 1).is_err());
        assert!(split_weights(&[1.0; 4], 0, 1).is_err());
    }

    const MORPH_GLB: &[u8] = include_bytes!("../tests/data/morph.glb");

    #[test]
    fn reads_targets_default_weights_and_weight_channels() {
        let (mut assets, mut images) = (Assets3d::default(), Assets::<Image>::with_white());
        let model = assets.load_gltf(&mut images, MORPH_GLB).unwrap();
        let model = assets.model(model).unwrap().clone();
        assert_eq!(model.parts.len(), 2);
        assert_eq!(model.parts[0].mesh, model.parts[1].mesh, "同一个网格只存一份");
        let mesh = assets.mesh(model.parts[0].mesh).unwrap();
        assert_eq!(mesh.targets.len(), 2);
        assert_eq!(mesh.targets[0].positions, [Vec3::ZERO, Vec3::ZERO, Vec3::Y, Vec3::Y]);
        assert_eq!(mesh.targets[0].normals, [Vec3::new(0.0, 0.5, 0.0); 4]);
        assert_eq!(mesh.targets[1].positions, [Vec3::new(0.5, 0.0, 0.0); 4]);
        assert!(mesh.targets[1].normals.is_empty(), "没写 NORMAL 的目标法线不变");
        assert_eq!(model.parts[0].weights, [0.5, 0.0], "节点的权重优先");
        assert_eq!(model.parts[1].weights, [0.25, 0.0], "节点没写时用网格的");

        let squish = &model.animations[0];
        assert_eq!((squish.name.as_str(), squish.duration), ("squish", 1.0));
        let [channel] = &squish.channels[..] else { panic!("{:?}", squish.channels) };
        assert_eq!((channel.node, channel.property, channel.interpolation), (model.parts[0].node.unwrap(), Property::Weights(0), Interpolation::Linear));
        assert_eq!(channel.values, [Vec4::ZERO, Vec4::new(1.0, 0.5, 0.0, 0.0), Vec4::ZERO]);
    }
}
