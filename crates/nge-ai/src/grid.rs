//! 格子寻路：A*（八个方向，不切墙角），直线能不能走通，路径拉直。

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use glam::Vec2;

/// 最多展开多少个格子，超过算走不到，免得大地图上卡一帧。
pub const MAX_EXPANDED: usize = 4096;

/// 格子：一行一行从上往下，`(列, 行)`。地图外面算挡住。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grid {
    cols: usize,
    rows: usize,
    blocked: Vec<bool>,
}

pub type Cell = (i32, i32);

impl Grid {
    /// `blocked` 长度不是 `cols` 的整数倍时，最后不满一行的丢掉。
    pub fn new(cols: usize, blocked: Vec<bool>) -> Self {
        let rows = blocked.len().checked_div(cols).unwrap_or(0);
        let mut blocked = blocked;
        blocked.truncate(rows * cols);
        Self { cols, rows, blocked }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn open(&self, (c, r): Cell) -> bool {
        c >= 0 && r >= 0 && (c as usize) < self.cols && (r as usize) < self.rows && !self.blocked[r as usize * self.cols + c as usize]
    }
}

const STRAIGHT: f32 = 1.0;
const DIAGONAL: f32 = std::f32::consts::SQRT_2;

/// 八个方向的距离下界：先斜着走、再直着走。
fn octile(a: Cell, b: Cell) -> f32 {
    let (dx, dy) = ((a.0 - b.0).abs() as f32, (a.1 - b.1).abs() as f32);
    STRAIGHT * (dx + dy) + (DIAGONAL - 2.0 * STRAIGHT) * dx.min(dy)
}

#[derive(PartialEq)]
struct Open {
    f: f32,
    g: f32,
    cell: Cell,
}

impl Eq for Open {}

impl Ord for Open {
    // BinaryHeap 是大顶堆：f 小的排前面；一样时 g 大的（离终点近的）先
    fn cmp(&self, other: &Self) -> Ordering {
        other.f.total_cmp(&self.f).then(self.g.total_cmp(&other.g))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A*：从 `from` 到 `to` 的最短格子路径，两头都在里面。起点或终点挡住了、走不到、展开超过 [`MAX_EXPANDED`] 时是 `None`。
/// 斜着走时旁边两格都要是空的。
pub fn find_path(grid: &Grid, from: Cell, to: Cell) -> Option<Vec<Cell>> {
    if !grid.open(from) || !grid.open(to) {
        return None;
    }
    // 只记碰到过的格子（到这里的最短距离、从哪来），开销跟着展开的格子数走，不跟地图大小走
    let mut best: HashMap<Cell, (f32, Option<Cell>)> = HashMap::from([(from, (0.0, None))]);
    let cost = |best: &HashMap<Cell, (f32, Option<Cell>)>, c: Cell| best.get(&c).map_or(f32::INFINITY, |b| b.0);
    let mut open = BinaryHeap::new();
    open.push(Open { f: octile(from, to), g: 0.0, cell: from });
    let mut expanded = 0;
    while let Some(Open { g, cell, .. }) = open.pop() {
        if cell == to {
            let mut path = vec![to];
            while let Some(prev) = best[path.last().expect("非空")].1 {
                path.push(prev);
            }
            path.reverse();
            return Some(path);
        }
        if g > cost(&best, cell) {
            continue;
        }
        expanded += 1;
        if expanded > MAX_EXPANDED {
            return None;
        }
        for dc in -1..=1 {
            for dr in -1..=1 {
                if (dc, dr) == (0, 0) {
                    continue;
                }
                let next = (cell.0 + dc, cell.1 + dr);
                let diagonal = dc != 0 && dr != 0;
                if !grid.open(next) || (diagonal && !(grid.open((cell.0 + dc, cell.1)) && grid.open((cell.0, cell.1 + dr)))) {
                    continue;
                }
                let g = g + if diagonal { DIAGONAL } else { STRAIGHT };
                if g < cost(&best, next) {
                    best.insert(next, (g, Some(cell)));
                    open.push(Open { f: g + octile(next, to), g, cell: next });
                }
            }
        }
    }
    None
}

/// 取样的步长（格）；方块比这还小时按半径取，最小 0.01 格。
const STEP: f32 = 0.1;

/// 从 `a` 走直线到 `b`（格子坐标，格子 `(c, r)` 占 `[c, c+1) × [r, r+1)`），半边长 `radius` 的方块一路上碰不碰到挡住的格子。
pub fn clear(grid: &Grid, a: Vec2, b: Vec2, radius: f32) -> bool {
    let step = STEP.min(radius.max(0.01));
    let steps = (a.distance(b) / step).ceil().max(1.0) as usize;
    (0..=steps).all(|i| {
        let p = a.lerp(b, i as f32 / steps as f32);
        // 方块碰到的格子：边正好落在格线上的不算碰到下一格
        let (lo, hi) = (p - radius, p + radius);
        let (c0, r0) = (lo.x.floor() as i32, lo.y.floor() as i32);
        let (c1, r1) = ((hi.x.ceil() as i32 - 1).max(c0), (hi.y.ceil() as i32 - 1).max(r0));
        (c0..=c1).all(|c| (r0..=r1).all(|r| grid.open((c, r))))
    })
}

/// 格子的中心（格子坐标）。
pub fn middle((c, r): Cell) -> Vec2 {
    Vec2::new(c as f32 + 0.5, r as f32 + 0.5)
}

/// 拉直：从当前点往后，一直能直线走到（[`clear`]）的就接着往后看，走不通了就在上一点拐弯。两头留着。
pub fn smooth(grid: &Grid, path: &[Cell], radius: f32) -> Vec<Cell> {
    let Some(&first) = path.first() else { return Vec::new() };
    let mut out = vec![first];
    let mut at = 0;
    while at + 1 < path.len() {
        let mut far = at + 1;
        while far + 1 < path.len() && clear(grid, middle(path[at]), middle(path[far + 1]), radius) {
            far += 1;
        }
        out.push(path[far]);
        at = far;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `#` 挡住，别的空着。
    fn grid(rows: &[&str]) -> Grid {
        Grid::new(rows[0].len(), rows.iter().flat_map(|r| r.chars().map(|c| c == '#')).collect())
    }

    fn length(path: &[Cell]) -> f32 {
        path.windows(2).map(|w| octile(w[0], w[1])).sum()
    }

    #[test]
    fn goes_around_a_wall_on_the_shortest_route() {
        let g = grid(&["....", ".##.", ".#..", "...."]);
        let path = find_path(&g, (0, 0), (2, 2)).expect("有路");
        assert_eq!((path[0], *path.last().unwrap()), ((0, 0), (2, 2)));
        assert!(path.iter().all(|&c| g.open(c)));
        assert!(path.windows(2).all(|w| (w[0].0 - w[1].0).abs() <= 1 && (w[0].1 - w[1].1).abs() <= 1), "一步一格 {path:?}");
        // 两边绕都是 6 步：斜着走都要切过墙角
        assert!((length(&path) - 6.0).abs() < 1e-4, "{path:?}");
        assert_eq!(find_path(&g, (0, 3), (3, 3)).unwrap().len(), 4, "空旷处走直线");
        assert!((length(&find_path(&g, (3, 0), (0, 2)).unwrap()) - 5.0).abs() < 1e-4, "从左边绕下来");
        let open = grid(&["....", "....", "...."]);
        assert!((length(&find_path(&open, (0, 0), (3, 2)).unwrap()) - (2.0 * DIAGONAL + 1.0)).abs() < 1e-4, "能斜着走就斜着走");
    }

    #[test]
    fn diagonal_steps_do_not_squeeze_past_a_corner() {
        let g = grid(&[".#", "#."]);
        assert_eq!(find_path(&g, (0, 0), (1, 1)), None, "两个实心格子的墙角挤不过去");
        let g = grid(&["..", "#."]);
        assert_eq!(find_path(&g, (0, 0), (1, 1)), Some(vec![(0, 0), (1, 0), (1, 1)]), "一边有墙也不斜着切");
        let g = grid(&["..", ".."]);
        assert_eq!(find_path(&g, (0, 0), (1, 1)), Some(vec![(0, 0), (1, 1)]), "两边都空才斜着走");
    }

    #[test]
    fn no_path_when_blocked_walled_in_or_too_far() {
        let g = grid(&["...", ".#.", "..."]);
        assert_eq!(find_path(&g, (0, 0), (1, 1)), None, "终点挡住");
        assert_eq!(find_path(&g, (1, 1), (0, 0)), None, "起点挡住");
        assert_eq!(find_path(&g, (0, 0), (5, 0)), None, "地图外面");
        let walled = grid(&["#####", "#...#", "#####", "....."]);
        assert_eq!(find_path(&walled, (1, 1), (0, 3)), None, "被围住");
        assert_eq!(find_path(&g, (2, 2), (2, 2)), Some(vec![(2, 2)]), "起点就是终点");
        // 展开上限：一条很长的空走廊，终点在最后
        let long = Grid::new(MAX_EXPANDED + 10, vec![false; MAX_EXPANDED + 10]);
        assert_eq!(find_path(&long, (0, 0), (MAX_EXPANDED as i32 + 9, 0)), None);
        assert!(find_path(&long, (0, 0), (MAX_EXPANDED as i32 - 10, 0)).is_some());
    }

    #[test]
    fn the_heuristic_keeps_the_search_narrow() {
        // 空旷的大地图上走直线：好的启发函数只展开路上的格子，到得了终点；
        // 不用启发函数（全按 0）要展开半个地图，超过上限
        let g = Grid::new(200, vec![false; 200 * 60]);
        assert_eq!(find_path(&g, (0, 30), (199, 30)).map(|p| p.len()), Some(200));
    }

    #[test]
    fn clear_respects_the_radius_at_corners() {
        let g = grid(&["...", ".#.", "..."]);
        assert!(clear(&g, Vec2::new(0.5, 0.5), Vec2::new(2.5, 0.5), 0.4), "沿着墙上面走");
        assert!(!clear(&g, Vec2::new(0.5, 0.5), Vec2::new(2.5, 2.5), 0.1), "穿过墙");
        assert!(!clear(&g, Vec2::new(0.5, 0.5), Vec2::new(2.5, 0.9), 0.4), "擦着墙角，按半径算碰到");
        assert!(clear(&g, Vec2::new(0.5, 0.5), Vec2::new(2.5, 0.9), 0.05), "细的擦得过去");
        assert!(clear(&g, Vec2::new(0.5, 0.5), Vec2::new(0.5, 0.5), 0.5), "边正好在格线上不算碰到");
        assert!(!clear(&g, Vec2::new(0.5, 0.5), Vec2::new(-0.5, 0.5), 0.4), "地图外面");
        assert!(!clear(&g, Vec2::new(0.5, 1.55), Vec2::new(1.55, 0.5), 0.01), "细线只擦过墙角一点点也算碰到（按半边长取样）");
    }

    #[test]
    fn smooth_keeps_only_the_turns() {
        let open = grid(&["....", "....", "...."]);
        let path = find_path(&open, (0, 0), (3, 2)).unwrap();
        assert_eq!(smooth(&open, &path, 0.4), vec![(0, 0), (3, 2)], "空旷处拉成一段");
        let wall = grid(&["....", "###.", "...."]);
        let path = find_path(&wall, (0, 0), (0, 2)).unwrap();
        let s = smooth(&wall, &path, 0.4);
        assert_eq!((s[0], *s.last().unwrap()), ((0, 0), (0, 2)));
        assert!(s.len() >= 3 && s.len() < path.len(), "拐弯处留下拐点 {s:?}");
        assert!(s.windows(2).all(|w| clear(&wall, middle(w[0]), middle(w[1]), 0.4)), "每段都走得通 {s:?}");
        assert_eq!(smooth(&wall, &[], 0.4), Vec::<Cell>::new());
        assert_eq!(smooth(&wall, &[(0, 0)], 0.4), vec![(0, 0)]);
    }

    #[test]
    fn short_data_drops_the_partial_row() {
        let g = Grid::new(3, vec![false; 7]);
        assert_eq!((g.cols(), g.rows()), (3, 2));
        assert!(!g.open((0, 2)) && Grid::new(0, vec![true]).rows() == 0);
    }
}
