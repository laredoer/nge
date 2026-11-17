use serde_json::{Value as Json, json};

use super::*;

fn graph(def: Json) -> AnimGraph {
    AnimGraph::from_def(&serde_json::from_value(def).unwrap()).unwrap()
}

fn compile_error(def: Json) -> String {
    match AnimGraph::from_def(&serde_json::from_value(def).unwrap()) {
        Ok(_) => panic!("应该编译不过"),
        Err(e) => e,
    }
}

/// 测试用的片段时长。
fn durations(clip: &str) -> Option<f32> {
    match clip {
        "idle" => Some(2.0),
        "walk" | "a" | "b" | "c" | "wave" => Some(1.0),
        "run" | "hit" => Some(0.5),
        "jump" => Some(0.4),
        "land" => Some(0.2),
        _ => None,
    }
}

fn step(g: &mut AnimGraph, dt: f32) -> Vec<String> {
    g.step(dt, durations).into_iter().map(|f| f.name).collect()
}

/// 第 `layer` 层在播的片段：(名字, 时间, 权重)。
fn clips(g: &AnimGraph, layer: usize) -> Vec<(String, f32, f32)> {
    let out = &g.output()[layer];
    out.clips.iter().map(|c| (g.graph().clips()[c.clip].name.clone(), c.time, c.weight)).collect()
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

fn assert_clips(g: &AnimGraph, layer: usize, want: &[(&str, f32, f32)]) {
    let got = clips(g, layer);
    let ok = got.len() == want.len() && got.iter().zip(want).all(|((n, t, w), (wn, wt, ww))| n == wn && close(*t, *wt) && close(*w, *ww));
    assert!(ok, "第 {layer} 层：{got:?}，应该是 {want:?}");
}

// ---------- 编译 ----------

#[test]
fn compile_reports_mistakes() {
    let two = |to: Json| json!({ "params": { "speed": 0, "moving": false, "jump": "trigger" }, "start": "a", "states": { "a": { "clip": "a", "to": to }, "b": { "clip": "b" } } });
    let cases = [
        (two(json!([{ "to": "nope" }])), "没有状态"),
        (two(json!([{ "to": "b", "when": "fast" }])), "没有参数"),
        (two(json!([{ "to": "b", "when": "speed" }])), "这里要"),
        (two(json!([{ "to": "b", "when": "speed > fast" }])), "右边要是数"),
        (two(json!([{ "to": "b", "when": "!jump" }])), "这里要"),
        (two(json!([{ "to": "b", "when": "moving > 1" }])), "这里要"),
        (json!({ "states": { "a": { "clip": "a", "blend": { "by": "x", "clips": [] } } } }), "只写一个"),
        (json!({ "states": { "a": {} } }), "只写一个"),
        (json!({ "states": { "a": { "clip": "a" }, "b": { "clip": "b" } } }), "要写 start"),
        (json!({}), "没有状态"),
        (json!({ "params": { "x": "kind" }, "states": { "a": { "clip": "a" } } }), "\"trigger\""),
        (json!({ "params": { "x": 0 }, "states": { "a": { "blend": { "by": "x", "clips": [{ "clip": "a", "at": [0, 1] }] } } } }), "at 要和 by"),
        (json!({ "params": { "x": 0 }, "states": { "a": { "blend": { "by": ["x", "y"], "clips": [] } } } }), "没有参数"),
        (json!({ "clips": { "a": { "frames": [1], "fps": 0 } }, "states": { "a": { "clip": "a" } } }), "fps"),
        (json!({ "clips": { "a": { "events": [{ "t": -1, "name": "x" }] } }, "states": { "a": { "clip": "a" } } }), "不小于 0"),
        (json!({ "layers": [{ "weight": "w", "states": { "a": { "clip": "a" } } }] }), "没有参数"),
        (json!({ "params": { "on": true }, "states": { "a": { "clip": "a", "speedParam": "on" } } }), "这里要"),
    ];
    for (def, want) in cases {
        let e = compile_error(def.clone());
        assert!(e.contains(want), "{def} 的错误 {e:?} 里应该有 {want:?}");
    }
}

#[test]
fn definition_reads_from_json() {
    let def = GraphDef::from_json(r#"{ "params": { "speed": 1.5, "on": true, "go": "trigger" }, "states": { "a": { "clip": "a" } } }"#).unwrap();
    let g = AnimGraph::from_def(&def).unwrap();
    assert_eq!((g.float("speed"), g.bool("on"), g.triggered("go")), (Some(1.5), Some(true), Some(false)), "初始值");
    assert!(GraphDef::from_json("{ nope").unwrap_err().contains("JSON"));
    assert!(GraphDef::from_json(r#"{ "statez": {} }"#).is_err(), "写错的字段");
}

#[test]
fn params_check_names_and_kinds() {
    let mut g = graph(json!({ "params": { "speed": 0, "on": false, "go": "trigger" }, "states": { "a": { "clip": "a" } } }));
    assert!(g.set_float("speed", 3.0) && g.float("speed") == Some(3.0));
    assert!(!g.set_float("on", 1.0) && !g.set_float("nope", 1.0), "类型不对、名字不对");
    assert!(!g.set_float("speed", f32::NAN) && g.float("speed") == Some(3.0), "NaN 不写");
    assert!(g.set_bool("on", true) && g.bool("on") == Some(true));
    assert!(!g.set_bool("go", true) && !g.trigger("on"));
    assert!(g.trigger("go") && g.triggered("go") == Some(true));
    assert!(g.reset_trigger("go") && g.triggered("go") == Some(false));
}

// ---------- 转换 ----------

fn idle_walk(when: Json) -> AnimGraph {
    graph(json!({
        "params": { "speed": 0, "moving": false },
        "start": "idle",
        "states": { "idle": { "clip": "idle", "to": [{ "to": "walk", "when": when }] }, "walk": { "clip": "walk" } },
    }))
}

#[test]
fn bool_and_float_conditions_all_have_to_hold() {
    let mut g = idle_walk(json!(["moving", "speed > 1"]));
    step(&mut g, 0.1);
    assert_eq!(g.state(0), Some("idle"), "默认状态");
    g.set_bool("moving", true);
    g.set_float("speed", 0.5);
    step(&mut g, 0.1);
    assert_eq!(g.state(0), Some("idle"), "speed 不够");
    g.set_float("speed", 2.0);
    step(&mut g, 0.1);
    assert_eq!(g.state(0), Some("walk"));

    for (when, speed, moves) in [("speed >= 1", 1.0, true), ("speed <= 1", 1.5, false), ("speed < 1", 1.0, false), ("speed == 1", 1.0, true), ("speed != 1", 1.0, false)] {
        let mut g = idle_walk(json!(when));
        g.set_float("speed", speed);
        step(&mut g, 0.1);
        assert_eq!(g.state(0) == Some("walk"), moves, "{when}，speed = {speed}");
    }
    let mut g = idle_walk(json!("!moving"));
    step(&mut g, 0.1);
    assert_eq!(g.state(0), Some("walk"), "取反");
}

#[test]
fn trigger_is_used_once_and_waits_until_a_transition_uses_it() {
    let mut g = graph(json!({
        "params": { "jump": "trigger" },
        "start": "idle",
        "states": {
            "idle": { "clip": "idle", "to": [{ "to": "jump", "when": "jump" }] },
            "jump": { "clip": "jump", "loop": false, "to": [{ "to": "idle", "exit": 1 }] },
        },
    }));
    g.trigger("jump");
    step(&mut g, 0.1);
    assert_eq!((g.state(0), g.triggered("jump")), (Some("jump"), Some(false)), "转了，用掉");
    g.trigger("jump");
    step(&mut g, 0.1);
    assert_eq!((g.state(0), g.triggered("jump")), (Some("jump"), Some(true)), "jump 状态不看它：留着");
    step(&mut g, 0.4);
    assert_eq!(g.state(0), Some("idle"), "播完回去");
    step(&mut g, 0.1);
    assert_eq!((g.state(0), g.triggered("jump")), (Some("jump"), Some(false)), "回到 idle 后用掉留着的");
}

#[test]
fn one_transition_per_step() {
    let mut g = graph(json!({
        "params": { "go": true },
        "start": "a",
        "states": { "a": { "clip": "a", "to": [{ "to": "b", "when": "go" }] }, "b": { "clip": "b", "to": [{ "to": "c", "when": "go" }] }, "c": { "clip": "c" } },
    }));
    step(&mut g, 0.1);
    assert_eq!(g.state(0), Some("b"));
    step(&mut g, 0.1);
    assert_eq!(g.state(0), Some("c"));
}

#[test]
fn exit_time_waits_until_enough_has_played() {
    let mut g = graph(json!({ "start": "land", "states": { "land": { "clip": "land", "loop": false, "to": [{ "to": "walk", "exit": 1 }] }, "walk": { "clip": "walk" } } }));
    step(&mut g, 0.1);
    assert_eq!(g.state(0), Some("land"), "播到一半");
    step(&mut g, 0.1);
    assert_eq!(g.state(0), Some("walk"), "播完");

    // 循环的：累计播过 1.5 遍
    let mut g = graph(json!({ "start": "walk", "states": { "walk": { "clip": "walk", "to": [{ "to": "run", "exit": 1.5 }] }, "run": { "clip": "run" } } }));
    step(&mut g, 0.9);
    step(&mut g, 0.5);
    assert_eq!(g.state(0), Some("walk"), "1.4 遍");
    step(&mut g, 0.2);
    assert_eq!(g.state(0), Some("run"), "绕回去以后也接着算");

    // 倒放：从尾巴往头播，播完一遍也算
    let mut g = graph(json!({ "start": "land", "states": { "land": { "clip": "land", "speed": -1, "loop": false, "to": [{ "to": "walk", "exit": 1 }] }, "walk": { "clip": "walk" } } }));
    step(&mut g, 0.15);
    assert_eq!(g.state(0), Some("land"));
    step(&mut g, 0.1);
    assert_eq!(g.state(0), Some("walk"));

    // 不循环的停在尾巴以后不再累计：exit 2 永远到不了
    let mut g = graph(json!({ "start": "land", "states": { "land": { "clip": "land", "loop": false, "to": [{ "to": "walk", "exit": 2 }] }, "walk": { "clip": "walk" } } }));
    step(&mut g, 5.0);
    step(&mut g, 5.0);
    assert_eq!(g.state(0), Some("land"));
}

#[test]
fn any_state_goes_first_and_skips_the_current_state() {
    let mut g = graph(json!({
        "params": { "hit": "trigger", "go": true },
        "start": "idle",
        "states": { "idle": { "clip": "idle", "to": [{ "to": "walk", "when": "go" }] }, "walk": { "clip": "walk" }, "hurt": { "clip": "hit" } },
        "any": [{ "to": "hurt", "when": "hit" }],
    }));
    g.trigger("hit");
    step(&mut g, 0.1);
    assert_eq!(g.state(0), Some("hurt"), "任意状态的转换先看");
    step(&mut g, 0.1);
    g.trigger("hit");
    step(&mut g, 0.1);
    assert_eq!(g.state(0), Some("hurt"));
    assert_clips(&g, 0, &[("hit", 0.2, 1.0)]);
    assert_eq!(g.triggered("hit"), Some(true), "转到自己的跳过，trigger 不用掉");
}

// ---------- 过渡 ----------

#[test]
fn fade_mixes_the_states_then_drops_the_old_one() {
    let mut g = graph(json!({
        "params": { "moving": false },
        "start": "idle",
        "states": { "idle": { "clip": "idle", "to": [{ "to": "walk", "when": "moving", "fade": 0.4 }] }, "walk": { "clip": "walk" } },
    }));
    step(&mut g, 0.1);
    g.set_bool("moving", true);
    step(&mut g, 0.1);
    assert_clips(&g, 0, &[("idle", 0.2, 1.0)]);
    step(&mut g, 0.2);
    assert_clips(&g, 0, &[("idle", 0.4, 0.5), ("walk", 0.2, 0.5)]);
    step(&mut g, 0.2);
    assert_clips(&g, 0, &[("walk", 0.4, 1.0)]);
}

fn abc() -> AnimGraph {
    graph(json!({
        "params": { "to": 0 },
        "start": "a",
        "states": {
            "a": { "clip": "a", "to": [{ "to": "b", "when": "to == 1", "fade": 1 }, { "to": "c", "when": "to == 2", "fade": 1 }] },
            "b": { "clip": "b", "to": [{ "to": "c", "when": "to == 2", "fade": 1 }, { "to": "a", "when": "to == 0", "fade": 1 }] },
            "c": { "clip": "c" },
        },
    }))
}

#[test]
fn interrupting_a_fade_keeps_the_weights_continuous() {
    let mut g = abc();
    g.set_float("to", 1.0);
    step(&mut g, 0.0);
    step(&mut g, 0.25);
    assert_clips(&g, 0, &[("a", 0.25, 0.75), ("b", 0.25, 0.25)]);
    g.set_float("to", 2.0);
    step(&mut g, 0.0);
    assert_eq!(g.state(0), Some("c"));
    assert_clips(&g, 0, &[("a", 0.25, 0.75), ("b", 0.25, 0.25)]);
    step(&mut g, 0.5);
    assert_clips(&g, 0, &[("a", 0.75, 0.375), ("b", 0.75, 0.125), ("c", 0.5, 0.5)]);
}

#[test]
fn going_back_to_a_fading_state_keeps_its_time() {
    let mut g = abc();
    step(&mut g, 0.3);
    g.set_float("to", 1.0);
    step(&mut g, 0.0);
    step(&mut g, 0.4);
    assert_clips(&g, 0, &[("a", 0.7, 0.6), ("b", 0.4, 0.4)]);
    g.set_float("to", 0.0);
    step(&mut g, 0.0);
    assert_eq!(g.state(0), Some("a"));
    assert_clips(&g, 0, &[("a", 0.7, 0.6), ("b", 0.4, 0.4)]);
    step(&mut g, 0.2);
    assert_clips(&g, 0, &[("a", 0.9, 0.68), ("b", 0.6, 0.32)]);
}

#[test]
fn instances_are_capped() {
    let states: serde_json::Map<String, Json> =
        (0..12).map(|i| (format!("s{i}"), json!({ "clip": "a", "to": [{ "to": format!("s{}", (i + 1) % 12), "fade": 100 }] }))).collect();
    let mut g = graph(json!({ "start": "s0", "states": states }));
    for _ in 0..20 {
        step(&mut g, 0.5);
        let out = clips(&g, 0);
        assert!(out.len() <= MAX_INSTANCES, "{}", out.len());
        let sum: f32 = out.iter().map(|c| c.2).sum();
        assert!(close(sum, 1.0), "权重加起来是 1：{sum}");
    }
}

// ---------- 混合 ----------

fn assert_weights(got: Vec<f32>, want: &[f32]) {
    assert!(got.len() == want.len() && got.iter().zip(want).all(|(a, b)| close(*a, *b)), "{got:?}，应该是 {want:?}");
}

#[test]
fn blend1d_splits_between_neighbours() {
    let at = [0.0, 80.0, 240.0];
    assert_weights(blend1d(&at, -5.0), &[1.0, 0.0, 0.0]);
    assert_weights(blend1d(&at, 0.0), &[1.0, 0.0, 0.0]);
    assert_weights(blend1d(&at, 40.0), &[0.5, 0.5, 0.0]);
    assert_weights(blend1d(&at, 80.0), &[0.0, 1.0, 0.0]);
    assert_weights(blend1d(&at, 160.0), &[0.0, 0.5, 0.5]);
    assert_weights(blend1d(&at, 300.0), &[0.0, 0.0, 1.0]);
    assert_weights(blend1d(&[240.0, 0.0, 80.0], 40.0), &[0.0, 0.5, 0.5]);
    assert_weights(blend1d(&[5.0], 1.0), &[1.0]);
    assert_weights(blend1d(&at, f32::NAN), &[1.0, 0.0, 0.0]);
    assert_weights(blend1d(&[0.0, 0.0, 1.0], 0.0), &[1.0, 0.0, 0.0]);
}

#[test]
fn blend2d_is_exact_on_the_points_and_sums_to_one() {
    let at = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]];
    for (i, p) in at.iter().enumerate() {
        let mut want = [0.0; 5];
        want[i] = 1.0;
        assert_weights(blend2d(&at, *p), &want);
    }
    assert_weights(blend2d(&at, [0.5, 0.0]), &[0.5, 0.5, 0.0, 0.0, 0.0]);
    assert_weights(blend2d(&at, [f32::NAN, 0.0]), &[1.0, 0.0, 0.0, 0.0, 0.0]);
    assert_weights(blend2d(&[[3.0, 3.0]], [0.0, 0.0]), &[1.0]);
    for x in -4..=4 {
        for y in -4..=4 {
            let w = blend2d(&at, [x as f32 * 0.4, y as f32 * 0.4]);
            assert!(close(w.iter().sum(), 1.0) && w.iter().all(|w| *w >= 0.0), "({x}, {y})：{w:?}");
        }
    }
}

fn locomotion(extra: Json) -> AnimGraph {
    let mut def = json!({
        "params": { "speed": 0 },
        "start": "move",
        "states": { "move": { "blend": { "by": "speed", "clips": [{ "clip": "walk", "at": 0 }, { "clip": "run", "at": 1 }] } } },
        "clips": {
            "walk": { "events": [{ "t": 0, "name": "walk-step" }, { "t": 0.5, "name": "walk-step" }] },
            "run": { "events": [{ "t": 0, "name": "run-step" }, { "t": 0.25, "name": "run-step" }] },
        },
    });
    if let (Some(d), Json::Object(e)) = (def.as_object_mut(), extra) {
        d.extend(e);
    }
    graph(def)
}

#[test]
fn blend_children_share_one_phase() {
    let mut g = locomotion(json!({}));
    g.set_float("speed", 0.5);
    step(&mut g, 0.375);
    assert_clips(&g, 0, &[("walk", 0.5, 0.5), ("run", 0.25, 0.5)]);
    g.set_float("speed", 1.0);
    step(&mut g, 0.125);
    assert_clips(&g, 0, &[("run", 0.375, 1.0)]);
}

#[test]
fn blend2d_state_uses_both_params() {
    let mut g = graph(json!({
        "params": { "x": 0, "y": 0 },
        "start": "aim",
        "states": { "aim": { "blend": { "by": ["x", "y"], "clips": [{ "clip": "a", "at": [0, 0] }, { "clip": "b", "at": [1, 0] }, { "clip": "c", "at": [0, 1] }] } } },
    }));
    g.set_float("y", 1.0);
    step(&mut g, 0.1);
    assert_clips(&g, 0, &[("c", 0.1, 1.0)]);
}

#[test]
fn speed_param_scales_and_negative_speed_starts_at_the_end() {
    let mut g = graph(json!({ "params": { "rate": 0.5 }, "states": { "w": { "clip": "walk", "speed": 2, "speedParam": "rate" } } }));
    step(&mut g, 0.25);
    assert_clips(&g, 0, &[("walk", 0.25, 1.0)]);

    let mut g = graph(json!({ "states": { "l": { "clip": "land", "speed": -1, "loop": false } } }));
    step(&mut g, 0.0);
    assert_clips(&g, 0, &[("land", 0.2, 1.0)]);
    step(&mut g, 0.05);
    assert_clips(&g, 0, &[("land", 0.15, 1.0)]);
}

#[test]
fn output_time_wraps_or_clamps() {
    let mut g = graph(json!({ "states": { "w": { "clip": "walk" } } }));
    step(&mut g, 1.25);
    assert_clips(&g, 0, &[("walk", 0.25, 1.0)]);
    let mut g = graph(json!({ "states": { "j": { "clip": "jump", "loop": false } } }));
    step(&mut g, 1.0);
    assert_clips(&g, 0, &[("jump", 0.4, 1.0)]);
    // 时长未知的片段：循环的停在 0，不循环的直接到尾
    let mut g = graph(json!({ "states": { "u": { "clip": "unknown" } } }));
    step(&mut g, 1.0);
    assert_clips(&g, 0, &[("unknown", 0.0, 1.0)]);
}

// ---------- 分层 ----------

#[test]
fn layers_report_weight_and_mode() {
    let mut g = graph(json!({
        "params": { "wave": 0.25 },
        "start": "walk",
        "states": { "walk": { "clip": "walk" } },
        "layers": [
            { "name": "upper", "weight": "wave", "mask": ["spine"], "states": { "wave": { "clip": "wave" } } },
            { "name": "hit", "mode": "additive", "weight": 2, "reference": "idle", "states": { "hit": { "clip": "hit" } } },
        ],
    }));
    step(&mut g, 0.1);
    let out = g.output();
    assert_eq!(out.iter().map(|l| (l.layer, l.weight, l.additive)).collect::<Vec<_>>(), [(0, 1.0, false), (1, 0.25, false), (2, 1.0, true)], "权重限在 0–1");
    let layers = g.graph().layers();
    assert_eq!((layers[1].name.as_str(), layers[1].mask.clone()), ("upper", Some(vec!["spine".to_owned()])));
    assert_eq!(layers[2].reference.map(|c| g.graph().clips()[c].name.as_str()), Some("idle"));
    g.set_float("wave", 0.75);
    step(&mut g, 0.1);
    assert_eq!(g.output()[1].weight, 0.75);
    assert_clips(&g, 1, &[("wave", 0.2, 1.0)]);
}

// ---------- 事件：crossed ----------

#[test]
fn crossed_forward_and_across_loops() {
    let ev = [0.0, 0.5];
    assert_eq!(crossed(&ev, 1.0, true, 0.2, 0.6, false), [1]);
    assert_eq!(crossed(&ev, 1.0, true, 0.2, 0.5, false), [1], "终点算");
    assert_eq!(crossed(&ev, 1.0, true, 0.5, 0.7, false), Vec::<usize>::new(), "起点不算，不重复");
    assert_eq!(crossed(&ev, 1.0, true, 0.6, 1.2, false), [0], "绕回开头");
    assert_eq!(crossed(&ev, 1.0, true, 0.0, 0.1, true), [0], "刚进入：起点也算");
    assert_eq!(crossed(&ev, 1.0, true, 0.0, 0.1, false), Vec::<usize>::new());
    assert_eq!(crossed(&ev, 1.0, true, 0.4, 2.6, false), [1, 0, 1, 0, 1], "跳帧：每圈都发，按先后");
    assert_eq!(crossed(&[0.5], 1.0, true, 0.0, 100.0, false).len(), MAX_LOOPS as usize, "最多算这么多圈");
}

#[test]
fn crossed_backward() {
    let ev = [0.0, 0.5];
    assert_eq!(crossed(&ev, 1.0, true, 0.6, 0.2, false), [1]);
    assert_eq!(crossed(&ev, 1.0, true, 0.2, -0.3, false), [0], "倒着绕过开头");
    assert_eq!(crossed(&ev, 1.0, true, 0.5, 0.3, false), Vec::<usize>::new(), "起点不算");
    assert_eq!(crossed(&ev, 1.0, true, 0.5, 0.3, true), [1], "刚进入：起点也算");
    assert_eq!(crossed(&ev, 1.0, true, 1.2, -0.6, false), [0, 1, 0, 1], "倒着跨两圈，按经过的先后");
}

#[test]
fn crossed_once_without_loop() {
    let ev = [0.0, 0.2];
    // 状态机里不循环的停在尾巴时就是 1 × 时长（f32 换成 f64）
    let end = f64::from(0.2f32);
    assert_eq!(crossed(&ev, 0.2, false, 0.1, end, false), [1], "播到尾");
    assert_eq!(crossed(&ev, 0.2, false, end, end, false), Vec::<usize>::new(), "停在尾巴不再发");
    assert_eq!(crossed(&ev, 0.2, false, 0.0, 0.1, true), [0]);
    assert_eq!(crossed(&ev, 0.0, true, 0.0, 0.0, true), [0], "时长 0：进入时发 0 秒的");
    assert_eq!(crossed(&ev, 0.0, true, 0.0, 0.0, false), Vec::<usize>::new());
}

// ---------- 事件：状态机 ----------

#[test]
fn events_fire_once_per_crossing() {
    let mut g = graph(json!({ "states": { "w": { "clip": "walk" } }, "clips": { "walk": { "events": [{ "t": 0, "name": "down" }, { "t": 0.5, "name": "up" }] } } }));
    assert_eq!(step(&mut g, 0.1), ["down"], "进入时 0 秒的");
    assert_eq!(step(&mut g, 0.3), Vec::<String>::new());
    assert_eq!(step(&mut g, 0.2), ["up"]);
    assert_eq!(step(&mut g, 0.5), ["down"], "绕回开头");
    assert_eq!(step(&mut g, 2.0), ["up", "down", "up", "down"], "跳帧");
    assert_eq!(step(&mut g, 0.0), Vec::<String>::new(), "不动不发");
}

#[test]
fn reverse_play_fires_events_backwards() {
    let mut g = graph(json!({ "states": { "w": { "clip": "walk", "speed": -1 } }, "clips": { "walk": { "events": [{ "t": 0.25, "name": "b" }, { "t": 0.5, "name": "a" }] } } }));
    assert_eq!(step(&mut g, 0.3), Vec::<String>::new());
    assert_eq!(step(&mut g, 0.3), ["a"]);
    assert_eq!(step(&mut g, 0.3), ["b"]);
}

#[test]
fn blend_events_come_from_the_heaviest_clip() {
    let mut g = locomotion(json!({}));
    g.set_float("speed", 0.25);
    // 有效时长 0.875，走 0.7 秒到相位 0.8
    assert_eq!(step(&mut g, 0.7), ["walk-step", "walk-step"], "走占多数：只有走的脚步（0 和一半）");
    g.set_float("speed", 0.75);
    // 有效时长 0.625，走 0.5 秒从相位 0.8 到 1.6
    assert_eq!(step(&mut g, 0.5), ["run-step", "run-step"], "跑占多数：相位同步，接着下一圈的开头和一半");
}

#[test]
fn fading_states_only_the_heavier_one_fires() {
    let mut g = graph(json!({
        "params": { "go": false },
        "start": "a",
        "states": { "a": { "clip": "a", "to": [{ "to": "b", "when": "go", "fade": 1 }] }, "b": { "clip": "b" } },
        "clips": { "a": { "events": [{ "t": 0.3, "name": "a1" }, { "t": 0.8, "name": "a2" }] }, "b": { "events": [{ "t": 0.05, "name": "b1" }, { "t": 0.55, "name": "b2" }] } },
    }));
    step(&mut g, 0.2);
    g.set_bool("go", true);
    assert_eq!(step(&mut g, 0.0), Vec::<String>::new());
    assert_eq!(step(&mut g, 0.2), ["a1"], "a 还重：b 的 0.05 不发");
    assert_eq!(step(&mut g, 0.4), ["b2"], "b 重了：a 的 0.8 不发");
    assert_eq!(step(&mut g, 0.5), ["b1"], "淡完只有 b");
}

#[test]
fn entering_a_state_fires_its_zero_time_event_once() {
    let mut g = graph(json!({
        "params": { "hit": "trigger" },
        "start": "idle",
        "states": { "idle": { "clip": "idle", "to": [{ "to": "hurt", "when": "hit" }] }, "hurt": { "clip": "hit", "loop": false } },
        "clips": { "hit": { "events": [{ "t": 0, "name": "ouch" }] } },
    }));
    step(&mut g, 0.1);
    g.trigger("hit");
    assert_eq!(step(&mut g, 0.1), Vec::<String>::new(), "这一步转过去");
    assert_eq!(step(&mut g, 0.1), ["ouch"]);
    assert_eq!(step(&mut g, 0.1), Vec::<String>::new());
}

#[test]
fn layers_fire_their_own_events_unless_weight_is_zero() {
    let mut g = graph(json!({
        "params": { "wave": 0 },
        "start": "walk",
        "states": { "walk": { "clip": "walk" } },
        "layers": [{ "name": "upper", "weight": "wave", "states": { "wave": { "clip": "wave" } } }],
        "clips": { "walk": { "events": [{ "t": 0.2, "name": "step" }] }, "wave": { "events": [{ "t": 0.2, "name": "hand" }] } },
    }));
    assert_eq!(step(&mut g, 0.3), ["step"]);
    g.set_float("wave", 1.0);
    assert_eq!(step(&mut g, 1.0), ["step", "hand"]);
    let fired = g.step(1.0, durations);
    assert_eq!((fired[1].layer.as_str(), fired[1].state.as_str(), fired[1].clip.as_str()), ("upper", "wave", "wave"));
}
