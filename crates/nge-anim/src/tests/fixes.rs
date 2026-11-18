//! 再进入、自己转自己、trigger 的清理、淡入时的起点事件、一样重时的规则、编译时的检查。

use super::*;

/// 攻击（不循环，0.25 秒有判定）用 trigger 进，播完淡回 idle。
fn attack(fade_in: f32) -> AnimGraph {
    graph(json!({
        "params": { "atk": "trigger" },
        "clips": { "hit": { "events": [{ "t": 0, "name": "swing" }, { "t": 0.25, "name": "hitbox" }] } },
        "start": "idle",
        "states": {
            "idle": { "clip": "idle", "to": [{ "to": "attack", "when": "atk", "fade": fade_in }] },
            "attack": { "clip": "hit", "loop": false, "to": [{ "to": "idle", "exit": 1, "fade": 0.3 }] },
        },
    }))
}

fn steps(g: &mut AnimGraph, n: usize, dt: f32) -> Vec<String> {
    (0..n).flat_map(|_| step(g, dt)).collect()
}

#[test]
fn reentering_a_finished_one_shot_while_it_fades_out_plays_it_again() {
    let mut g = attack(0.05);
    g.trigger("atk");
    let first = steps(&mut g, 12, 0.05);
    assert_eq!(first.iter().filter(|e| *e == "hitbox").count(), 1, "{first:?}");
    assert_eq!(g.state(0), Some("idle"), "播完正在淡回 idle");
    g.trigger("atk");
    let second = steps(&mut g, 12, 0.05);
    assert_eq!(second.iter().filter(|e| *e == "hitbox").count(), 1, "再按一次照样有判定：{second:?}");
    assert_eq!(second.first().map(String::as_str), Some("swing"), "从头播：{second:?}");
}

#[test]
fn going_back_to_a_fading_looping_state_still_keeps_its_time() {
    let mut g = abc();
    step(&mut g, 0.3);
    g.set_float("to", 1.0);
    step(&mut g, 0.0);
    step(&mut g, 0.4);
    g.set_float("to", 0.0);
    step(&mut g, 0.0);
    assert_clips(&g, 0, &[("a", 0.7, 0.6), ("b", 0.4, 0.4)]);
}

#[test]
fn a_transition_to_the_same_state_starts_it_over() {
    let mut g = graph(json!({
        "params": { "atk": "trigger" },
        "clips": { "hit": { "events": [{ "t": 0, "name": "swing" }] } },
        "start": "attack",
        "states": { "attack": { "clip": "hit", "loop": false, "to": [{ "to": "attack", "when": "atk" }] } },
    }));
    assert_eq!(steps(&mut g, 4, 0.1), ["swing"]);
    g.trigger("atk");
    assert_eq!(step(&mut g, 0.1), ["swing"], "重新开始，起点的事件再发一次");
    assert_clips(&g, 0, &[("hit", 0.0, 1.0)]);
    assert_eq!(g.triggered("atk"), Some(false));
    step(&mut g, 0.1);
    assert_clips(&g, 0, &[("hit", 0.1, 1.0)]);

    // 循环的状态转到自己也从头开始
    let mut g = graph(json!({
        "params": { "again": "trigger" },
        "start": "walk",
        "states": { "walk": { "clip": "walk", "to": [{ "to": "walk", "when": "again", "fade": 0.2 }] } },
    }));
    step(&mut g, 0.3);
    g.trigger("again");
    step(&mut g, 0.1);
    assert_clips(&g, 0, &[("walk", 0.4, 1.0)]);
    step(&mut g, 0.1);
    assert_clips(&g, 0, &[("walk", 0.5, 0.5), ("walk", 0.1, 0.5)]);
}

#[test]
fn a_trigger_nothing_here_can_use_is_dropped_at_the_end_of_the_step() {
    let mut g = graph(json!({
        "params": { "hurt": "trigger" },
        "start": "idle",
        "states": { "idle": { "clip": "idle" }, "hurt": { "clip": "hit", "loop": false, "to": [{ "to": "idle", "exit": 1 }] } },
        "any": [{ "to": "hurt", "when": "hurt" }],
    }));
    g.trigger("hurt");
    step(&mut g, 0.1);
    assert_eq!(g.state(0), Some("hurt"));
    g.trigger("hurt");
    step(&mut g, 0.1);
    assert_eq!(g.triggered("hurt"), Some(false), "受击中又受击：这里没有转换用它，清掉");
    let states: Vec<String> = (0..10)
        .map(|_| {
            step(&mut g, 0.1);
            g.state(0).unwrap().to_owned()
        })
        .collect();
    let back = states.iter().position(|s| s == "idle").expect("播完回 idle");
    assert!(states[back..].iter().all(|s| s == "idle"), "回 idle 以后不再进 hurt：{states:?}");
}

#[test]
fn a_trigger_waits_while_a_transition_here_can_still_use_it() {
    let mut g = graph(json!({
        "params": { "jump": "trigger" },
        "start": "land",
        "states": { "land": { "clip": "land", "loop": false, "to": [{ "to": "jump", "when": "jump", "exit": 1 }] }, "jump": { "clip": "jump" } },
    }));
    g.trigger("jump");
    step(&mut g, 0.1);
    assert_eq!((g.state(0), g.triggered("jump")), (Some("land"), Some(true)), "退出时间没到：留着");
    step(&mut g, 0.1);
    assert_eq!((g.state(0), g.triggered("jump")), (Some("jump"), Some(false)));
}

#[test]
fn a_fade_fires_the_start_events_of_the_new_state_on_the_step_it_enters() {
    let mut g = attack(0.25);
    step(&mut g, 0.1);
    g.trigger("atk");
    assert_eq!(step(&mut g, 0.05), ["swing"], "淡入 0.25 秒：起手的判定在进入的这一步发");
    let rest = steps(&mut g, 8, 0.05);
    assert!(!rest.contains(&"swing".to_owned()), "不重复：{rest:?}");
}

#[test]
fn equal_weights_go_to_the_newer_state() {
    let mut g = graph(json!({
        "params": { "go": false },
        "start": "a",
        "states": { "a": { "clip": "a", "to": [{ "to": "b", "when": "go", "fade": 1 }] }, "b": { "clip": "b" } },
        "clips": { "a": { "events": [{ "t": 0.6, "name": "a" }] }, "b": { "events": [{ "t": 0.45, "name": "b" }] } },
    }));
    step(&mut g, 0.4);
    g.set_bool("go", true);
    step(&mut g, 0.0);
    // 这一步过渡走到一半，推进时两边各 0.5：a 经过 0.6，b 经过 0.45，只有新的 b 发
    assert_eq!(step(&mut g, 0.5), ["b"], "一样重：新的赢");
    assert_clips(&g, 0, &[("a", 0.9, 0.5), ("b", 0.5, 0.5)]);

    // 混合树里两个片段各一半：写在后面的发
    let mut g = locomotion(json!({}));
    g.set_float("speed", 0.5);
    let fired: Vec<String> = steps(&mut g, 4, 0.1);
    assert!(!fired.is_empty() && fired.iter().all(|e| e == "run-step"), "{fired:?}");
}

#[test]
fn heaviest_is_instance_weight_times_clip_weight_and_ties_go_to_the_later_one() {
    let s = |clip, weight| ClipSample { clip, time: 0.0, weight };
    assert_eq!(heaviest(&[s(0, 0.3), s(1, 0.4), s(2, 0.3)]).map(|c| c.clip), Some(1));
    assert_eq!(heaviest(&[s(0, 0.5), s(1, 0.5)]).map(|c| c.clip), Some(1), "一样重：后面的赢");
    assert_eq!(heaviest(&[]).map(|c| c.clip), None);

    // 0.6 的淡出状态里两个片段各一半（0.3），0.4 的新状态只有一个片段：新状态赢
    let mut g = graph(json!({
        "params": { "speed": 0.5, "go": false },
        "clips": { "walk": { "events": [{ "t": 0.95, "name": "walk" }] }, "run": { "events": [{ "t": 0.45, "name": "run" }] }, "a": { "events": [{ "t": 0.35, "name": "a" }] } },
        "start": "move",
        "states": {
            "move": { "blend": { "by": "speed", "clips": [{ "clip": "walk", "at": 0 }, { "clip": "run", "at": 1 }] }, "to": [{ "to": "other", "when": "go", "fade": 1 }] },
            "other": { "clip": "a" },
        },
    }));
    step(&mut g, 0.0);
    g.set_bool("go", true);
    step(&mut g, 0.0);
    step(&mut g, 0.3);
    assert_eq!(step(&mut g, 0.1), ["a"], "按 实例权重 × 片段权重 比");
}

#[test]
fn capping_drops_the_lightest_instance() {
    let states: serde_json::Map<String, Json> = (0..12)
        .map(|i| (format!("s{i}"), json!({ "clip": format!("c{i}"), "to": [{ "to": format!("s{}", (i + 1) % 12), "fade": 100, "exit": 0 }] })))
        .collect();
    let mut g = graph(json!({ "start": "s0", "states": states }));
    step(&mut g, 0.0);
    for _ in 0..20 {
        let before = clips(&g, 0);
        step(&mut g, 0.5);
        let after = clips(&g, 0);
        if before.len() == MAX_INSTANCES {
            let gone: Vec<&(String, f32, f32)> = before.iter().filter(|b| !after.iter().any(|a| a.0 == b.0)).collect();
            let lightest = before.iter().min_by(|a, b| a.2.total_cmp(&b.2)).unwrap();
            assert_eq!(gone, [lightest], "去掉最轻的：{before:?} → {after:?}");
        }
    }
}

#[test]
fn exit_time_tolerates_rounding_when_adding_up_small_steps() {
    // 1.75 秒的片段一步 0.25 秒：f64 累加 7 步得 0.9999999999999998
    let mut g = graph(json!({ "start": "long", "states": { "long": { "clip": "long", "loop": false, "to": [{ "to": "walk", "exit": 1 }] }, "walk": { "clip": "walk" } } }));
    let long = |c: &str| if c == "long" { Some(1.75) } else { durations(c) };
    for _ in 0..6 {
        g.step(0.25, long);
    }
    assert_eq!(g.state(0), Some("long"));
    g.step(0.25, long);
    assert_eq!(g.state(0), Some("walk"), "第 7 步播完");
}

#[test]
fn layer_weight_params_are_clamped() {
    let mut g = graph(json!({
        "params": { "w": 2 },
        "states": { "walk": { "clip": "walk" } },
        "layers": [{ "name": "upper", "weight": "w", "states": { "wave": { "clip": "wave" } } }],
    }));
    step(&mut g, 0.1);
    assert_eq!(g.output()[1].weight, 1.0);
    g.set_float("w", -1.0);
    step(&mut g, 0.1);
    assert_eq!(g.output()[1].weight, 0.0);
}

#[test]
fn compile_rejects_always_true_transitions_clashing_layers_and_repeated_blend_points() {
    let one = |extra: Json| {
        let mut def = json!({ "params": { "x": 0, "y": 0, "w": 1 }, "start": "a", "states": { "a": { "clip": "a" }, "b": { "clip": "b" } } });
        def.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        def
    };
    let cases = [
        (one(json!({ "states": { "a": { "clip": "a", "to": [{ "to": "b", "when": [] }] }, "b": { "clip": "b" } } })), "没有条件也没有 exit"),
        (one(json!({ "any": [{ "to": "b" }] })), "没有条件也没有 exit"),
        (one(json!({ "layers": [{ "name": "base", "states": { "w": { "clip": "wave" } } }] })), "层名重复"),
        (one(json!({ "layers": [{ "name": "u", "states": { "w": { "clip": "wave" } } }, { "name": "u", "states": { "w": { "clip": "wave" } } }] })), "层名重复"),
        (json!({ "params": { "w": 1 }, "layers": [{ "weight": "w", "states": { "w": { "clip": "wave" } } }] }), "第 0 层"),
        (json!({ "layers": [{ "weight": 0.5, "states": { "w": { "clip": "wave" } } }] }), "第 0 层"),
        (one(json!({ "states": { "a": { "blend": { "by": "x", "clips": [{ "clip": "a", "at": 1 }, { "clip": "b", "at": 1 }] } }, "b": { "clip": "b" } } })), "位置重复"),
        (one(json!({ "states": { "a": { "blend": { "by": ["x", "y"], "clips": [{ "clip": "a", "at": [0, 1] }, { "clip": "b", "at": [0, 1] }] } }, "b": { "clip": "b" } } })), "位置重复"),
    ];
    for (def, want) in cases {
        let e = compile_error(def.clone());
        assert!(e.contains(want), "{def}：{e}");
    }
    // 只有 exit、只有条件的都可以；第 0 层写 1 也可以
    graph(one(json!({ "states": { "a": { "clip": "a", "to": [{ "to": "b", "exit": 1 }, { "to": "b", "when": [], "exit": 0.5 }] }, "b": { "clip": "b" } } })));
    graph(json!({ "layers": [{ "weight": 1, "states": { "w": { "clip": "wave" } } }] }));
}
