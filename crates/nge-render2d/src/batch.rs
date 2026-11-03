//! 精灵合批：按 (z, 纹理) 排序，相邻同纹理的合成一段实例。

use std::ops::Range;

use crate::assets::TextureId;

/// 每个精灵一份，48 字节。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Instance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    /// 旋转（弧度）和 z。
    pub rotation_z: [f32; 2],
    pub uv: [f32; 4],
    /// RGBA8，线性。
    pub color: [u8; 4],
    pub _pad: u32,
}

pub(crate) struct Item {
    pub texture: TextureId,
    pub instance: Instance,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Batch {
    pub texture: TextureId,
    pub range: Range<u32>,
}

/// 排序并合批，结果写进 `instances` / `batches`（复用分配）。z 相同时保持原有顺序之外按纹理分组。
pub(crate) fn batch(items: &mut [Item], instances: &mut Vec<Instance>, batches: &mut Vec<Batch>) {
    items.sort_by(|a, b| {
        a.instance.rotation_z[1].total_cmp(&b.instance.rotation_z[1]).then(a.texture.cmp(&b.texture))
    });
    instances.clear();
    batches.clear();
    for (i, item) in items.iter().enumerate() {
        let i = i as u32;
        instances.push(item.instance);
        match batches.last_mut() {
            Some(last) if last.texture == item.texture => last.range.end = i + 1,
            _ => batches.push(Batch { texture: item.texture, range: i..i + 1 }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(texture: u32, z: f32, x: f32) -> Item {
        let instance = Instance { pos: [x, 0.0], rotation_z: [0.0, z], ..bytemuck::Zeroable::zeroed() };
        Item { texture: TextureId(texture), instance }
    }

    fn run(mut items: Vec<Item>) -> (Vec<Instance>, Vec<Batch>) {
        let (mut instances, mut batches) = (Vec::new(), Vec::new());
        batch(&mut items, &mut instances, &mut batches);
        (instances, batches)
    }

    #[test]
    fn instance_is_48_bytes() {
        assert_eq!(size_of::<Instance>(), 48);
    }

    #[test]
    fn same_z_groups_by_texture() {
        let (_, batches) = run(vec![item(1, 0.0, 0.0), item(2, 0.0, 1.0), item(1, 0.0, 2.0)]);
        assert_eq!(
            batches,
            vec![Batch { texture: TextureId(1), range: 0..2 }, Batch { texture: TextureId(2), range: 2..3 }]
        );
    }

    #[test]
    fn z_order_wins_over_texture() {
        let (instances, batches) = run(vec![item(1, 2.0, 0.0), item(2, 1.0, 1.0), item(1, 0.0, 2.0)]);
        let xs: Vec<f32> = instances.iter().map(|i| i.pos[0]).collect();
        assert_eq!(xs, vec![2.0, 1.0, 0.0]);
        assert_eq!(batches.len(), 3);
    }

    #[test]
    fn single_texture_is_one_draw_and_stable() {
        let items = (0..1000).map(|i| item(3, 0.0, i as f32)).collect();
        let (instances, batches) = run(items);
        assert_eq!(batches, vec![Batch { texture: TextureId(3), range: 0..1000 }]);
        assert!(instances.windows(2).all(|w| w[0].pos[0] < w[1].pos[0]));
    }

    #[test]
    fn empty() {
        let (instances, batches) = run(Vec::new());
        assert!(instances.is_empty() && batches.is_empty());
    }
}
