//! 精灵合批：遮罩先画，再按 (z, 纹理) 排序，相邻同管线同纹理的合成一段实例。

use std::ops::Range;

use crate::assets::TextureId;

/// 每个精灵一份，64 字节。
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
    /// bit0：受 2D 光照；bit1：用法线图；bit2：不被影子盖住（挡光的精灵自己）。
    pub flags: u32,
    /// 给着色器的参数。
    pub params: [f32; 4],
}

pub(crate) const LIT: u32 = 1;
pub(crate) const BUMPY: u32 = 2;
pub(crate) const NO_SHADOW: u32 = 4;

pub(crate) struct Item {
    pub texture: TextureId,
    /// 法线图；白色是没有。
    pub normal: TextureId,
    /// 用哪条管线（渲染器自己编的号）。
    pub pipe: u32,
    /// 遮罩：不管 z 多少都先画。
    pub mask: bool,
    pub instance: Instance,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Batch {
    pub texture: TextureId,
    pub normal: TextureId,
    pub pipe: u32,
    pub range: Range<u32>,
}

/// 排序并合批，结果写进 `instances` / `batches`（复用分配）。遮罩排最前，z 相同时按纹理分组。
/// 管线号不参与排序（它按着色器第一次用到的先后分，会变），只在换管线时断开一批，同 z 同纹理的先后还是原来的顺序。
pub(crate) fn batch(items: &mut [Item], instances: &mut Vec<Instance>, batches: &mut Vec<Batch>) {
    items.sort_by(|a, b| b.mask.cmp(&a.mask).then(a.instance.rotation_z[1].total_cmp(&b.instance.rotation_z[1])).then(a.texture.cmp(&b.texture)).then(a.normal.cmp(&b.normal)));
    instances.clear();
    batches.clear();
    for (i, item) in items.iter().enumerate() {
        let i = i as u32;
        instances.push(item.instance);
        match batches.last_mut() {
            Some(last) if last.texture == item.texture && last.normal == item.normal && last.pipe == item.pipe => last.range.end = i + 1,
            _ => batches.push(Batch { texture: item.texture, normal: item.normal, pipe: item.pipe, range: i..i + 1 }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(texture: u32, z: f32, x: f32) -> Item {
        let instance = Instance { pos: [x, 0.0], rotation_z: [0.0, z], ..bytemuck::Zeroable::zeroed() };
        Item { texture: TextureId::new(texture), normal: TextureId::WHITE, pipe: 0, mask: false, instance }
    }

    fn run(mut items: Vec<Item>) -> (Vec<Instance>, Vec<Batch>) {
        let (mut instances, mut batches) = (Vec::new(), Vec::new());
        batch(&mut items, &mut instances, &mut batches);
        (instances, batches)
    }

    #[test]
    fn instance_is_64_bytes() {
        assert_eq!(size_of::<Instance>(), 64);
    }

    #[test]
    fn masks_first_and_pipelines_split_batches_without_reordering() {
        let with = |mut i: Item, pipe: u32, mask: bool| {
            (i.pipe, i.mask) = (pipe, mask);
            i
        };
        let (instances, batches) =
            run(vec![with(item(1, 0.0, 0.0), 2, false), with(item(1, 0.0, 1.0), 1, false), with(item(1, 9.0, 2.0), 3, true), with(item(1, 0.0, 3.0), 2, false)]);
        let xs: Vec<f32> = instances.iter().map(|i| i.pos[0]).collect();
        assert_eq!(xs, vec![2.0, 0.0, 1.0, 3.0], "遮罩不管 z 先画，同 z 同纹理保持原来的顺序");
        let t = TextureId::new(1);
        let b = |pipe, range| Batch { texture: t, normal: TextureId::WHITE, pipe, range };
        assert_eq!(batches, vec![b(3, 0..1), b(2, 1..2), b(1, 2..3), b(2, 3..4)], "换管线就断开");
    }

    #[test]
    fn same_z_groups_by_texture() {
        let (_, batches) = run(vec![item(1, 0.0, 0.0), item(2, 0.0, 1.0), item(1, 0.0, 2.0)]);
        assert_eq!(
            batches,
            vec![Batch { texture: TextureId::new(1), normal: TextureId::WHITE, pipe: 0, range: 0..2 }, Batch { texture: TextureId::new(2), normal: TextureId::WHITE, pipe: 0, range: 2..3 }]
        );
    }

    #[test]
    fn same_texture_groups_by_normal_map() {
        let with = |mut i: Item, normal: u32| {
            i.normal = TextureId::new(normal);
            i
        };
        let (instances, batches) = run(vec![with(item(1, 0.0, 0.0), 5), with(item(1, 0.0, 1.0), 6), with(item(1, 0.0, 2.0), 5)]);
        let xs: Vec<f32> = instances.iter().map(|i| i.pos[0]).collect();
        assert_eq!(xs, vec![0.0, 2.0, 1.0]);
        assert_eq!(batches.iter().map(|b| (b.normal.index(), b.range.clone())).collect::<Vec<_>>(), vec![(5, 0..2), (6, 2..3)], "法线图不同就断开");
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
        assert_eq!(batches, vec![Batch { texture: TextureId::new(3), normal: TextureId::WHITE, pipe: 0, range: 0..1000 }]);
        assert!(instances.windows(2).all(|w| w[0].pos[0] < w[1].pos[0]));
    }

    #[test]
    fn empty() {
        let (instances, batches) = run(Vec::new());
        assert!(instances.is_empty() && batches.is_empty());
    }
}
