//! 导航网格（格子摆在世界里）和追击：看见目标就绕开墙追，追丢了、追太远了回出发点。

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_ecs::Time;
use nge_render2d::Transform2d;

use crate::fsm::Fsm;
use crate::grid::{Cell, Grid, clear, find_path, middle, smooth};

/// 追击的角色按多大算碰墙：半边长 0.4 格，适合比一格小一点的角色。
pub const AGENT_RADIUS: f32 = 0.4;
/// 追的时候隔多久（秒）重新找一次路。
pub const REPLAN: f32 = 0.3;
/// 目标跑出 `sight` 的几倍算追丢。
pub const LOSE: f32 = 1.5;
/// 离出发点多近（像素）算到家。
pub const ARRIVE: f32 = 2.0;
/// 回到家后歇多久（秒）才会再追：免得在追得到和追不到的边上来回折腾。
pub const REST: f32 = 1.0;
/// 看不看得见按多粗的视线算（格）：斜着穿过两块实心格子的墙角算挡住。
const SIGHT: f32 = 0.05;

/// 组件：格子摆在世界里的样子。`origin` 是左上角，y 朝上，一格边长 `tile`。
#[derive(Component, Debug, Clone, PartialEq)]
pub struct NavGrid {
    pub grid: Grid,
    pub origin: Vec2,
    pub tile: f32,
}

impl NavGrid {
    /// 世界坐标换成格子坐标（带小数，格子 `(c, r)` 占 `[c, c+1) × [r, r+1)`）。
    fn local(&self, p: Vec2) -> Vec2 {
        Vec2::new(p.x - self.origin.x, self.origin.y - p.y) / self.tile
    }

    pub fn cell(&self, p: Vec2) -> Cell {
        let l = self.local(p).floor();
        (l.x as i32, l.y as i32)
    }

    pub fn center(&self, (c, r): Cell) -> Vec2 {
        self.origin + Vec2::new(c as f32 + 0.5, -(r as f32 + 0.5)) * self.tile
    }

    pub fn contains(&self, p: Vec2) -> bool {
        let l = self.local(p);
        l.x >= 0.0 && l.y >= 0.0 && l.x < self.grid.cols() as f32 && l.y < self.grid.rows() as f32
    }

    /// 两点之间的直线有没有墙挡着（和找路一样，斜着穿过墙角算挡住）。
    pub fn sees(&self, a: Vec2, b: Vec2) -> bool {
        clear(&self.grid, self.local(a), self.local(b), SIGHT)
    }

    /// 从 `from` 到 `to` 的路线（世界坐标）：找路、拉直。第一个点是下一步要去的地方，最后一个点是 `to` 本身。走不到是 `None`。
    pub fn path(&self, from: Vec2, to: Vec2, radius: f32) -> Option<Vec<Vec2>> {
        let cells = smooth(&self.grid, &find_path(&self.grid, self.cell(from), self.cell(to))?, radius);
        let mut points: Vec<Vec2> = cells.iter().map(|&c| self.center(c)).collect();
        // 起点格子的中心：能直线走到下一点就不用先回中心
        if points.len() > 1 && clear(&self.grid, self.local(from), middle(cells[1]), radius) {
            points.remove(0);
        }
        // 最后一段走到 `to` 本身：从上一点能直线走到就换掉终点格子的中心，不然经过中心再过去
        let before = if points.len() > 1 { self.local(points[points.len() - 2]) } else { self.local(from) };
        let last = points.last_mut().expect("至少有起点");
        if clear(&self.grid, before, self.local(to), radius) {
            *last = to;
        } else if *last != to {
            points.push(to);
        }
        Some(points)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChaseState {
    /// 在家等着。
    Idle,
    Chase,
    /// 回出发点。
    Return,
}

/// 消息：追击的状态变了。
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ChaseChanged {
    pub entity: Entity,
    pub state: ChaseState,
    /// 变的时候在哪。
    pub at: Vec2,
}

/// 组件：看见 `target` 就追（绕开所在 [`NavGrid`] 的墙），目标跑远了、自己离家超过 `leash`、找不到路就回 `home`。
/// 直接挪 `Transform2d`：配 kinematic 刚体时不会被墙挡住，路线本身绕开了墙。
#[derive(Component, Debug, Clone)]
pub struct Chaser {
    pub target: Option<Entity>,
    /// 每秒多少像素。
    pub speed: f32,
    /// 多远能看见目标（中间不能有墙）。
    pub sight: f32,
    /// 离出发点多远就不追了。
    pub leash: f32,
    pub home: Vec2,
    fsm: Fsm<ChaseState>,
    path: Vec<Vec2>,
    replan: f32,
}

impl Chaser {
    pub fn new(home: Vec2, speed: f32, sight: f32, leash: f32) -> Self {
        // 一开始就歇够了，看见就追
        let mut fsm = Fsm::new(ChaseState::Idle);
        fsm.tick(REST);
        Self { target: None, speed, sight, leash, home, fsm, path: Vec::new(), replan: 0.0 }
    }

    pub fn state(&self) -> ChaseState {
        self.fsm.state()
    }

    /// 还要走的路线（世界坐标）。
    pub fn path(&self) -> &[Vec2] {
        &self.path
    }

    /// 这一帧该在什么状态。
    fn decide(&self, at: Vec2, target: Option<Vec2>, grid: Option<&NavGrid>) -> ChaseState {
        match self.state() {
            ChaseState::Idle => match target {
                Some(t) if self.fsm.time() >= REST && at.distance(t) <= self.sight && grid.is_none_or(|g| g.sees(at, t)) => ChaseState::Chase,
                // 不在家（比如被挪走了）就回去
                _ if at.distance(self.home) > ARRIVE => ChaseState::Return,
                _ => ChaseState::Idle,
            },
            ChaseState::Chase => match target {
                Some(t) if at.distance(t) <= self.sight * LOSE && at.distance(self.home) <= self.leash => ChaseState::Chase,
                _ => ChaseState::Return,
            },
            // 走完了路线才算到家，不差那最后一点
            ChaseState::Return if self.path.is_empty() && at.distance(self.home) <= ARRIVE => ChaseState::Idle,
            ChaseState::Return => ChaseState::Return,
        }
    }
}

/// 沿路线走 `step` 像素，走到的点从路线里去掉。
fn walk(mut at: Vec2, path: &mut Vec<Vec2>, mut step: f32) -> Vec2 {
    while step > 0.0 && !path.is_empty() {
        let to = path[0] - at;
        let length = to.length();
        if length <= step {
            at = path.remove(0);
            step -= length;
        } else {
            at += to / length * step;
            step = 0.0;
        }
    }
    at
}

pub fn chase(
    time: Res<Time>,
    grids: Query<&NavGrid>,
    targets: Query<&Transform2d, Without<Chaser>>,
    mut chasers: Query<(Entity, &mut Chaser, &mut Transform2d)>,
    mut changed: MessageWriter<ChaseChanged>,
) {
    let dt = time.delta;
    for (entity, mut chaser, mut transform) in &mut chasers {
        let c = &mut *chaser;
        let at = transform.translation;
        let grid = grids.iter().find(|g| g.contains(at));
        let target = c.target.and_then(|e| targets.get(e).ok()).map(|t| t.translation);
        c.fsm.tick(dt);
        let mut next = c.decide(at, target, grid);
        loop {
            // 换了状态马上重新找路（回到 Idle 时路线已经走完了）
            if c.fsm.go(next) {
                c.replan = 0.0;
                changed.write(ChaseChanged { entity, state: next, at });
            }
            let goal = match next {
                ChaseState::Chase => target,
                ChaseState::Return => Some(c.home),
                ChaseState::Idle => None,
            };
            c.replan -= dt;
            let Some(goal) = goal.filter(|_| c.replan <= 0.0) else { break };
            // 出发点不会动，回家只找一次路
            c.replan = if next == ChaseState::Chase { REPLAN } else { f32::INFINITY };
            match grid.map_or(Some(vec![goal]), |g| g.path(at, goal, crate::AGENT_RADIUS)) {
                Some(path) => c.path = path,
                // 追不到就回家（这一帧就开始找回家的路）；回家的路找不到就直线飘回去
                None if next == ChaseState::Chase => {
                    next = ChaseState::Return;
                    continue;
                }
                None => c.path = vec![goal],
            }
            break;
        }
        let to = walk(at, &mut c.path, c.speed * dt);
        if to != at {
            transform.translation = to;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy_ecs::message::Messages;
    use nge_core::Viewport;
    use nge_ecs::App;

    use super::*;
    use crate::AiPlugin;

    const VIEWPORT: Viewport = Viewport { size: [100.0, 100.0], pixels: [100, 100] };
    const DT: f32 = 1.0 / 60.0;

    /// `#` 挡住。一格 10 像素，左上角在 (0, 0)：格子 (c, r) 的中心是 (10c + 5, -10r - 5)。
    fn nav(rows: &[&str]) -> NavGrid {
        NavGrid { grid: Grid::new(rows[0].len(), rows.iter().flat_map(|r| r.chars().map(|c| c == '#')).collect()), origin: Vec2::ZERO, tile: 10.0 }
    }

    fn at(x: f32, y: f32) -> Transform2d {
        Transform2d { translation: Vec2::new(x, y), ..Default::default() }
    }

    struct World {
        app: App,
        ghost: Entity,
        target: Entity,
    }

    fn world(rows: &[&str], home: Vec2, target: Vec2, chaser: impl FnOnce(Chaser) -> Chaser) -> World {
        let mut app = App::new();
        app.add_plugins(AiPlugin);
        let w = app.world_mut();
        w.spawn(nav(rows));
        let target = w.spawn(at(target.x, target.y)).id();
        let ghost = w.spawn((at(home.x, home.y), chaser(Chaser { target: Some(target), ..Chaser::new(home, 100.0, 60.0, 200.0) }))).id();
        World { app, ghost, target }
    }

    impl World {
        fn run(&mut self, frames: usize) -> Vec<ChaseState> {
            let mut states = Vec::new();
            for _ in 0..frames {
                self.app.tick(VIEWPORT, DT);
                states.extend(self.app.world_mut().resource_mut::<Messages<ChaseChanged>>().drain().map(|m| {
                    assert_eq!(m.entity, self.ghost);
                    m.state
                }));
            }
            states
        }
        fn ghost(&self) -> Vec2 {
            self.app.world().get::<Transform2d>(self.ghost).unwrap().translation
        }
        fn state(&self) -> ChaseState {
            self.app.world().get::<Chaser>(self.ghost).unwrap().state()
        }
        fn move_target(&mut self, x: f32, y: f32) {
            self.app.world_mut().get_mut::<Transform2d>(self.target).unwrap().translation = Vec2::new(x, y);
        }
        /// 每帧检查幽灵没进墙。
        fn run_inside(&mut self, frames: usize, nav: &NavGrid) {
            for _ in 0..frames {
                self.run(1);
                let p = self.ghost();
                assert!(nav.grid.open(nav.cell(p)), "进了墙 {p}");
            }
        }
    }

    #[test]
    fn nav_grid_converts_between_world_and_cells() {
        let n = NavGrid { origin: Vec2::new(100.0, 50.0), tile: 10.0, ..nav(&["...", "..."]) };
        assert_eq!(n.cell(Vec2::new(115.0, 45.0)), (1, 0));
        assert_eq!(n.cell(Vec2::new(129.0, 31.0)), (2, 1));
        assert_eq!(n.center((2, 1)), Vec2::new(125.0, 35.0));
        assert!(n.contains(Vec2::new(100.0, 50.0)) && n.contains(Vec2::new(129.9, 30.1)));
        assert!(!n.contains(Vec2::new(130.0, 40.0)) && !n.contains(Vec2::new(99.0, 40.0)) && !n.contains(Vec2::new(110.0, 51.0)) && !n.contains(Vec2::new(110.0, 30.0)));
    }

    #[test]
    fn a_path_skips_the_start_cell_and_ends_at_the_goal() {
        let n = nav(&["....", "##..", "...."]);
        let p = n.path(Vec2::new(6.0, -4.0), Vec2::new(7.0, -26.0), AGENT_RADIUS).unwrap();
        assert_eq!(*p.last().unwrap(), Vec2::new(7.0, -26.0), "最后一个点是目标本身");
        assert!(p.len() >= 2, "绕过墙 {p:?}");
        assert_ne!(p[0], n.center((0, 0)), "能直线走到下一点就不回起点格子的中心 {p:?}");
        assert_eq!(n.path(Vec2::new(5.0, -5.0), Vec2::new(5.0, -15.0), AGENT_RADIUS), None, "终点在墙里");
    }

    #[test]
    fn starts_chasing_only_when_the_target_is_seen() {
        // 目标在 sight 以内，但隔着墙
        let rows = ["......", "..#...", "..#...", "......"];
        let mut w = world(&rows, Vec2::new(5.0, -15.0), Vec2::new(45.0, -15.0), |c| c);
        assert_eq!(w.run(10), []);
        assert_eq!((w.state(), w.ghost()), (ChaseState::Idle, Vec2::new(5.0, -15.0)), "隔着墙看不见");
        // 目标走到看得见又在 sight 以内的地方
        w.move_target(5.0, -35.0);
        assert_eq!(w.run(1), [ChaseState::Chase]);
        // 看得见，但在 sight 以外
        let mut far = world(&rows, Vec2::new(5.0, -5.0), Vec2::new(5.0, -35.0), |c| Chaser { sight: 25.0, ..c });
        assert_eq!(far.run(10), [], "太远了");
        far.move_target(5.0, -25.0);
        assert_eq!(far.run(1), [ChaseState::Chase]);
    }

    #[test]
    fn chases_around_a_wall_without_entering_it() {
        let rows = ["......", "####..", "......", "......"];
        let n = nav(&rows);
        // 先在同一行看见目标、开始追
        let mut w = world(&rows, Vec2::new(5.0, -5.0), Vec2::new(45.0, -5.0), |c| Chaser { sight: 100.0, ..c });
        w.run_inside(1, &n);
        assert_eq!(w.state(), ChaseState::Chase);
        // 目标挪到墙正下方：要绕过右边的缺口
        w.move_target(5.0, -25.0);
        w.run_inside(120, &n);
        assert!(w.ghost().distance(Vec2::new(5.0, -25.0)) < 1.0, "绕过来追上了 {}", w.ghost());
    }

    #[test]
    fn gives_up_when_the_target_runs_off_and_goes_home() {
        let rows = [".........."];
        let mut w = world(&rows, Vec2::new(5.0, -5.0), Vec2::new(45.0, -5.0), |c| Chaser { speed: 50.0, ..c });
        assert_eq!(w.run(1), [ChaseState::Chase]);
        w.run(20);
        assert!(w.ghost().x > 10.0, "追过去了 {}", w.ghost());
        // 目标跑出 sight × 1.5 = 90
        w.move_target(5.0 + 200.0, -5.0);
        assert_eq!(w.run(1), [ChaseState::Return]);
        let states = w.run(120);
        assert_eq!(states, [ChaseState::Idle], "到家回到 Idle");
        assert_eq!(w.ghost(), Vec2::new(5.0, -5.0), "回到出发点");
        // 没有目标也回家
        let mut lost = world(&rows, Vec2::new(5.0, -5.0), Vec2::new(45.0, -5.0), |c| c);
        lost.run(10);
        lost.app.world_mut().despawn(lost.target);
        assert_eq!(lost.run(1), [ChaseState::Return]);
    }

    #[test]
    fn the_leash_stops_a_chase_far_from_home() {
        let rows = [".............................."];
        // 目标一直在前面一点，幽灵离家超过 leash 就回去
        let mut w = world(&rows, Vec2::new(5.0, -5.0), Vec2::new(45.0, -5.0), |c| Chaser { leash: 60.0, speed: 120.0, ..c });
        let mut states = Vec::new();
        for _ in 0..120 {
            let g = w.ghost();
            w.move_target(g.x + 30.0, -5.0);
            states.extend(w.run(1));
            if states.contains(&ChaseState::Return) {
                break;
            }
        }
        assert_eq!(states, [ChaseState::Chase, ChaseState::Return]);
        assert!((w.ghost().x - 5.0 - 60.0).abs() < 3.0, "超过 leash 那一刻掉头 {}", w.ghost());
    }

    #[test]
    fn unreachable_targets_send_it_home_and_replans_follow_the_target() {
        // 目标在被围住的格子旁边的墙里（终点挡住）：看得见、找不到路
        let rows = ["....#", "....."];
        let mut w = world(&rows, Vec2::new(5.0, -5.0), Vec2::new(45.0, -5.0), |c| c);
        // 目标在墙里看不见：先让它在看得见的地方开始追
        w.move_target(35.0, -5.0);
        assert_eq!(w.run(1), [ChaseState::Chase]);
        w.move_target(45.0, -5.0);
        let states = w.run(30);
        assert_eq!(states.first(), Some(&ChaseState::Return), "到了重新找路的时候找不到路 {states:?}");
        // 重新找路：目标挪了，过一会儿路线的终点跟着换
        let mut w = world(&[".........."], Vec2::new(5.0, -5.0), Vec2::new(45.0, -5.0), |c| Chaser { speed: 1.0, ..c });
        w.run(1);
        w.move_target(35.0, -5.0);
        w.run(1);
        let end = |w: &World| *w.app.world().get::<Chaser>(w.ghost).unwrap().path().last().unwrap();
        assert_eq!(end(&w), Vec2::new(45.0, -5.0), "还没到重新找路的时候");
        w.run((REPLAN / DT) as usize + 1);
        assert_eq!(end(&w), Vec2::new(35.0, -5.0));
    }

    #[test]
    fn sight_and_paths_agree_on_corners() {
        let n = nav(&[".#", "#."]);
        assert!(!n.sees(Vec2::new(5.0, -5.0), Vec2::new(15.0, -15.0)), "斜着穿过两块墙的墙角算挡住");
        assert!(n.path(Vec2::new(5.0, -5.0), Vec2::new(15.0, -15.0), AGENT_RADIUS).is_none());
        // 每一段都按半径走得通，包括走到目标本身的最后一段（目标贴着墙）
        let n = nav(&["...", "#..", "..."]);
        for to in [Vec2::new(11.0, -15.0), Vec2::new(13.0, -25.0), Vec2::new(25.0, -15.0), Vec2::new(11.0, -25.0)] {
            let from = Vec2::new(5.0, -5.0);
            let p = n.path(from, to, AGENT_RADIUS).unwrap();
            assert_eq!(*p.last().unwrap(), to);
            // 目标自己贴着墙（方块压到墙）时最后一段怎么走都碰，只检查前面的
            let end = if clear(&n.grid, n.local(to), n.local(to), AGENT_RADIUS) { p.len() } else { p.len() - 1 };
            let mut prev = from;
            for &q in &p[..end] {
                assert!(clear(&n.grid, n.local(prev), n.local(q), AGENT_RADIUS), "{prev} → {q} {p:?}");
                prev = q;
            }
        }
    }

    #[test]
    fn rests_at_home_before_chasing_again() {
        let rows = [".........."];
        let mut w = world(&rows, Vec2::new(5.0, -5.0), Vec2::new(45.0, -5.0), |c| c);
        assert_eq!(w.run(1), [ChaseState::Chase], "一开始看见就追");
        w.move_target(500.0, -5.0);
        (0..120).find(|_| w.run(1).contains(&ChaseState::Idle)).expect("回到家");
        w.move_target(45.0, -5.0);
        assert_eq!(w.run((REST / DT) as usize - 5), [], "刚回家，歇一会儿");
        assert_eq!(w.run(10), [ChaseState::Chase]);
    }

    #[test]
    fn idle_away_from_home_goes_back() {
        let rows = [".........."];
        let mut w = world(&rows, Vec2::new(5.0, -5.0), Vec2::new(500.0, -5.0), |c| c);
        w.run(1);
        w.app.world_mut().get_mut::<Transform2d>(w.ghost).unwrap().translation = Vec2::new(65.0, -5.0);
        assert_eq!(w.run(1), [ChaseState::Return], "被挪走了就回去");
        assert_eq!(w.run(60), [ChaseState::Idle]);
        assert_eq!(w.ghost(), Vec2::new(5.0, -5.0));
    }

    #[test]
    fn returns_home_around_walls() {
        let rows = ["......", "####..", "......"];
        let n = nav(&rows);
        let mut w = world(&rows, Vec2::new(5.0, -25.0), Vec2::new(5.0, -5.0), |c| c);
        // 被挪到墙上面，目标不在
        w.app.world_mut().despawn(w.target);
        w.app.world_mut().get_mut::<Transform2d>(w.ghost).unwrap().translation = Vec2::new(5.0, -5.0);
        w.run_inside(120, &n);
        assert_eq!((w.state(), w.ghost()), (ChaseState::Idle, Vec2::new(5.0, -25.0)));
    }

    #[test]
    fn walking_carries_the_rest_of_the_step_to_the_next_leg() {
        let mut path = vec![Vec2::new(3.0, 0.0), Vec2::new(3.0, 4.0)];
        assert_eq!(walk(Vec2::ZERO, &mut path, 5.0), Vec2::new(3.0, 2.0));
        assert_eq!(path, [Vec2::new(3.0, 4.0)]);
        assert_eq!(walk(Vec2::ZERO, &mut Vec::new(), 5.0), Vec2::ZERO);
        let mut path = vec![Vec2::new(1.0, 0.0)];
        assert_eq!(walk(Vec2::ZERO, &mut path, 5.0), Vec2::new(1.0, 0.0));
        assert!(path.is_empty());
    }

    #[test]
    fn without_a_grid_it_flies_straight() {
        let mut app = App::new();
        app.add_plugins(AiPlugin);
        let w = app.world_mut();
        let target = w.spawn(at(50.0, 0.0)).id();
        let ghost = w.spawn((at(0.0, 0.0), Chaser { target: Some(target), ..Chaser::new(Vec2::ZERO, 60.0, 100.0, 500.0) })).id();
        for _ in 0..30 {
            app.tick(VIEWPORT, DT);
        }
        let p = app.world().get::<Transform2d>(ghost).unwrap().translation;
        assert!((p - Vec2::new(30.0, 0.0)).length() < 1.5, "{p}");
    }
}
