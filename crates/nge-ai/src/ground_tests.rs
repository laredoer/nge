//! 地面寻路：站的地方、走 / 落 / 穿 / 跳的连线，跳按能力生成（见 docs/gameplay2d.md）。

use super::*;

/// 用字符画地形：`#` 实心，`=` 单向平台，`/` 45° 上坡，`\` 45° 下坡，别的是空。
fn terrain(rows: &[&str]) -> Terrain {
    let cols = rows[0].len();
    let tiles = rows
        .iter()
        .flat_map(|r| r.chars())
        .map(|ch| match ch {
            '#' => Ground::Solid,
            '=' => Ground::OneWay,
            '/' => Ground::Slope { left: 0.0, right: 1.0 },
            '\\' => Ground::Slope { left: 1.0, right: 0.0 },
            _ => Ground::Empty,
        })
        .collect();
    Terrain::new(cols, tiles)
}

fn link(g: &GroundGraph, from: Cell, to: Cell) -> Option<Move> {
    let (a, b) = (g.spot(from)?, g.spot(to)?);
    g.links[a].iter().find(|l| l.to == b).map(|l| l.kind)
}

const REACH: Reach = Reach { height: 3.0, distance: 5.0 };

#[test]
fn spots_are_the_cells_above_floors() {
    let t = terrain(&["....", ".=..", "...#", "####"]);
    let g = GroundGraph::build(&t, REACH);
    let mut cells: Vec<Cell> = g.spots.iter().map(|s| s.cell).collect();
    cells.sort();
    assert_eq!(cells, [(0, 2), (1, 0), (1, 2), (2, 2), (3, 1)]);
    assert_eq!(g.spots[g.spot((3, 1)).unwrap()].point(), Vec2::new(3.5, -2.0));
}

#[test]
fn walking_along_floors_and_slopes() {
    // 平地 → 45° 上坡 → 高一格的平地
    let t = terrain(&["......", "../###", "######"]);
    let g = GroundGraph::build(&t, REACH);
    assert_eq!(link(&g, (0, 1), (1, 1)), Some(Move::Walk));
    assert_eq!(link(&g, (1, 1), (2, 1)), Some(Move::Walk), "走上坡");
    assert_eq!(link(&g, (2, 1), (3, 0)), Some(Move::Walk), "坡顶接上高一格的平地");
    assert_eq!(link(&g, (3, 0), (2, 1)), Some(Move::Walk), "反过来下坡");
    // 一格高的台阶走不上去（要跳）
    let t = terrain(&["....", "..##", "####"]);
    let g = GroundGraph::build(&t, REACH);
    assert_eq!(link(&g, (1, 1), (2, 0)), Some(Move::Jump));
}

#[test]
fn falling_off_edges_and_dropping_through_one_way_platforms() {
    let t = terrain(&[
        "##....", //
        "......",
        "..==..",
        "......",
        "######",
    ]);
    let g = GroundGraph::build(&t, Reach { height: 0.5, distance: 1.0 });
    // 从左上的台子边上落到单向平台上
    assert_eq!(link(&g, (1, -1), (2, 1)), None, "地图外面不是站的地方");
    assert_eq!(link(&g, (2, 1), (2, 3)), Some(Move::Drop), "从单向平台穿下去");
    assert_eq!(link(&g, (3, 1), (4, 3)), Some(Move::Fall), "从单向平台边上走出去落下");
    assert_eq!(link(&g, (2, 3), (2, 1)), None, "跳不了那么高");
}

#[test]
fn jump_links_follow_the_jump_ability() {
    // 地面上方 2 格有一块单向平台，右边隔 3 格空隙有另一块地
    let t = terrain(&[
        "..........", //
        "..........",
        "...==.....",
        "..........",
        "####...###",
    ]);
    let high = GroundGraph::build(&t, Reach { height: 3.0, distance: 5.0 });
    assert_eq!(link(&high, (2, 3), (3, 1)), Some(Move::Jump), "跳得上单向平台（从下面穿上去）");
    assert_eq!(link(&high, (3, 3), (7, 3)), Some(Move::Jump), "跳得过 3 格的空隙");
    let low = GroundGraph::build(&t, Reach { height: 1.5, distance: 5.0 });
    assert_eq!(link(&low, (2, 3), (3, 1)), None, "跳不了 2 格高");
    assert_eq!(link(&low, (3, 3), (7, 3)), Some(Move::Jump));
    // 刚好够高不行：最高点要比落点高出一点才落得上去
    let barely = GroundGraph::build(&t, Reach { height: 2.1, distance: 5.0 });
    assert_eq!(link(&barely, (2, 3), (3, 1)), None, "最高点只比平台高 0.1 格");
    let short = GroundGraph::build(&t, Reach { height: 3.0, distance: 3.0 });
    assert_eq!(link(&short, (3, 3), (7, 3)), None, "跳不了 4 格远");
    // 能力越大连线越多
    let count = |g: &GroundGraph| g.links.iter().flatten().filter(|l| l.kind == Move::Jump).count();
    assert!(count(&high) > count(&low) && count(&high) > count(&short));
}

#[test]
fn jumps_do_not_pass_through_ceilings_or_walls() {
    // 头顶两格有实心的天花板：从天花板底下跳不上右边高一格的台子，从缺口下面跳得上去
    let t = terrain(&[
        "......", //
        "###...",
        ".....#",
        "....##",
        "######",
    ]);
    let g = GroundGraph::build(&t, Reach { height: 3.0, distance: 5.0 });
    assert_eq!(link(&g, (0, 3), (4, 2)), None, "天花板挡着");
    assert_eq!(link(&g, (3, 3), (4, 2)), Some(Move::Jump), "从天花板的缺口下面跳得上去");
    // 头顶两格高的天花板：满高的跳碰头，矮一点的跳过去
    let t = terrain(&["######", "......", "......", "##..##"]);
    let g = GroundGraph::build(&t, Reach { height: 2.9, distance: 5.0 });
    let (a, b) = (g.spot((1, 2)).unwrap(), g.spot((4, 2)).unwrap());
    let l = g.links[a].iter().find(|l| l.to == b).expect("矮跳过得去");
    assert_eq!(l.kind, Move::Jump);
    assert!(l.height < 1.5, "跳得矮 {}", l.height);
    assert_eq!(g.jump_height(a, b), Some(l.height));
    // 往下跳的时候半路有单向平台：会被接住，到不了下面
    let t = terrain(&["........", "#.......", "#.===...", "#.......", "########"]);
    let g = GroundGraph::build(&t, Reach { height: 3.0, distance: 5.0 });
    assert_eq!(link(&g, (0, 0), (3, 1)), Some(Move::Jump), "跳到单向平台上");
    assert_eq!(link(&g, (0, 0), (3, 3)), None, "穿不过单向平台落到地上");
    // 墙后面的跳不到
    let t = terrain(&["......", "..#...", "..#...", "######"]);
    let g = GroundGraph::build(&t, Reach { height: 1.0, distance: 6.0 });
    assert_eq!(link(&g, (1, 2), (3, 2)), None, "墙比跳得高");
}

#[test]
fn paths_combine_walks_jumps_and_drops() {
    let t = terrain(&[
        "..........", //
        ".......===",
        "..........",
        "....==....",
        "..........",
        "##########",
    ]);
    let g = GroundGraph::build(&t, Reach { height: 2.5, distance: 5.0 });
    let (a, b) = (g.spot((0, 4)).unwrap(), g.spot((9, 0)).unwrap());
    let hops = g.find(a, b).expect("走得到右上的平台");
    let kinds: Vec<Move> = hops.iter().map(|h| h.kind).collect();
    assert!(kinds.contains(&Move::Jump), "{kinds:?}");
    assert_eq!(hops.last().unwrap().to, b);
    for w in hops.windows(2) {
        assert_eq!(w[0].to, w[1].from, "一步接一步");
    }
    // 回来：落下或穿下去，不用跳
    let back = g.find(b, a).expect("回得来");
    assert!(back.iter().all(|h| h.kind != Move::Jump), "{back:?}");
    assert_eq!(g.find(a, a), Some(Vec::new()));
    // 跳不上去时走不到
    let weak = GroundGraph::build(&t, Reach { height: 1.0, distance: 2.0 });
    assert_eq!(weak.find(weak.spot((0, 4)).unwrap(), weak.spot((9, 0)).unwrap()), None);
}

#[test]
fn reach_from_physics() {
    // 起跳 640、重力 1800、速度 260、格 32：高 3.56 格，远 5.78 格
    let r = Reach::from_physics(640.0, 1800.0, 260.0, 32.0);
    assert!((r.height - 3.555).abs() < 0.01 && (r.distance - 5.777).abs() < 0.01, "{r:?}");
    assert_eq!(r.across(r.height + 0.1), None);
    assert!((r.across(0.0).unwrap() - r.distance).abs() < 1e-4);
    assert!((r.across(r.height).unwrap() - r.distance / 2.0).abs() < 1e-4);
}

#[test]
fn spots_below_a_point() {
    let t = terrain(&["....", "./..", "####"]);
    let g = GroundGraph::build(&t, REACH);
    let below = |x: f32, y: f32| g.spot_below(Vec2::new(x, y), 4).map(|i| g.spots[i].cell);
    assert_eq!(below(0.5, -2.0), Some((0, 1)), "站在地上");
    assert_eq!(below(0.5, -0.2), Some((0, 1)), "在空中往下找");
    assert_eq!(below(1.8, -1.2), Some((1, 1)), "站在坡上");
    assert_eq!(below(1.2, -0.5), Some((1, 1)));
    assert_eq!(below(5.5, -1.0), None, "地图外面");
}

#[test]
fn dropping_through_stacked_one_way_rows_lands_below_all_of_them() {
    // 两行单向平台叠着：物理那边一起穿过，寻路也落到下面的地上
    let t = terrain(&["....", "....", "====", "====", "....", "####"]);
    let g = GroundGraph::build(&t, REACH);
    assert_eq!(link(&g, (1, 1), (1, 4)), Some(Move::Drop));
    assert_eq!(g.spot((1, 2)), None, "下面那行单向平台上面贴着上面那行，不是站的地方");
}

#[test]
fn building_a_big_map_stays_fast() {
    // 400 × 60 的地图：每 4 行一层平台，长 6 空 3
    let (cols, rows) = (400usize, 60usize);
    let mut tiles = vec![Ground::Empty; cols * rows];
    for r in (3..rows).step_by(4) {
        for c in 0..cols {
            if c % 9 < 6 || r == rows - 1 {
                tiles[r * cols + c] = if r % 8 == 3 { Ground::OneWay } else { Ground::Solid };
            }
        }
    }
    let t = Terrain::new(cols, tiles);
    let reach = Reach { height: 3.8, distance: 2.4 };
    // 取三次里最快的：别的测试一起跑时机器忙，偶尔慢一次不算
    let took = (0..3)
        .map(|_| {
            let start = std::time::Instant::now();
            let g = GroundGraph::build(&t, reach);
            assert!(g.spots.len() > 4000, "{}", g.spots.len());
            start.elapsed()
        })
        .min()
        .unwrap();
    assert!(took.as_millis() < 20, "建图 {took:?}");
}

#[test]
fn jumps_check_the_walker_size() {
    // 坑两边的地面，头顶三格高的天花板：矮的跳得过去，高 2.5 格的碰头
    let t = terrain(&["######", "......", "......", "......", "##..##"]);
    let short = GroundGraph::build(&t, REACH);
    assert_eq!(link(&short, (1, 3), (4, 3)), Some(Move::Jump));
    let tall = GroundGraph::build_for(&t, Walker { height: 2.5, ..Walker::new(REACH) });
    assert_eq!(link(&tall, (1, 3), (4, 3)), None, "高的碰头");
}

#[test]
fn slopes_steeper_than_the_walker_can_climb_are_not_walkable() {
    let t = terrain(&["......", "../###", "######"]);
    let g = GroundGraph::build(&t, REACH);
    assert_eq!(link(&g, (1, 1), (2, 1)), Some(Move::Walk), "默认能走 45°");
    let g = GroundGraph::build_for(&t, Walker { max_slope: 30f32.to_radians(), ..Walker::new(REACH) });
    assert_eq!(g.spot((2, 1)), None, "最陡 30° 的站不上 45° 的坡");
    assert_eq!(link(&g, (1, 1), (2, 1)), None);
}

#[test]
fn heading_for_a_point_over_a_pit_stops_at_the_nearest_edge() {
    // 坑上方没有能站的地方：走到离它最近的、走得到的地方
    let t = terrain(&["..........", "..........", "####...###"]);
    let g = GroundGraph::build(&t, Reach { height: 1.0, distance: 1.0 });
    let from = g.spot((0, 1)).unwrap();
    let over = Vec2::new(5.5, -1.5);
    assert_eq!(g.spot_below(over, 64), None);
    let hops = g.find_near(from, over, &|_, _| false).unwrap();
    assert_eq!(g.spots[hops.last().unwrap().to].cell, (3, 1), "停在坑边");
    assert!(hops.iter().all(|h| h.kind == Move::Walk));
    // 已经在最近的地方：不用走
    assert_eq!(g.find_near(g.spot((3, 1)).unwrap(), over, &|_, _| false), Some(vec![]));
}

#[test]
fn avoided_links_are_not_used() {
    let t = terrain(&["....", "..##", "####"]);
    let g = GroundGraph::build(&t, REACH);
    let (a, b) = (g.spot((1, 1)).unwrap(), g.spot((2, 0)).unwrap());
    assert!(g.find(a, b).is_some());
    let hops = g.find_avoiding(a, b, &|x, y| x == (1, 1) && y == (2, 0)).unwrap();
    assert!(hops.iter().all(|h| g.spots[h.from].cell != (1, 1) || g.spots[h.to].cell != (2, 0)), "换一条跳 {hops:?}");
    assert_eq!(g.find_avoiding(a, b, &|_, y| y == (2, 0)), None, "跳上去的全不走就到不了");
}

#[test]
fn ground_maps_see_over_slopes_and_through_one_way_rows() {
    let t = terrain(&["......", "..=...", "../###", "######"]);
    let m = GroundMap { terrain: t, origin: Vec2::new(0.0, 0.0), tile: 1.0 };
    // 站在坡上的点（坡格里、顶边以上）看得见右边高台上的
    assert!(m.sees(Vec2::new(2.7, -2.1), Vec2::new(4.5, -1.5)));
    assert!(m.sees(Vec2::new(0.5, -2.5), Vec2::new(2.5, -0.5)), "单向平台不挡");
    assert!(!m.sees(Vec2::new(0.5, -2.5), Vec2::new(4.5, -2.5)), "实心的挡");
    assert!(m.covers(Vec2::new(1.0, -1.0)) && !m.covers(Vec2::new(1.0, -5.0)) && !m.covers(Vec2::new(1.0, 2.0)));
}
