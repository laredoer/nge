//! 大地图的性能回归（见 docs/big-scene-perf.md）：2000 × 200 的地图改一格在阈值以下（镜头移动的在 nge-render2d 的 tiles.rs）。
//! 每项跑几次取最快的一次，阈值放得很宽，只拦住退回整张重建、整张遍历这种量级的退化。

use std::time::{Duration, Instant};

use serde_json::json;

use crate::testing::{app, frame};

const COLS: usize = 2000;
const ROWS: usize = 200;
/// 改一格的一帧（整帧：同步、物理、行为树都算上）。改动前是 8 秒多。
const EDIT_LIMIT: Duration = Duration::from_millis(60);

/// 地面起伏的大地图：上面空、下面实心，帧号 0 实心、1 单向平台。
fn big_map() -> Vec<i32> {
    let mut d = vec![-1; COLS * ROWS];
    for c in 0..COLS {
        let ground = 120 + (c * 7 % 23);
        for r in ground..ROWS {
            d[r * COLS + c] = 0;
        }
        if c % 37 < 6 {
            d[(ground - 8) * COLS + c] = 1;
        }
    }
    d
}

fn fastest(runs: usize, mut f: impl FnMut(usize)) -> Duration {
    (0..runs)
        .map(|i| {
            let t = Instant::now();
            f(i);
            t.elapsed()
        })
        .min()
        .unwrap()
}

#[test]
fn editing_one_cell_of_a_huge_map_stays_cheap() {
    let mut app = app();
    let map = json!({ "id": "big", "texture": "grid", "tile": 16, "cols": COLS, "x": 0, "y": 0, "data": big_map(), "solid": [0], "oneWay": [1], "tap": true });
    let t = Instant::now();
    frame(&mut app, Some(json!({ "tilemaps": [map] })));
    eprintln!("整张建：{:?}", t.elapsed());
    frame(&mut app, None);
    let edit = fastest(5, |i| {
        app.command("setTiles", &json!({ "id": "big", "tiles": [[1000 + i, 150, -1]] }));
        frame(&mut app, None);
    });
    eprintln!("改一格的一帧：{edit:?}");
    let idle = fastest(5, |_| {
        frame(&mut app, None);
    });
    eprintln!("什么都不改的一帧：{idle:?}");
    assert!(edit < EDIT_LIMIT, "改一格 {edit:?}");
}
