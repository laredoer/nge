//! 走地面的寻路：节点是能站的地方，连线有走、跳、落（含从单向平台穿下去），跳的连线按跳跃能力算（见 docs/gameplay2d.md）。
//!
//! 坐标按格算：x 往右，y 往上，地图左上角是 (0, 0)；格子 `(c, r)` 占 `[c, c+1] × [-(r+1), -r]`。
//! 站的地方上面要空一格；跳的时候按角色的方块（`Walker` 的半宽、高）查碰没碰到。

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

    /// 在不在地图里：横着在列里，竖着在顶边上面一格到底边之间。
    pub fn covers(&self, p: Vec2) -> bool {
        let l = self.local(p);
        l.x >= 0.0 && l.x < self.terrain.cols as f32 && l.y <= 1.0 && l.y >= -(self.terrain.rows as f32)
    }

    /// 地图底边的世界 y：比它还低就是掉出地图了。
    pub fn bottom(&self) -> f32 {
        self.origin.y - self.terrain.rows as f32 * self.tile
    }

    /// 两点之间有没有挡着（实心格、斜坡顶边以下；单向平台不挡），按四分之一格取样。
    pub fn sees(&self, a: Vec2, b: Vec2) -> bool {
        let (a, b) = (self.local(a), self.local(b));
        let n = ((b - a).length() * 4.0).ceil().max(1.0) as usize;
        (1..n).all(|k| !self.terrain.blocked(a.lerp(b, k as f32 / n as f32)))
    }
}

/// 走地面的角色（格）：跳跃能力，方块的半宽、高，能走上去的最陡的坡（弧度）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Walker {
    pub reach: Reach,
    pub half: f32,
    pub height: f32,
    pub max_slope: f32,
}

impl Walker {
    /// 默认的方块：半宽 0.35、高 0.8 格，坡最陡 50°（和角色控制器的默认一样）。
    pub fn new(reach: Reach) -> Self {
        Self { reach, half: 0.35, height: 0.8, max_slope: 50f32.to_radians() }
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

/// 跳的轨迹取样：两次之间最多隔多远（格），最多取几次。
const STRIDE: f32 = 0.25;
const MAX_SAMPLES: usize = 48;
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
    /// 每一列的站的地方（从上往下），跳的时候只看够得着的几列。
    columns: Vec<Vec<usize>>,
    pub walker: Walker,
}

impl GroundGraph {
    /// 按默认大小的角色建图。
    pub fn build(terrain: &Terrain, reach: Reach) -> Self {
        Self::build_for(terrain, Walker::new(reach))
    }

    pub fn build_for(terrain: &Terrain, walker: Walker) -> Self {
        let mut spots = Vec::new();
        let mut index = HashMap::new();
        let mut columns = vec![Vec::new(); terrain.cols];
        // 比能走的坡还陡的站不住
        let steepest = if walker.max_slope.is_finite() { walker.max_slope.tan() } else { f32::INFINITY };
        for r in 0..terrain.rows as i32 {
            for c in 0..terrain.cols as i32 {
                if let Some((left, right)) = terrain.floor((c, r))
                    && (right - left).abs() <= steepest + 1e-4
                {
                    index.insert((c, r), spots.len());
                    columns[c as usize].push(spots.len());
                    spots.push(Spot { cell: (c, r), left, right });
                }
            }
        }
        let mut graph = Self { links: vec![Vec::new(); spots.len()], spots, index, columns, walker };
        let walks: Vec<Vec<Link>> = (0..graph.spots.len()).map(|i| graph.walks(i)).collect();
        // 走得通的一段地面：同一段里的不用跳
        let mut floor = vec![usize::MAX; graph.spots.len()];
        for start in 0..floor.len() {
            if floor[start] != usize::MAX {
                continue;
            }
            let mut stack = vec![start];
            floor[start] = start;
            while let Some(i) = stack.pop() {
                for l in &walks[i] {
                    if floor[l.to] == usize::MAX {
                        floor[l.to] = start;
                        stack.push(l.to);
                    }
                }
            }
        }
        for (i, mut links) in walks.into_iter().enumerate() {
            links.extend(graph.falls(terrain, i));
            let walked: Vec<usize> = links.iter().map(|l| l.to).collect();
            links.extend(graph.jumps(terrain, i, &|j| walked.contains(&j) || floor[j] == floor[i]));
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
        // 叠着的几行单向平台一起穿（物理那边也一起穿）
        let below = (r + 1..).find(|&rr| terrain.at((c, rr)) != Ground::OneWay).unwrap_or(r + 1);
        if below > r + 1
            && let Some(j) = land(c, below, s.point().y)
        {
            let drop = s.point().y - self.spots[j].point().y;
            out.push(Link { to: j, kind: Move::Drop, cost: 1.0 + drop * 0.5, height: 0.0 });
        }
        out
    }

    /// 跳得到的：落点的高和远都在能力以内，一路上不碰头、不撞墙。先试满高，碰头再试矮一点的。
    fn jumps(&self, terrain: &Terrain, i: usize, skip: &dyn Fn(usize) -> bool) -> Vec<Link> {
        let a = self.spots[i].point();
        let reach = self.walker.reach;
        let Some(far) = reach.across(-(terrain.rows as f32)) else { return Vec::new() };
        let c = self.spots[i].cell.0;
        let span = far.ceil() as i32 + 1;
        let cols = (c - span).max(0)..=(c + span).min(terrain.cols as i32 - 1);
        let mut out = Vec::new();
        // 每列看比起跳点高的（够得着的）和低的里面最上面那个：更低的被它的地面挡着，要去那里是从边上落下去
        let candidates = cols.flat_map(|col| {
            let column = &self.columns[col as usize];
            let lower = column.iter().position(|&j| self.spots[j].point().y < a.y - 0.01).unwrap_or(column.len());
            column[..(lower + 1).min(column.len())].iter().copied()
        });
        for j in candidates {
            let b = self.spots[j].point();
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            if j == i || dy > reach.height || dx.abs() > far || dx.abs() < 0.5 || (dx.abs() < 1.5 && dy.abs() < 0.01) {
                continue;
            }
            // 往下跳的落点那一列，从起跳的高度到落点之间要空着（不跳到伸出来的地面底下）
            let (bc, br) = self.spots[j].cell;
            let ar = self.spots[i].cell.1;
            if skip(j) || (dy < -0.01 && (ar + 1..br).any(|r| terrain.at((bc, r)) != Ground::Empty)) {
                continue;
            }
            let Some(height) = JUMPS.iter().map(|k| reach.height * k).find(|&h| {
                // 矮一点的跳：飞得短，横着也跳不了那么远
                let low = Reach { height: h, distance: reach.distance * (h / reach.height).sqrt() };
                h >= dy + LIFT && low.across(dy).is_some_and(|d| dx.abs() <= d) && clear_jump(terrain, &self.walker, a, b, h)
            }) else {
                continue;
            };
            out.push(Link { to: j, kind: Move::Jump, cost: 1.5 + dx.abs() + dy.abs() * 0.5, height });
        }
        out
    }

    /// 从 `from` 到 `to` 的一步步走法（Dijkstra），走不到是 `None`；起点就是终点时是空的。
    pub fn find(&self, from: usize, to: usize) -> Option<Vec<Hop>> {
        self.find_avoiding(from, to, &|_, _| false)
    }

    /// 同 `find`，不走 `avoid(起点格, 终点格)` 为真的连线（比如试了几次都没跳上去的）。
    pub fn find_avoiding(&self, from: usize, to: usize, avoid: &dyn Fn(Cell, Cell) -> bool) -> Option<Vec<Hop>> {
        if to >= self.spots.len() {
            return None;
        }
        let (best, found) = self.search(from, Some(to), avoid)?;
        found.then(|| Self::hops(&best, to))
    }

    /// 去不了 `p`（格坐标）脚下时用：走到走得到的地方里离 `p` 最近的（横着近的优先，一样近再看竖着）。
    pub fn find_near(&self, from: usize, p: Vec2, avoid: &dyn Fn(Cell, Cell) -> bool) -> Option<Vec<Hop>> {
        let (best, _) = self.search(from, None, avoid)?;
        let far = |i: usize| {
            let q = self.spots[i].point();
            ((q.x - p.x).abs(), (q.y - p.y).abs())
        };
        let to = best.keys().copied().min_by(|&a, &b| {
            let (fa, fb) = (far(a), far(b));
            fa.0.total_cmp(&fb.0).then(fa.1.total_cmp(&fb.1)).then(a.cmp(&b))
        })?;
        Some(Self::hops(&best, to))
    }

    fn hops(best: &HashMap<usize, (f32, Option<(usize, Move)>)>, to: usize) -> Vec<Hop> {
        let mut hops = Vec::new();
        let mut at = to;
        while let Some((_, Some((prev, kind)))) = best.get(&at) {
            hops.push(Hop { from: *prev, to: at, kind: *kind });
            at = *prev;
        }
        hops.reverse();
        hops
    }

    /// 从 `from` 展开（最多 `MAX_SPOTS` 个），找到 `to` 就停；返回每个到过的地方的最短距离和上一步、找没找到。
    #[allow(clippy::type_complexity)]
    fn search(&self, from: usize, to: Option<usize>, avoid: &dyn Fn(Cell, Cell) -> bool) -> Option<(HashMap<usize, (f32, Option<(usize, Move)>)>, bool)> {
        if from >= self.spots.len() {
            return None;
        }
        let mut best: HashMap<usize, (f32, Option<(usize, Move)>)> = HashMap::from([(from, (0.0, None))]);
        let mut open = BinaryHeap::from([Open { cost: 0.0, spot: from }]);
        let mut expanded = 0;
        while let Some(Open { cost, spot }) = open.pop() {
            if Some(spot) == to {
                return Some((best, true));
            }
            if cost > best[&spot].0 {
                continue;
            }
            expanded += 1;
            if expanded > MAX_SPOTS {
                // 找某一点时展开太多算找不到；找最近的时用展开过的
                return to.is_none().then_some((best, false));
            }
            for l in &self.links[spot] {
                if avoid(self.spots[spot].cell, self.spots[l.to].cell) {
                    continue;
                }
                let c = cost + l.cost;
                if best.get(&l.to).is_none_or(|(old, _)| c < *old) {
                    best.insert(l.to, (c, Some((spot, l.kind))));
                    open.push(Open { cost: c, spot: l.to });
                }
            }
        }
        to.is_none().then_some((best, false))
    }
}

/// 跳 `h` 格高、匀速横着走过去的抛物线，一路取样查方块碰没碰到挡住的地方。
fn clear_jump(terrain: &Terrain, walker: &Walker, a: Vec2, b: Vec2, h: f32) -> bool {
    let (half, height) = (walker.half.max(0.0), walker.height.max(0.0));
    let dy = b.y - a.y;
    let land = (1.0 + (1.0 - dy / h).max(0.0).sqrt()) / 2.0;
    // 取样密度：两次取样之间横竖都不超过 STRIDE 格
    let span = ((b.x - a.x).abs() / land).max(4.0 * h * land);
    let samples = ((span / STRIDE).ceil() as usize).clamp(4, MAX_SAMPLES);
    let ups = [0.05, height / 2.0, height];
    let sides = [-half, 0.0, half];
    // 方块盖住的格子全是空的就不用逐点查；和上一次取样盖住的一样时直接跳过
    let mut empty = None;
    for k in 1..samples {
        let t = land * k as f32 / samples as f32;
        let p = Vec2::new(a.x + (b.x - a.x) * t / land, a.y + 4.0 * h * t * (1.0 - t));
        let cells = ((p.x - half).floor() as i32, (p.x + half).floor() as i32, (-(p.y + height)).floor() as i32, (-(p.y + 0.05)).floor() as i32);
        if empty == Some(cells) {
            continue;
        }
        let (c0, c1, r0, r1) = cells;
        if (r0..=r1).all(|r| (c0..=c1).all(|c| terrain.at((c, r)) == Ground::Empty)) {
            empty = Some(cells);
            continue;
        }
        empty = None;
        let falling = t > 0.5;
        // 方块的几个点：底边离地一点、中间、顶
        for up in ups {
            for side in sides {
                let q = Vec2::new(p.x + side, p.y + up);
                if terrain.blocked(q) {
                    return false;
                }
                // 往下落的时候，单向平台也会接住（落点那块除外：落点的地面就在方块底下）
                if falling && up < 0.1 && (q.y - b.y).abs() > 0.3 && terrain.at((q.x.floor() as i32, (-q.y).floor() as i32)) == Ground::OneWay {
                    return false;
                }
            }
        }
    }
    true
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
