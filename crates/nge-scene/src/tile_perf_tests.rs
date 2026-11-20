//! 大地图的性能回归（见 docs/big-scene-perf.md）：2000 × 200 的地图用 `setTiles` 改一格、改声明里的一格都在阈值以下
//! （镜头移动的整帧在 tests/big_map_perf.rs）。
//! 每项跑几次取最快的一次，阈值放得很宽，只拦住退回整张重建、整张遍历这种量级的退化。

use std::time::{Duration, Instant};

use serde_json::json;

use crate::testing::{app, frame};

const COLS: usize = 2000;
const ROWS: usize = 200;
/// `setTiles` 改一格的一帧（整帧：同步、物理、行为树都算上）。
const EDIT_LIMIT: Duration = Duration::from_millis(60);
/// 声明里改一格：解析整份场景 JSON、逐格比、局部改、一帧。
const DECLARED_LIMIT: Duration = Duration::from_millis(60);

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

#[test]
fn declaring_one_changed_cell_of_a_huge_map_stays_cheap() {
    let mut app = app();
    let mut data = big_map();
    let scene = |d: &[i32]| json!({ "tilemaps": [{ "id": "big", "texture": "grid", "tile": 16, "cols": COLS, "data": d, "solid": [0], "oneWay": [1], "tap": true }] });
    frame(&mut app, Some(scene(&data)));
    frame(&mut app, None);
    let mut texts = Vec::new();
    let build = fastest(5, |i| {
        data[150 * COLS + 1000 + i] = -1;
        texts.push(serde_json::to_string(&scene(&data)).unwrap());
    });
    eprintln!("拼、序列化整份场景（TSX 那边要做的，这里用 serde_json 量）：{build:?}");
    let edit = fastest(5, |i| {
        let value: serde_json::Value = serde_json::from_str(&texts[i]).unwrap();
        frame(&mut app, Some(value));
    });
    eprintln!("声明里改一格（解析 JSON、同步、一帧）：{edit:?}");
    assert!(edit < DECLARED_LIMIT, "声明里改一格 {edit:?}");
}
