//! 变形目标的 GPU 数据：所有网格的位置差、法线差放进一个只增不减的 storage buffer，每个目标每个顶点两个 vec4（位置差、法线差）。
//! 扩容时在 GPU 上把旧数据拷到新缓冲。

use bevy_ecs::prelude::Component;
use glam::Vec3;
use nge_core::{Gfx, wgpu};

use crate::mesh::{MAX_MORPH_TARGETS, Mesh};

/// 直接用 [`crate::Mesh3d`] 画的实体的变形权重（第 i 个对应网格的第 i 个目标），少的按 0 算。
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct MorphWeights(pub Vec<f32>);

/// 初始容量（vec4 个数）；storage buffer 不能为空。
const INITIAL: u64 = 256;

pub(crate) struct MorphBuffer {
    pub buffer: wgpu::Buffer,
    /// 已经放进去多少个 vec4。
    len: u64,
    capacity: u64,
    warned: bool,
}

impl MorphBuffer {
    pub fn new(device: &wgpu::Device) -> Self {
        Self { buffer: buffer(device, INITIAL), len: 0, capacity: INITIAL, warned: false }
    }

    /// 追加一个网格的目标，返回着色器用的 [起点, 目标数, 顶点数, 0]；第二个值表示缓冲换了（bind group 要重建）。
    /// 放不下（超过一次能绑定的大小）时这个网格不变形，warn 一次。
    pub fn append(&mut self, gfx: &Gfx, mesh: &Mesh) -> ([u32; 4], bool) {
        let (count, vertices) = (mesh.targets.len().min(MAX_MORPH_TARGETS), mesh.vertices.len());
        let still = [0, 0, vertices as u32, 0];
        if count == 0 {
            return (still, false);
        }
        let mut data = Vec::with_capacity(count * vertices * 2);
        for target in &mesh.targets[..count] {
            for i in 0..vertices {
                let normal = target.normals.get(i).copied().unwrap_or(Vec3::ZERO);
                data.push(target.positions[i].extend(0.0).to_array());
                data.push(normal.extend(0.0).to_array());
            }
        }
        let start = self.len;
        let total = start + data.len() as u64;
        let limit = u64::from(gfx.device.limits().max_storage_buffer_binding_size) / 16;
        let Some(capacity) = capacity_for(total, self.capacity, limit) else {
            if !std::mem::replace(&mut self.warned, true) {
                log::warn!("变形数据超过一次能绑定的大小（{limit} 个 vec4），多出来的网格不变形");
            }
            return (still, false);
        };
        let grew = capacity != self.capacity;
        if grew {
            // 旧数据在 GPU 上搬到新缓冲，不留 CPU 副本
            let bigger = buffer(&gfx.device, capacity);
            let mut encoder = gfx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("mesh morph targets") });
            encoder.copy_buffer_to_buffer(&self.buffer, 0, &bigger, 0, start * 16);
            gfx.queue.submit([encoder.finish()]);
            (self.buffer, self.capacity) = (bigger, capacity);
        }
        gfx.queue.write_buffer(&self.buffer, start * 16, bytemuck::cast_slice(&data));
        self.len = total;
        ([start as u32, count as u32, vertices as u32, 0], grew)
    }
}

/// 放下 `total` 个 vec4 要多大的容量：够用不变，不够按两倍长但不超过 `limit`；超过 `limit` 是 `None`。
fn capacity_for(total: u64, capacity: u64, limit: u64) -> Option<u64> {
    if total > limit {
        None
    } else if total <= capacity {
        Some(capacity)
    } else {
        Some(total.next_power_of_two().min(limit))
    }
}

fn buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mesh morph targets"),
        size: capacity * 16,
        // 扩容时从旧的拷过来
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

/// 本地包围盒：每个顶点按各目标位置差的绝对值之和（分量各自算）往两边扩。
/// glTF 的权重可以是负的、也可以几个目标同时加满，所以不是只并上每个目标权重 1 时的位置。
pub(crate) fn bounds(mesh: &Mesh) -> (Vec3, Vec3) {
    let mut out = (Vec3::INFINITY, Vec3::NEG_INFINITY);
    for (i, v) in mesh.vertices.iter().enumerate() {
        let p = Vec3::from(v.position);
        let reach: Vec3 = mesh.targets.iter().take(MAX_MORPH_TARGETS).map(|t| t.positions[i].abs()).sum();
        out = (out.0.min(p - reach), out.1.max(p + reach));
    }
    out
}

/// 8 个权重打包成着色器里的两个 vec4。
pub(crate) fn pack_weights(weights: &[f32; MAX_MORPH_TARGETS]) -> [[f32; 4]; 2] {
    [[weights[0], weights[1], weights[2], weights[3]], [weights[4], weights[5], weights[6], weights[7]]]
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::*;
    use crate::mesh::{MorphTarget, Vertex};

    #[test]
    fn bounds_cover_any_mix_of_targets_including_negative_weights() {
        let mesh = Mesh {
            vertices: [Vec3::ZERO, Vec3::X, Vec3::Y].map(|p| Vertex::new(p, Vec3::Z, Vec2::ZERO)).to_vec(),
            indices: vec![0, 1, 2],
            targets: vec![
                MorphTarget { positions: vec![Vec3::ZERO, Vec3::ZERO, Vec3::Y * 2.0], normals: vec![] },
                MorphTarget { positions: vec![Vec3::NEG_X * 3.0; 3], normals: vec![] },
            ],
            ..Default::default()
        };
        // 每个顶点按 Σ|d| 往两边扩：顶点 2 是 x ±3、y ±2
        assert_eq!(bounds(&mesh), (Vec3::new(-3.0, -1.0, 0.0), Vec3::new(4.0, 3.0, 0.0)));
        assert_eq!(bounds(&Mesh { targets: vec![], ..mesh.clone() }), (Vec3::ZERO, Vec3::new(1.0, 1.0, 0.0)));

        // 两个目标同方向同时加满、或权重是负的，都还在盒子里
        let (lo, hi) = bounds(&Mesh { targets: vec![MorphTarget { positions: vec![Vec3::Y; 3], normals: vec![] }; 2], ..mesh });
        for p in [Vec3::Y * 3.0, Vec3::NEG_Y * 2.0] {
            assert!(p.cmpge(lo).all() && p.cmple(hi).all(), "{p} 不在 {lo}–{hi}");
        }
    }

    #[test]
    fn capacity_doubles_up_to_the_binding_limit() {
        assert_eq!(capacity_for(100, 256, 1 << 20), Some(256), "够用不换");
        assert_eq!(capacity_for(300, 256, 1 << 20), Some(512), "按两倍长");
        assert_eq!(capacity_for(700, 256, 1000), Some(1000), "不超过上限");
        assert_eq!(capacity_for(1000, 256, 1000), Some(1000), "正好放满");
        assert_eq!(capacity_for(1001, 256, 1000), None, "放不下");
    }

    #[test]
    fn weights_pack_in_order() {
        let w = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        assert_eq!(pack_weights(&w), [[1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]]);
    }
}
