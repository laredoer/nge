//! 3D 导航网格（NavMesh3d）：从 3D 地形/场景三角网格自动筛选可行走坡面、剔除静态障碍，
//! 提供空间哈希加速的最近点查询、A* 走廊搜索与漏斗（Funnel）路径平滑。

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use bevy_ecs::prelude::*;
use glam::{Vec2, Vec3};

/// 单次 3D A* 最多展开的三角形数，避免超大地图极端情况卡顿。
pub const MAX_NAV3D_EXPANDED: usize = 8192;

/// 3D 导航障碍物（用于构建 NavMesh 时剔除不可行走区域）。
#[derive(Debug, Clone, PartialEq)]
pub enum NavObstacle3d {
    /// 竖直圆柱（树干、石柱、圣坛等）。
    Cylinder { center: Vec3, radius: f32, half_height: f32 },
    /// 轴对齐包围盒（建筑、围墙、箱子等）。
    Box { min: Vec3, max: Vec3 },
}

impl NavObstacle3d {
    /// 给定额外膨胀半径（`agent_radius`）判断点是否被该障碍物覆盖。
    pub fn blocks(&self, p: Vec3, pad: f32) -> bool {
        match *self {
            Self::Cylinder { center, radius, half_height } => {
                let dx = p.x - center.x;
                let dz = p.z - center.z;
                let r = radius + pad;
                dx * dx + dz * dz <= r * r && (p.y - center.y).abs() <= half_height + 24.0
            }
            Self::Box { min, max } => {
                p.x >= min.x - pad
                    && p.x <= max.x + pad
                    && p.z >= min.z - pad
                    && p.z <= max.z + pad
                    && p.y >= min.y - 12.0
                    && p.y <= max.y + 24.0
            }
        }
    }
}

/// 构建 [`NavMesh3d`] 的参数。
#[derive(Debug, Clone, PartialEq)]
pub struct NavConfig3d {
    /// 可行走的最大坡度（度），默认 45°。
    pub max_slope_deg: f32,
    /// 最低可行走海拔（例如过滤海平面以下区域），默认不限制。
    pub min_y: f32,
    /// 角色半径（用于给静态障碍物向外膨胀，避免贴边穿模）。
    pub agent_radius: f32,
    /// 静态障碍物列表。
    pub obstacles: Vec<NavObstacle3d>,
}

impl Default for NavConfig3d {
    fn default() -> Self {
        Self { max_slope_deg: 45.0, min_y: f32::NEG_INFINITY, agent_radius: 12.0, obstacles: Vec::new() }
    }
}

/// 导航网格中的一个可行走三角形。
#[derive(Debug, Clone)]
pub struct NavTri {
    pub verts: [u32; 3],
    pub center: Vec3,
    pub normal: Vec3,
    /// 每条边 `e` = `(verts[e], verts[(e+1)%3])` 对面的相邻三角形下标。
    pub neighbors: [Option<u32>; 3],
    pub min_xz: Vec2,
    pub max_xz: Vec2,
}

/// 3D 导航网格资源：支持从三角网格实时构建、点投影、高度采样与 A* + 漏斗平滑寻路。
#[derive(Resource, Debug, Clone, Default)]
pub struct NavMesh3d {
    vertices: Vec<Vec3>,
    tris: Vec<NavTri>,
    /// XZ 空间哈希加速：`(gx, gz)` → 覆盖该格子的三角形下标。
    cell_size: f32,
    buckets: HashMap<(i32, i32), Vec<u32>>,
}

impl NavMesh3d {
    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    pub fn tri_count(&self) -> usize {
        self.tris.len()
    }

    pub fn vertices(&self) -> &[Vec3] {
        &self.vertices
    }

    pub fn tris(&self) -> &[NavTri] {
        &self.tris
    }

    /// 从一组世界空间三角形构建 3D 导航网格：
    /// 1. 过滤掉退化三角形、过陡坡面（`normal.y < cos(max_slope)`）、低于 `min_y` 的面、被静态障碍覆盖的面；
    /// 2. 焊接共享顶点（量化精度 0.05），自动建立边邻接图；
    /// 3. 建立 XZ 空间网格索引加速最近面查询。
    pub fn build(triangles: &[[Vec3; 3]], config: &NavConfig3d) -> Self {
        let min_ny = config.max_slope_deg.clamp(1.0, 89.0).to_radians().cos();
        let pad = config.agent_radius.max(0.0);

        let mut vertices: Vec<Vec3> = Vec::new();
        let mut weld: HashMap<(i32, i32, i32), u32> = HashMap::new();
        let mut tris: Vec<NavTri> = Vec::new();

        let mut intern = |p: Vec3| -> u32 {
            // 0.05 世界单位量化，稳健焊接共享边顶点
            let key = ((p.x * 20.0).round() as i32, (p.y * 20.0).round() as i32, (p.z * 20.0).round() as i32);
            if let Some(&idx) = weld.get(&key) {
                return idx;
            }
            let idx = vertices.len() as u32;
            vertices.push(p);
            weld.insert(key, idx);
            idx
        };

        for &[a, b, c] in triangles {
            if !(a.is_finite() && b.is_finite() && c.is_finite()) {
                continue;
            }
            let raw_n = (b - a).cross(c - a);
            let len = raw_n.length();
            if len < 1e-5 {
                continue;
            }
            let mut normal = raw_n / len;
            // 确保法线朝上（统一绕序，使俯视逆时针）
            let (p0, p1, p2) = if normal.y < 0.0 {
                normal = -normal;
                (a, c, b)
            } else {
                (a, b, c)
            };
            if normal.y < min_ny {
                continue;
            }
            let center = (p0 + p1 + p2) / 3.0;
            if center.y < config.min_y || p0.y.min(p1.y).min(p2.y) < config.min_y - 2.0 {
                continue;
            }
            if !config.obstacles.is_empty() && config.obstacles.iter().any(|o| o.blocks(center, pad)) {
                continue;
            }
            let v0 = intern(p0);
            let v1 = intern(p1);
            let v2 = intern(p2);
            if v0 == v1 || v1 == v2 || v2 == v0 {
                continue;
            }
            let min_xz = Vec2::new(p0.x.min(p1.x).min(p2.x), p0.z.min(p1.z).min(p2.z));
            let max_xz = Vec2::new(p0.x.max(p1.x).max(p2.x), p0.z.max(p1.z).max(p2.z));
            tris.push(NavTri { verts: [v0, v1, v2], center, normal, neighbors: [None; 3], min_xz, max_xz });
        }

        // 建立无向边 → 相邻三角形映射
        let mut edge_map: HashMap<(u32, u32), Vec<(u32, usize)>> = HashMap::with_capacity(tris.len() * 2);
        for (ti, tri) in tris.iter().enumerate() {
            for e in 0..3 {
                let u = tri.verts[e];
                let v = tri.verts[(e + 1) % 3];
                let key = if u < v { (u, v) } else { (v, u) };
                edge_map.entry(key).or_default().push((ti as u32, e));
            }
        }
        for owners in edge_map.values() {
            if owners.len() == 2 {
                let (t0, e0) = owners[0];
                let (t1, e1) = owners[1];
                tris[t0 as usize].neighbors[e0] = Some(t1);
                tris[t1 as usize].neighbors[e1] = Some(t0);
            }
        }

        // 计算空间哈希桶尺寸（大约 32×32 格）
        let (mut lo, mut hi) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
        for tri in &tris {
            lo = lo.min(tri.min_xz);
            hi = hi.max(tri.max_xz);
        }
        let span = if lo.is_finite() && hi.is_finite() { (hi.x - lo.x).max(hi.y - lo.y) } else { 64.0 };
        let cell_size = (span / 32.0).max(4.0);
        let mut buckets: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (ti, tri) in tris.iter().enumerate() {
            let x0 = (tri.min_xz.x / cell_size).floor() as i32;
            let x1 = (tri.max_xz.x / cell_size).floor() as i32;
            let z0 = (tri.min_xz.y / cell_size).floor() as i32;
            let z1 = (tri.max_xz.y / cell_size).floor() as i32;
            for gx in x0..=x1 {
                for gz in z0..=z1 {
                    buckets.entry((gx, gz)).or_default().push(ti as u32);
                }
            }
        }

        Self { vertices, tris, cell_size, buckets }
    }

    /// 查找距离 `pos` 最近的可行走三角形与表面投影点。
    pub fn nearest_point(&self, pos: Vec3) -> Option<(u32, Vec3)> {
        if self.tris.is_empty() {
            return None;
        }
        let gx = (pos.x / self.cell_size).floor() as i32;
        let gz = (pos.z / self.cell_size).floor() as i32;
        let mut best: Option<(u32, Vec3, f32)> = None;

        // 从 3×3 邻域逐步扩大到 7×7，大多数查询在第一次就命中
        for radius in [1i32, 3] {
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if let Some(list) = self.buckets.get(&(gx + dx, gz + dz)) {
                        for &ti in list {
                            let p = self.closest_on_tri(ti as usize, pos);
                            let d2 = p.distance_squared(pos);
                            if best.is_none_or(|b| d2 < b.2) {
                                best = Some((ti, p, d2));
                            }
                        }
                    }
                }
            }
            if best.is_some() {
                break;
            }
        }

        // 兜底全扫描（目标离网格很远时）
        if best.is_none() {
            for ti in 0..self.tris.len() {
                let p = self.closest_on_tri(ti, pos);
                let d2 = p.distance_squared(pos);
                if best.is_none_or(|b| d2 < b.2) {
                    best = Some((ti as u32, p, d2));
                }
            }
        }

        best.map(|(ti, p, _)| (ti, p))
    }

    /// 在给定 XZ 坐标采样最接近 `hint_y` 的导航网格表面高度。
    pub fn sample_height(&self, xz: Vec2, hint_y: f32) -> Option<f32> {
        if self.tris.is_empty() {
            return None;
        }
        let gx = (xz.x / self.cell_size).floor() as i32;
        let gz = (xz.y / self.cell_size).floor() as i32;
        let mut best: Option<(f32, f32)> = None;
        if let Some(list) = self.buckets.get(&(gx, gz)) {
            for &ti in list {
                let tri = &self.tris[ti as usize];
                if xz.x < tri.min_xz.x - 1e-3 || xz.x > tri.max_xz.x + 1e-3 || xz.y < tri.min_xz.y - 1e-3 || xz.y > tri.max_xz.y + 1e-3 {
                    continue;
                }
                let a = self.vertices[tri.verts[0] as usize];
                let b = self.vertices[tri.verts[1] as usize];
                let c = self.vertices[tri.verts[2] as usize];
                if let Some(y) = barycentric_height(xz, a, b, c) {
                    let dy = (y - hint_y).abs();
                    if best.is_none_or(|b| dy < b.1) {
                        best = Some((y, dy));
                    }
                }
            }
        }
        best.map(|(y, _)| y).or_else(|| self.nearest_point(Vec3::new(xz.x, hint_y, xz.y)).map(|(_, p)| p.y))
    }

    /// 3D 寻路：A* 搜索三角形走廊 + Funnel（漏斗）拉直平滑，返回贴合 3D 表面的路点序列（含起点与终点）。
    pub fn find_path(&self, start: Vec3, goal: Vec3) -> Option<Vec<Vec3>> {
        let (start_tri, start_pos) = self.nearest_point(start)?;
        let (goal_tri, goal_pos) = self.nearest_point(goal)?;
        if start_tri == goal_tri {
            return Some(vec![start_pos, goal_pos]);
        }

        // A* over triangles
        let mut open = BinaryHeap::new();
        let mut came_from: HashMap<u32, u32> = HashMap::new();
        let mut g_score: HashMap<u32, f32> = HashMap::new();
        g_score.insert(start_tri, 0.0);
        open.push(AStarNode { tri: start_tri, f: start_pos.distance(goal_pos) });

        let mut expanded = 0usize;
        let mut found = false;
        while let Some(cur) = open.pop() {
            if cur.tri == goal_tri {
                found = true;
                break;
            }
            expanded += 1;
            if expanded > MAX_NAV3D_EXPANDED {
                break;
            }
            let cur_g = g_score[&cur.tri];
            let cur_center = self.tris[cur.tri as usize].center;
            for next in self.tris[cur.tri as usize].neighbors.into_iter().flatten() {
                let next_center = self.tris[next as usize].center;
                let tentative = cur_g + cur_center.distance(next_center);
                if g_score.get(&next).is_none_or(|&old| tentative < old) {
                    came_from.insert(next, cur.tri);
                    g_score.insert(next, tentative);
                    let f = tentative + next_center.distance(goal_pos);
                    open.push(AStarNode { tri: next, f });
                }
            }
        }
        if !found {
            return None;
        }

        // 重建三角形走廊
        let mut corridor = vec![goal_tri];
        let mut cur = goal_tri;
        while let Some(&prev) = came_from.get(&cur) {
            corridor.push(prev);
            cur = prev;
        }
        corridor.reverse();

        // 提取相邻三角形之间的传送门边（左点, 右点）
        let mut portals: Vec<(Vec3, Vec3)> = Vec::with_capacity(corridor.len() + 1);
        portals.push((start_pos, start_pos));
        for pair in corridor.windows(2) {
            let t0 = &self.tris[pair[0] as usize];
            let t1_idx = pair[1];
            if let Some(edge) = t0.neighbors.iter().position(|&n| n == Some(t1_idx)) {
                let v_a = self.vertices[t0.verts[edge] as usize];
                let v_b = self.vertices[t0.verts[(edge + 1) % 3] as usize];
                // 沿着走廊前进方向（t0 -> t1）确定左/右端点，保证 Funnel 方向一致
                let dir = self.tris[t1_idx as usize].center - t0.center;
                let cross_y = (v_a.x - t0.center.x) * dir.z - (v_a.z - t0.center.z) * dir.x;
                if cross_y >= 0.0 {
                    portals.push((v_a, v_b));
                } else {
                    portals.push((v_b, v_a));
                }
            }
        }
        portals.push((goal_pos, goal_pos));

        let raw = string_pull(&portals);
        // 把每个路点采样回 3D 表面高度，并对超长跨度按需插值以贴合起伏坡面
        let mut smooth = Vec::with_capacity(raw.len());
        let mut prev_y = start_pos.y;
        for p in raw {
            let y = self.sample_height(Vec2::new(p.x, p.z), prev_y).unwrap_or(p.y);
            prev_y = y;
            let pt = Vec3::new(p.x, y, p.z);
            if smooth.last().is_none_or(|last: &Vec3| last.distance_squared(pt) > 1e-4) {
                smooth.push(pt);
            }
        }
        if smooth.len() < 2 {
            smooth.push(goal_pos);
        }
        Some(smooth)
    }

    fn closest_on_tri(&self, tri_idx: usize, p: Vec3) -> Vec3 {
        let tri = &self.tris[tri_idx];
        let a = self.vertices[tri.verts[0] as usize];
        let b = self.vertices[tri.verts[1] as usize];
        let c = self.vertices[tri.verts[2] as usize];
        closest_point_on_triangle(p, a, b, c)
    }
}

#[derive(Clone, Copy)]
struct AStarNode {
    tri: u32,
    f: f32,
}

impl PartialEq for AStarNode {
    fn eq(&self, other: &Self) -> bool {
        self.tri == other.tri && self.f.to_bits() == other.f.to_bits()
    }
}
impl Eq for AStarNode {}

impl Ord for AStarNode {
    fn cmp(&self, other: &Self) -> Ordering {
        other.f.total_cmp(&self.f).then_with(|| self.tri.cmp(&other.tri))
    }
}
impl PartialOrd for AStarNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// XZ 平面 2D 有向面积 × 2（`> 0` 表示 c 在 a→b 左侧，右手系 +Y 朝上）。
#[inline]
fn tri_area2_xz(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    (b.x - a.x) * (c.z - a.z) - (b.z - a.z) * (c.x - a.x)
}

/// Simple Stupid Funnel Algorithm：沿传送门列表拉直最短路径。
fn string_pull(portals: &[(Vec3, Vec3)]) -> Vec<Vec3> {
    let mut path = Vec::new();
    if portals.is_empty() {
        return path;
    }
    let mut portal_apex = portals[0].0;
    let mut portal_left = portals[0].0;
    let mut portal_right = portals[0].1;
    let mut left_index = 0usize;
    let mut right_index = 0usize;
    path.push(portal_apex);

    let mut i = 1usize;
    while i < portals.len() {
        let (left, right) = portals[i];

        // 更新右边界
        if tri_area2_xz(portal_apex, portal_right, right) <= 0.0 {
            if portal_apex.distance_squared(portal_right) < 1e-6 || tri_area2_xz(portal_apex, portal_left, right) > 0.0 {
                portal_right = right;
                right_index = i;
            } else {
                path.push(portal_left);
                portal_apex = portal_left;
                let apex_index = left_index;
                portal_left = portal_apex;
                portal_right = portal_apex;
                left_index = apex_index;
                right_index = apex_index;
                i = apex_index + 1;
                continue;
            }
        }

        // 更新左边界
        if tri_area2_xz(portal_apex, portal_left, left) >= 0.0 {
            if portal_apex.distance_squared(portal_left) < 1e-6 || tri_area2_xz(portal_apex, portal_right, left) < 0.0 {
                portal_left = left;
                left_index = i;
            } else {
                path.push(portal_right);
                portal_apex = portal_right;
                let apex_index = right_index;
                portal_left = portal_apex;
                portal_right = portal_apex;
                left_index = apex_index;
                right_index = apex_index;
                i = apex_index + 1;
                continue;
            }
        }

        i += 1;
    }

    let goal = portals.last().unwrap().0;
    if path.last().is_none_or(|p| p.distance_squared(goal) > 1e-4) {
        path.push(goal);
    }
    path
}

/// XZ 平面重心坐标高度插值；点不在三角形内时返回 `None`。
fn barycentric_height(p: Vec2, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let v0 = Vec2::new(b.x - a.x, b.z - a.z);
    let v1 = Vec2::new(c.x - a.x, c.z - a.z);
    let v2 = Vec2::new(p.x - a.x, p.y - a.z);
    let denom = v0.x * v1.y - v1.x * v0.y;
    if denom.abs() < 1e-6 {
        return None;
    }
    let inv = 1.0 / denom;
    let v = (v2.x * v1.y - v1.x * v2.y) * inv;
    let w = (v0.x * v2.y - v2.x * v0.y) * inv;
    let u = 1.0 - v - w;
    if u >= -1e-3 && v >= -1e-3 && w >= -1e-3 {
        Some(u * a.y + v * b.y + w * c.y)
    } else {
        None
    }
}

/// 点 `p` 到 3D 三角形 `(a, b, c)` 的最近点（Christer Ericson 经典 Voronoi 区域算法）。
fn closest_point_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return a + ab * v;
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return a + ac * w;
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return b + (c - b) * w;
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    a + ab * v + ac * w
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 生成一个 `cols × rows` 的平坦网格（每格 `step` 大小），高度由 `height(x, z)` 决定。
    fn grid_tris(cols: usize, rows: usize, step: f32, height: impl Fn(f32, f32) -> f32) -> Vec<[Vec3; 3]> {
        let mut tris = Vec::new();
        for gz in 0..rows {
            for gx in 0..cols {
                let x0 = gx as f32 * step;
                let z0 = gz as f32 * step;
                let x1 = x0 + step;
                let z1 = z0 + step;
                let p00 = Vec3::new(x0, height(x0, z0), z0);
                let p10 = Vec3::new(x1, height(x1, z0), z0);
                let p01 = Vec3::new(x0, height(x0, z1), z1);
                let p11 = Vec3::new(x1, height(x1, z1), z1);
                tris.push([p00, p01, p10]);
                tris.push([p10, p01, p11]);
            }
        }
        tris
    }

    #[test]
    fn filters_steep_slopes_and_underwater_triangles() {
        let mut tris = grid_tris(4, 4, 10.0, |x, _| if x < 15.0 { -20.0 } else { 0.0 });
        // 加一面 90° 竖直悬崖墙
        tris.push([Vec3::new(20.0, 0.0, 0.0), Vec3::new(20.0, 20.0, 0.0), Vec3::new(20.0, 0.0, 20.0)]);
        let nav = NavMesh3d::build(&tris, &NavConfig3d { max_slope_deg: 45.0, min_y: -5.0, ..Default::default() });
        assert!(nav.tri_count() > 0);
        for tri in nav.tris() {
            assert!(tri.normal.y >= 45f32.to_radians().cos() - 1e-4, "过陡坡面应被过滤");
            assert!(tri.center.y >= -5.0, "水下区域应被过滤");
        }
    }

    #[test]
    fn finds_smooth_path_around_obstacle_and_samples_slope_height() {
        // 10×10 缓坡地面（每格 10，y = x * 0.25）
        let tris = grid_tris(10, 10, 10.0, |x, _| x * 0.25);
        // 中间放一根挡路的柱子（x = 50, z = 0..60）
        let obstacle = NavObstacle3d::Box { min: Vec3::new(42.0, -5.0, -5.0), max: Vec3::new(58.0, 30.0, 62.0) };
        let nav = NavMesh3d::build(&tris, &NavConfig3d { agent_radius: 4.0, obstacles: vec![obstacle], ..Default::default() });

        let start = Vec3::new(15.0, 3.75, 25.0);
        let goal = Vec3::new(85.0, 21.25, 25.0);
        let path = nav.find_path(start, goal).expect("应找到绕过墙体的路径");
        assert!(path.len() >= 3, "绕墙路径至少包含拐点：{path:?}");
        assert!(path.first().unwrap().distance(start) < 1e-2);
        assert!(path.last().unwrap().distance(goal) < 1e-2);
        assert!(path.iter().any(|p| p.z > 62.0), "应绕过墙体末端（z > 62）：{path:?}");
        for p in &path {
            assert!((p.y - p.x * 0.25).abs() < 0.2, "路点高度应贴合坡面：{p:?}");
        }
    }

    #[test]
    fn returns_none_when_islands_are_disconnected() {
        let mut tris = grid_tris(2, 2, 10.0, |_, _| 0.0);
        // 另一座孤岛，中间隔开 100 单位
        tris.extend(grid_tris(2, 2, 10.0, |_, _| 0.0).into_iter().map(|[a, b, c]| [a + Vec3::X * 120.0, b + Vec3::X * 120.0, c + Vec3::X * 120.0]));
        let nav = NavMesh3d::build(&tris, &NavConfig3d::default());
        assert!(nav.find_path(Vec3::new(5.0, 0.0, 5.0), Vec3::new(125.0, 0.0, 5.0)).is_none());
    }
}
