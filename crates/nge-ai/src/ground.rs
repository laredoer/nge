//! 走地面的寻路：节点是能站的地方，连线有走、跳、落（含从单向平台穿下去），跳的连线按跳跃能力算（见 docs/gameplay2d.md）。
//!
//! 坐标按格算：x 往右，y 往上，地图左上角是 (0, 0)；格子 `(c, r)` 占 `[c, c+1] × [-(r+1), -r]`。
//! 走地面的角色按不超过一格大小算：站的地方上面要空一格，跳的时候按半宽 0.35、高 0.8 格的方块查碰没碰到。

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use bevy_ecs::component::Component;
use glam::Vec2;

use crate::grid::Cell;

/// 一格是什么。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ground {
    Empty,
    Solid,
    /// 单向平台：从上面能站，从下面、侧面穿过去。
    OneWay,
    /// 斜坡：顶边左右两头的高（格高的几分之几）。
    Slope { left: f32, right: f32 },
}

/// 地形：一行一行从上往下。地图外面是空的（能从两边、底下掉出去）。
#[derive(Debug, Clone, PartialEq)]
pub struct Terrain {
    cols: usize,
    rows: usize,
    tiles: Vec<Ground>,
}

impl Terrain {
    /// `tiles` 长度不是 `cols` 的整数倍时，最后不满一行的丢掉。
    pub fn new(cols: usize, tiles: Vec<Ground>) -> Self {
        let rows = tiles.len().checked_div(cols).unwrap_or(0);
        let mut tiles = tiles;
        tiles.truncate(rows * cols);
        Self { cols, rows, tiles }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn at(&self, (c, r): Cell) -> Ground {
        if c < 0 || r < 0 || c as usize >= self.cols || r as usize >= self.rows {
            return Ground::Empty;
        }
        self.tiles[r as usize * self.cols + c as usize]
    }

    /// 站的地方：这一格上的地面左右两头的高（y 往上）。实心或单向平台上面的空格，或者斜坡格自己（上面一格不能是实心的）。
    fn floor(&self, (c, r): Cell) -> Option<(f32, f32)> {
        let base = -(r as f32 + 1.0);
        match self.at((c, r)) {
            Ground::Empty => matches!(self.at((c, r + 1)), Ground::Solid | Ground::OneWay).then_some((base, base)),
            Ground::Slope { left, right } => (self.at((c, r - 1)) != Ground::Solid).then_some((base + left, base + right)),
            _ => None,
        }
    }

    /// 点 (x, y) 在不在挡住的地方（实心，或者斜坡顶边以下）。单向平台不算。
    fn blocked(&self, p: Vec2) -> bool {
        let cell = (p.x.floor() as i32, (-p.y).floor() as i32);
        match self.at(cell) {
            Ground::Solid => true,
            Ground::Slope { left, right } => {
                let f = p.x - cell.0 as f32;
                p.y < -(cell.1 as f32 + 1.0) + left + (right - left) * f
            }
            _ => false,
        }
    }
}

/// 组件：地形摆在世界里的样子，和 `NavGrid` 一样 `origin` 是左上角、y 朝上、一格边长 `tile`。
#[derive(Component, Debug, Clone, PartialEq)]
pub struct GroundMap {
    pub terrain: Terrain,
    pub origin: Vec2,
    pub tile: f32,
}

impl GroundMap {
    /// 世界坐标换成格坐标（y 往上，地图顶边是 0）。
    pub fn local(&self, p: Vec2) -> Vec2 {
        (p - self.origin) / self.tile
    }

    pub fn world(&self, p: Vec2) -> Vec2 {
        self.origin + p * self.tile
    }

    /// 横着在不在地图里。
    pub fn covers(&self, p: Vec2) -> bool {
        let x = self.local(p).x;
        x >= 0.0 && x < self.terrain.cols as f32
    }
}

/// 跳跃能力（格）：平地上最高跳多高、最远跳多远。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reach {
    pub height: f32,
    pub distance: f32,
}

impl Reach {
    /// 按起跳速度、重力、水平速度算（都是 point 和秒），换成格。
    pub fn from_physics(jump: f32, gravity: f32, speed: f32, tile: f32) -> Self {
        let (g, v) = (gravity.abs().max(1e-3), jump.max(0.0));
        Self { height: v * v / (2.0 * g) / tile, distance: speed.abs() * 2.0 * v / g / tile }
    }

    /// 落点比起跳点高 `dy` 格时，水平最远能跳多远；够不着那么高是 `None`。
    pub fn across(&self, dy: f32) -> Option<f32> {
        (self.height > 0.0 && dy <= self.height).then(|| self.distance * (1.0 + (1.0 - dy / self.height).sqrt()) / 2.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Walk,
    Jump,
    /// 从边上走出去落下。
    Fall,
    /// 从单向平台上穿下去（落的一种）。
    Drop,
}

/// 能站的地方：格子，地面左右两头的高。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spot {
    pub cell: Cell,
    pub left: f32,
    pub right: f32,
}

impl Spot {
    /// 格子中间的地面（格坐标，y 往上）。
    pub fn point(&self) -> Vec2 {
        Vec2::new(self.cell.0 as f32 + 0.5, (self.left + self.right) / 2.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    pub to: usize,
    pub kind: Move,
    pub cost: f32,
    /// 跳的时候跳多高（格，从起跳点量）；别的是 0。
    pub height: f32,
}

/// 一步：从 `from` 站的地方用 `kind` 到 `to`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hop {
    pub from: usize,
    pub to: usize,
    pub kind: Move,
}

/// 角色的方块：半宽、高（格）。
const HALF: f32 = 0.35;
const HEIGHT: f32 = 0.8;
/// 跳的轨迹按多少份取样。
const SAMPLES: usize = 24;
/// 跳的高试满高的几成（先高后低）。
const JUMPS: [f32; 4] = [1.0, 0.75, 0.55, 0.4];
/// 跳的最高点至少比落点高多少（格），才落得上去。
const LIFT: f32 = 0.3;
/// 找路最多展开多少个节点。
pub const MAX_SPOTS: usize = 4096;

/// 地面寻路图。
#[derive(Debug, Clone, PartialEq)]
pub struct GroundGraph {
    pub spots: Vec<Spot>,
    pub links: Vec<Vec<Link>>,
    index: HashMap<Cell, usize>,
    pub reach: Reach,
}

impl GroundGraph {
    pub fn build(terrain: &Terrain, reach: Reach) -> Self {
        let mut spots = Vec::new();
        let mut index = HashMap::new();
        for r in 0..terrain.rows as i32 {
            for c in 0..terrain.cols as i32 {
                if let Some((left, right)) = terrain.floor((c, r)) {
                    index.insert((c, r), spots.len());
                    spots.push(Spot { cell: (c, r), left, right });
                }
            }
        }
        let mut graph = Self { links: vec![Vec::new(); spots.len()], spots, index, reach };
        for i in 0..graph.spots.len() {
            let mut links = graph.walks(i);
            links.extend(graph.falls(terrain, i));
            let walked: Vec<usize> = links.iter().map(|l| l.to).collect();
            links.extend(graph.jumps(terrain, i).into_iter().filter(|l| !walked.contains(&l.to)));
            graph.links[i] = links;
        }
        graph
    }

    pub fn spot(&self, cell: Cell) -> Option<usize> {
        self.index.get(&cell).copied()
    }

    /// 点 `p`（格坐标）脚下的站的地方：同一列往下 `depth` 格以内第一个地面不高于它的（高出一点点也算，角色和地面之间有间隙）。
    pub fn spot_below(&self, p: Vec2, depth: i32) -> Option<usize> {
        let c = p.x.floor() as i32;
        let top = (-p.y - 0.3).floor() as i32;
        (top..=top + depth).find_map(|r| {
            let i = self.spot((c, r))?;
            let s = self.spots[i];
            let floor = s.left + (s.right - s.left) * (p.x - c as f32).clamp(0.0, 1.0);
            (floor <= p.y + 0.3).then_some(i)
        })
    }

    /// `from` 到 `to` 那条跳的连线跳多高（格）；不是跳的是 `None`。
    pub fn jump_height(&self, from: usize, to: usize) -> Option<f32> {
        self.links.get(from)?.iter().find(|l| l.to == to && l.kind == Move::Jump).map(|l| l.height)
    }

    /// 左右相邻、地面接得上的。
    fn walks(&self, i: usize) -> Vec<Link> {
        let s = self.spots[i];
        let (c, r) = s.cell;
        let mut out = Vec::new();
        for (dc, mine) in [(-1, s.left), (1, s.right)] {
            for dr in -1..=1 {
                let Some(j) = self.spot((c + dc, r + dr)) else { continue };
                let o = self.spots[j];
                let theirs = if dc < 0 { o.right } else { o.left };
                if (theirs - mine).abs() < 0.01 {
                    out.push(Link { to: j, kind: Move::Walk, cost: 1.0, height: 0.0 });
                }
            }
        }
        out
    }

    /// 从边上走出去落到下面第一个能站的地方；站在单向平台上的还能穿下去。
    fn falls(&self, terrain: &Terrain, i: usize) -> Vec<Link> {
        let s = self.spots[i];
        let (c, r) = s.cell;
        let mut out = Vec::new();
        let land = |col: i32, from_row: i32, top: f32| -> Option<usize> {
            (from_row..terrain.rows as i32).find_map(|rr| self.spot((col, rr)).filter(|&j| self.spots[j].point().y <= top + 0.01)).filter(|&j| {
                // 一路上没有实心的（单向平台落得穿：最上面那块就是落点）
                let land_row = self.spots[j].cell.1;
                (from_row..land_row).all(|rr| !matches!(terrain.at((col, rr)), Ground::Solid | Ground::Slope { .. }))
            })
        };
        for (dc, edge) in [(-1, s.left), (1, s.right)] {
            let col = c + dc;
            // 旁边那格走得进去（不是实心的），而且没有接得上的地面（接得上的是走）
            let side = terrain.at((col, r));
            if matches!(side, Ground::Solid | Ground::Slope { .. }) || self.walks(i).iter().any(|l| self.spots[l.to].cell.0 == col) {
                continue;
            }
            if let Some(j) = land(col, r, edge) {
                let drop = edge - self.spots[j].point().y;
                out.push(Link { to: j, kind: Move::Fall, cost: 1.0 + drop * 0.5, height: 0.0 });
            }
        }
        if terrain.at((c, r + 1)) == Ground::OneWay
            && let Some(j) = land(c, r + 2, s.point().y)
        {
            let drop = s.point().y - self.spots[j].point().y;
            out.push(Link { to: j, kind: Move::Drop, cost: 1.0 + drop * 0.5, height: 0.0 });
        }
        out
    }

    /// 跳得到的：落点的高和远都在能力以内，一路上不碰头、不撞墙。先试满高，碰头再试矮一点的。
    fn jumps(&self, terrain: &Terrain, i: usize) -> Vec<Link> {
        let a = self.spots[i].point();
        let reach = self.reach;
        let Some(far) = reach.across(-(terrain.rows as f32)) else { return Vec::new() };
        let mut out = Vec::new();
        for (j, s) in self.spots.iter().enumerate() {
            let b = s.point();
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            if j == i || dx.abs() > far || dx.abs() < 0.5 || (dx.abs() < 1.5 && dy.abs() < 0.01) {
                continue;
            }
            let Some(height) = JUMPS.iter().map(|k| reach.height * k).find(|&h| {
                // 矮一点的跳：飞得短，横着也跳不了那么远
                let low = Reach { height: h, distance: reach.distance * (h / reach.height).sqrt() };
                h >= dy + LIFT && low.across(dy).is_some_and(|d| dx.abs() <= d) && clear_jump(terrain, a, b, h)
            }) else {
                continue;
            };
            out.push(Link { to: j, kind: Move::Jump, cost: 1.5 + dx.abs() + dy.abs() * 0.5, height });
        }
        out
    }

    /// 从 `from` 到 `to` 的一步步走法（Dijkstra），走不到是 `None`；起点就是终点时是空的。
    pub fn find(&self, from: usize, to: usize) -> Option<Vec<Hop>> {
        if from >= self.spots.len() || to >= self.spots.len() {
            return None;
        }
        let mut best: HashMap<usize, (f32, Option<(usize, Move)>)> = HashMap::from([(from, (0.0, None))]);
        let mut open = BinaryHeap::from([Open { cost: 0.0, spot: from }]);
        let mut expanded = 0;
        while let Some(Open { cost, spot }) = open.pop() {
            if spot == to {
                let mut hops = Vec::new();
                let mut at = to;
                while let Some((_, Some((prev, kind)))) = best.get(&at) {
                    hops.push(Hop { from: *prev, to: at, kind: *kind });
                    at = *prev;
                }
                hops.reverse();
                return Some(hops);
            }
            if cost > best[&spot].0 {
                continue;
            }
            expanded += 1;
            if expanded > MAX_SPOTS {
                return None;
            }
            for l in &self.links[spot] {
                let c = cost + l.cost;
                if best.get(&l.to).is_none_or(|(old, _)| c < *old) {
                    best.insert(l.to, (c, Some((spot, l.kind))));
                    open.push(Open { cost: c, spot: l.to });
                }
            }
        }
        None
    }
}

/// 跳 `h` 格高、匀速横着走过去的抛物线，一路取样查方块碰没碰到挡住的地方。
fn clear_jump(terrain: &Terrain, a: Vec2, b: Vec2, h: f32) -> bool {
    let dy = b.y - a.y;
    let land = (1.0 + (1.0 - dy / h).max(0.0).sqrt()) / 2.0;
    (1..SAMPLES).all(|k| {
        let t = land * k as f32 / SAMPLES as f32;
        let p = Vec2::new(a.x + (b.x - a.x) * t / land, a.y + 4.0 * h * t * (1.0 - t));
        let falling = t > 0.5;
        // 方块的几个点：底边离地一点、中间、顶
        [0.05, HEIGHT / 2.0, HEIGHT].iter().all(|&up| {
            [-HALF, 0.0, HALF].iter().all(|&side| {
                let q = p + Vec2::new(side, up);
                // 往下落的时候，单向平台也会接住（落点那块除外：落点的地面就在方块底下）
                let caught = falling && up < 0.1 && terrain.at((q.x.floor() as i32, (-q.y).floor() as i32)) == Ground::OneWay && (q.y - b.y).abs() > 0.3;
                !terrain.blocked(q) && !caught
            })
        })
    })
}

#[derive(PartialEq)]
struct Open {
    cost: f32,
    spot: usize,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        other.cost.total_cmp(&self.cost).then(self.spot.cmp(&other.spot))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
#[path = "ground_tests.rs"]
mod tests;
