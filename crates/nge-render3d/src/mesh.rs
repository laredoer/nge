//! 网格：交错顶点（position / normal / uv）加 u32 索引，逆时针为正面。

use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

impl Vertex {
    pub fn new(position: Vec3, normal: Vec3, uv: Vec2) -> Self {
        Self { position: position.into(), normal: normal.into(), uv: uv.into() }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    /// 边长 `size` 的立方体，中心在原点；每个面 4 个顶点，uv 铺满整张图。
    pub fn cube(size: f32) -> Self {
        // (法线, u, v)，u × v = 法线，保证逆时针朝外
        const FACES: [(Vec3, Vec3, Vec3); 6] = [
            (Vec3::X, Vec3::NEG_Z, Vec3::Y),
            (Vec3::NEG_X, Vec3::Z, Vec3::Y),
            (Vec3::Y, Vec3::X, Vec3::NEG_Z),
            (Vec3::NEG_Y, Vec3::X, Vec3::Z),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
        ];
        let mut mesh = Self::default();
        for (n, u, v) in FACES {
            mesh.quad(n * size / 2.0, n, u * size / 2.0, v * size / 2.0);
        }
        mesh
    }

    /// 以 `center` 为中心、半轴 `u` / `v` 的四边形；uv 的 v 朝下（贴图上边对着 +v）。
    fn quad(&mut self, center: Vec3, normal: Vec3, u: Vec3, v: Vec3) {
        let base = self.vertices.len() as u32;
        for (du, dv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let uv = Vec2::new((du + 1.0) / 2.0, (1.0 - dv) / 2.0);
            self.vertices.push(Vertex::new(center + u * du + v * dv, normal, uv));
        }
        self.indices.extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
    }

    /// 朝 +Y 的正方形平面，中心在原点。
    pub fn plane(size: f32) -> Self {
        let mut mesh = Self::default();
        mesh.quad(Vec3::ZERO, Vec3::Y, Vec3::X * size / 2.0, Vec3::NEG_Z * size / 2.0);
        mesh
    }

    /// 经纬球；`segments` 是经线数（纬线数取一半），至少 3。
    pub fn sphere(radius: f32, segments: u32) -> Self {
        let segments = segments.max(3);
        let rings = (segments / 2).max(2);
        let mut mesh = Self::default();
        for j in 0..=rings {
            let theta = std::f32::consts::PI * j as f32 / rings as f32;
            for i in 0..=segments {
                let phi = std::f32::consts::TAU * i as f32 / segments as f32;
                let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), -theta.sin() * phi.sin());
                let uv = Vec2::new(i as f32 / segments as f32, j as f32 / rings as f32);
                mesh.vertices.push(Vertex::new(n * radius, n, uv));
            }
        }
        let row = segments + 1;
        for j in 0..rings {
            for i in 0..segments {
                let (a, b) = (j * row + i, (j + 1) * row + i);
                // 两极各有一半三角形退化成线，不生成
                if j + 1 < rings {
                    mesh.indices.extend([a, b, b + 1]);
                }
                if j > 0 {
                    mesh.indices.extend([a, b + 1, a + 1]);
                }
            }
        }
        mesh
    }

    /// 按面法线（面积加权）重算顶点法线。
    pub fn compute_normals(&mut self) {
        let mut sums = vec![Vec3::ZERO; self.vertices.len()];
        for tri in self.indices.chunks_exact(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| i as usize);
            if [a, b, c].iter().any(|&i| i >= self.vertices.len()) {
                continue;
            }
            let [pa, pb, pc] = [a, b, c].map(|i| Vec3::from(self.vertices[i].position));
            let face = (pb - pa).cross(pc - pa);
            for i in [a, b, c] {
                sums[i] += face;
            }
        }
        for (v, n) in self.vertices.iter_mut().zip(sums) {
            v.normal = n.try_normalize().unwrap_or(Vec3::Y).into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(v: &Vertex) -> Vec3 {
        Vec3::from(v.position)
    }

    /// 每个三角形按逆时针看向外（几何法线和顶点法线同向）。
    fn assert_outward(mesh: &Mesh) {
        assert_eq!(mesh.indices.len() % 3, 0);
        assert!(mesh.indices.iter().all(|&i| (i as usize) < mesh.vertices.len()));
        for tri in mesh.indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|k| &mesh.vertices[tri[k] as usize]);
            let face = (pos(b) - pos(a)).cross(pos(c) - pos(a));
            if face.length() < 1e-8 {
                continue;
            }
            assert!(face.dot(Vec3::from(a.normal)) > 0.0, "三角形 {tri:?} 朝内");
        }
        for v in &mesh.vertices {
            assert!((Vec3::from(v.normal).length() - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn cube_has_24_vertices_facing_out() {
        let cube = Mesh::cube(2.0);
        assert_eq!((cube.vertices.len(), cube.indices.len()), (24, 36));
        assert!(cube.vertices.iter().all(|v| pos(v).abs().max_element() == 1.0));
        assert_outward(&cube);
    }

    #[test]
    fn plane_faces_up() {
        let plane = Mesh::plane(4.0);
        assert_eq!((plane.vertices.len(), plane.indices.len()), (4, 6));
        assert!(plane.vertices.iter().all(|v| v.normal == [0.0, 1.0, 0.0] && v.position[1] == 0.0));
        assert_outward(&plane);
    }

    #[test]
    fn sphere_vertices_on_radius() {
        let sphere = Mesh::sphere(1.5, 16);
        assert!(sphere.vertices.iter().all(|v| (pos(v).length() - 1.5).abs() < 1e-4));
        assert_outward(&sphere);
        assert_eq!(Mesh::sphere(1.0, 1).vertices.len(), Mesh::sphere(1.0, 3).vertices.len(), "segments 至少 3");
    }

    #[test]
    fn computed_normals_follow_winding() {
        let mut mesh = Mesh {
            vertices: [Vec3::ZERO, Vec3::X, Vec3::Y].map(|p| Vertex::new(p, Vec3::ZERO, Vec2::ZERO)).to_vec(),
            indices: vec![0, 1, 2],
        };
        mesh.compute_normals();
        assert!(mesh.vertices.iter().all(|v| v.normal == [0.0, 0.0, 1.0]));
    }

    #[test]
    fn degenerate_triangles_get_a_fallback_normal() {
        let mut mesh = Mesh { vertices: vec![Vertex::new(Vec3::ZERO, Vec3::ZERO, Vec2::ZERO); 3], indices: vec![0, 1, 2] };
        mesh.compute_normals();
        assert!(mesh.vertices.iter().all(|v| (Vec3::from(v.normal).length() - 1.0).abs() < 1e-6));
    }
}
