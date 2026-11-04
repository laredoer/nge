use nge_ecs::App;
use nge_render2d::{Sprite, SpriteImage, Transform2d};
use serde_json::{Value as JsonValue, json};

use super::*;
use crate::testing::{VIEWPORT, app};

fn tick(app: &mut App, dt: f32) -> Vec<(String, JsonValue)> {
    app.tick(VIEWPORT, dt)
}

fn scene(app: &mut App, sprites: JsonValue) {
    app.prop("scene", Some(&json!({ "sprites": sprites })));
}

fn get(app: &mut App, id: &str) -> (Transform2d, Sprite) {
    let world = app.world_mut();
    let (_, t, s) = world.query::<(&SceneId, &Transform2d, &Sprite)>().iter(world).find(|(i, ..)| i.0 == id).unwrap();
    (*t, *s)
}

fn frame_of(app: &mut App, id: &str) -> u32 {
    match get(app, id).1.image {
        SpriteImage::Atlas(_, f) => f,
        SpriteImage::Texture(_) => panic!("不是图集"),
    }
}

fn names(events: &[(String, JsonValue)], name: &str) -> Vec<JsonValue> {
    events.iter().filter(|(n, _)| n == name).map(|(_, d)| d.clone()).collect()
}

#[test]
fn anim_loops_and_survives_other_changes() {
    let mut app = app();
    scene(&mut app, json!([{ "id": "a", "texture": "grid", "frame": 7, "anim": { "frames": [1, 2, 3], "fps": 10 } }]));
    tick(&mut app, 0.0);
    assert_eq!(frame_of(&mut app, "a"), 1, "有 anim 时 frame 不起作用");
    let frames: Vec<u32> = (0..4).map(|_| (tick(&mut app, 0.1), frame_of(&mut app, "a")).1).collect();
    assert_eq!(frames, [2, 3, 1, 2]);

    scene(&mut app, json!([{ "id": "a", "x": 5, "texture": "grid", "anim": { "frames": [1, 2, 3], "fps": 10 } }]));
    tick(&mut app, 0.1);
    assert_eq!(frame_of(&mut app, "a"), 3, "只改了 x，动画接着播");

    scene(&mut app, json!([{ "id": "a", "texture": "grid", "anim": { "frames": [5, 6], "fps": 10 } }]));
    tick(&mut app, 0.0);
    assert_eq!(frame_of(&mut app, "a"), 5, "anim 变了从头开始");

    tick(&mut app, 1000.15);
    assert_eq!(frame_of(&mut app, "a"), 6, "很大的 delta 按时间取模");
}

#[test]
fn anim_once_stops_on_the_last_frame_and_reports() {
    let mut app = app();
    scene(&mut app, json!([{ "id": "a", "texture": "grid", "anim": { "frames": [1, 2], "fps": 10, "loop": false } }]));
    tick(&mut app, 0.0);
    assert!(names(&tick(&mut app, 0.15), "animEnd").is_empty());
    assert_eq!(names(&tick(&mut app, 0.1), "animEnd"), [json!({ "id": "a" })]);
    assert!(names(&tick(&mut app, 0.1), "animEnd").is_empty(), "只发一次");
    assert_eq!(frame_of(&mut app, "a"), 2);
}

#[test]
fn invalid_anim_is_ignored() {
    let mut app = app();
    scene(&mut app, json!([
        { "id": "a", "texture": "grid", "frame": 3, "anim": { "frames": [] } },
        { "id": "b", "texture": "grid", "frame": 3, "anim": { "frames": [1], "fps": 0 } },
    ]));
    tick(&mut app, 0.5);
    assert_eq!((frame_of(&mut app, "a"), frame_of(&mut app, "b")), (3, 3));
    let world = app.world_mut();
    assert_eq!(world.query::<&FrameAnimation>().iter(world).count(), 0);
}

#[test]
fn tween_moves_rotates_fades_and_reports() {
    let mut app = app();
    scene(&mut app, json!([{ "id": "a", "w": 10 }]));
    tick(&mut app, 0.0);
    app.command("tween", &json!({ "id": "a", "to": { "x": 100, "rotation": 90, "alpha": 0 }, "duration": 1, "ease": "linear", "tag": "go" }));
    tick(&mut app, 0.0);
    tick(&mut app, 0.5);
    let (t, s) = get(&mut app, "a");
    assert!((t.translation.x - 50.0).abs() < 1e-3 && (t.rotation - 45f32.to_radians()).abs() < 1e-4 && (s.color.w - 0.5).abs() < 1e-4);
    let events = tick(&mut app, 0.6);
    assert_eq!(names(&events, "tweenEnd"), [json!({ "id": "a", "tag": "go", "cancelled": false })]);
    let (t, s) = get(&mut app, "a");
    assert_eq!((t.translation.x, s.color.w), (100.0, 0.0), "播完停在终点");
}

#[test]
fn delay_and_start_from_current_value() {
    let mut app = app();
    scene(&mut app, json!([{ "id": "a", "x": 10 }]));
    tick(&mut app, 0.0);
    app.command("tween", &json!({ "id": "a", "to": { "x": 20 }, "duration": 1, "delay": 1, "ease": "linear" }));
    tick(&mut app, 0.5);
    assert_eq!(get(&mut app, "a").0.translation.x, 10.0, "delay 期间不动");
    app.world_mut().query::<&mut Transform2d>().iter_mut(app.world_mut()).for_each(|mut t| t.translation.x = 0.0);
    tick(&mut app, 0.5);
    tick(&mut app, 0.5);
    assert!((get(&mut app, "a").0.translation.x - 10.0).abs() < 1e-3, "起点取开始时的值（0），一半是 10");
}

#[test]
fn yoyo_repeat_and_forever() {
    let mut app = app();
    scene(&mut app, json!([{ "id": "a" }, { "id": "b" }]));
    tick(&mut app, 0.0);
    app.command("tween", &json!({ "id": "a", "to": { "scale": 2 }, "duration": 1, "ease": "linear", "repeat": 1, "yoyo": true }));
    app.command("tween", &json!({ "id": "b", "to": { "y": 10 }, "duration": 1, "ease": "linear", "repeat": -1 }));
    tick(&mut app, 0.0);
    tick(&mut app, 0.5);
    assert!((get(&mut app, "a").0.scale.x - 1.5).abs() < 1e-4);
    tick(&mut app, 1.0);
    assert!((get(&mut app, "a").0.scale.x - 1.5).abs() < 1e-4, "第二次往回走");
    let events = tick(&mut app, 0.6);
    assert_eq!(names(&events, "tweenEnd"), [json!({ "id": "a", "tag": null, "cancelled": false })]);
    assert_eq!(get(&mut app, "a").0.scale.x, 1.0, "yoyo 播完回到起点");
    tick(&mut app, 100.25);
    assert!((get(&mut app, "b").0.translation.y - 3.5).abs() < 1e-2, "一直播的每次从头开始");
}

#[test]
fn newer_tween_takes_over_its_properties() {
    let mut app = app();
    scene(&mut app, json!([{ "id": "a" }]));
    tick(&mut app, 0.0);
    app.command("tween", &json!({ "id": "a", "to": { "x": 100, "y": 100 }, "duration": 1, "ease": "linear", "tag": "old" }));
    app.command("tween", &json!({ "id": "a", "to": { "x": -100 }, "duration": 1, "ease": "linear", "tag": "new" }));
    tick(&mut app, 0.0);
    tick(&mut app, 0.5);
    let t = get(&mut app, "a").0;
    assert!((t.translation.x + 50.0).abs() < 1e-3 && (t.translation.y - 50.0).abs() < 1e-3);
    app.command("tween", &json!({ "id": "a", "to": { "y": 0 }, "duration": 0 }));
    let events = tick(&mut app, 0.0);
    assert_eq!(get(&mut app, "a").0.translation.y, 0.0, "duration 为 0 直接到终点");
    assert_eq!(
        names(&events, "tweenEnd"),
        [json!({ "id": "a", "tag": "old", "cancelled": true }), json!({ "id": "a", "tag": null, "cancelled": false })],
        "属性全被接管的旧补间发 cancelled"
    );
    let events = tick(&mut app, 0.6);
    assert_eq!(names(&events, "tweenEnd"), [json!({ "id": "a", "tag": "new", "cancelled": false })]);
}

#[test]
fn dynamic_bodies_only_tween_scale_and_alpha() {
    let mut app = app();
    scene(&mut app, json!([{ "id": "a", "w": 10, "body": {} }, { "id": "k", "w": 10, "body": { "kind": "kinematic" } }]));
    tick(&mut app, 0.0);
    app.command("tween", &json!({ "id": "a", "to": { "x": 100, "scale": 2 }, "duration": 0 }));
    app.command("tween", &json!({ "id": "k", "to": { "x": 100 }, "duration": 0 }));
    app.command("tween", &json!({ "id": "missing", "to": { "x": 1 } }));
    tick(&mut app, 0.0);
    let (a, k) = (get(&mut app, "a").0, get(&mut app, "k").0);
    assert_eq!((a.translation.x, a.scale.x), (0.0, 2.0));
    assert_eq!(k.translation.x, 100.0, "kinematic 可以补间位置");
}

#[test]
fn eases_start_at_zero_and_end_at_one() {
    for ease in [Ease::Linear, Ease::EaseIn, Ease::EaseOut, Ease::EaseInOut, Ease::BackOut, Ease::BounceOut, Ease::ElasticOut] {
        assert!(ease.apply(0.0).abs() < 1e-5 && (ease.apply(1.0) - 1.0).abs() < 1e-5, "{ease:?}");
    }
    assert!(Ease::BackOut.apply(0.7) > 1.0, "backOut 会冲过头");
    assert_eq!(Ease::EaseOut.apply(f32::NAN), 0.0);
}

#[test]
fn explicit_from_keeps_an_interrupted_pulse_on_track() {
    let mut app = app();
    scene(&mut app, json!([{ "id": "a" }]));
    tick(&mut app, 0.0);
    let pulse = json!({ "id": "a", "from": { "scale": 1 }, "to": { "scale": 1.4 }, "duration": 0.15, "ease": "linear", "yoyo": true, "repeat": 1, "tag": "pulse" });
    app.command("tween", &pulse);
    tick(&mut app, 0.0);
    tick(&mut app, 0.1);
    app.command("tween", &pulse);
    let mut ends = Vec::new();
    for _ in 0..30 {
        ends.extend(names(&tick(&mut app, 1.0 / 60.0), "tweenEnd"));
    }
    assert_eq!(ends, [json!({ "id": "a", "tag": "pulse", "cancelled": true }), json!({ "id": "a", "tag": "pulse", "cancelled": false })]);
    assert_eq!(get(&mut app, "a").0.scale.x, 1.0, "连点也回到 1");
}

#[test]
fn delayed_tween_takes_over_when_it_starts() {
    let mut app = app();
    scene(&mut app, json!([{ "id": "a" }]));
    tick(&mut app, 0.0);
    app.command("tween", &json!({ "id": "a", "to": { "x": 100 }, "duration": 2, "ease": "linear", "tag": "old" }));
    tick(&mut app, 0.0);
    tick(&mut app, 0.5);
    app.command("tween", &json!({ "id": "a", "to": { "x": 0 }, "duration": 1, "delay": 0.5, "ease": "linear" }));
    tick(&mut app, 0.25);
    assert!((get(&mut app, "a").0.translation.x - 37.5).abs() < 1e-3, "delay 期间旧补间接着播");
    let events = tick(&mut app, 0.25);
    assert_eq!(names(&events, "tweenEnd"), [json!({ "id": "a", "tag": "old", "cancelled": true })]);
    assert!((get(&mut app, "a").0.translation.x - 37.5).abs() < 1e-3, "从开始时的值接着走");
    tick(&mut app, 0.5);
    assert!((get(&mut app, "a").0.translation.x - 18.75).abs() < 1e-3);
}

#[test]
fn rotation_tween_wins_over_spin_while_it_runs() {
    let mut app = app();
    scene(&mut app, json!([{ "id": "a", "spin": 90 }]));
    tick(&mut app, 0.0);
    app.command("tween", &json!({ "id": "a", "to": { "rotation": 90 }, "duration": 1, "ease": "linear" }));
    tick(&mut app, 0.0);
    tick(&mut app, 0.5);
    assert!((get(&mut app, "a").0.rotation.to_degrees() - 45.0).abs() < 1e-3);
    tick(&mut app, 0.6);
    assert!((get(&mut app, "a").0.rotation.to_degrees() - 90.0).abs() < 1e-3);
    tick(&mut app, 0.1);
    assert!((get(&mut app, "a").0.rotation.to_degrees() - 99.0).abs() < 1e-3, "播完 spin 接着转");
}

#[test]
fn tsx_changes_during_and_after_a_tween() {
    let mut app = app();
    scene(&mut app, json!([{ "id": "a" }]));
    tick(&mut app, 0.0);
    app.command("tween", &json!({ "id": "a", "to": { "x": 100 }, "duration": 1, "ease": "linear" }));
    tick(&mut app, 0.0);
    tick(&mut app, 0.5);
    scene(&mut app, json!([{ "id": "a", "x": 7 }]));
    tick(&mut app, 0.0);
    assert!((get(&mut app, "a").0.translation.x - 50.0).abs() < 1e-3, "补间期间以补间为准");
    tick(&mut app, 0.6);
    scene(&mut app, json!([{ "id": "a", "x": 8 }]));
    tick(&mut app, 0.0);
    assert_eq!(get(&mut app, "a").0.translation.x, 8.0, "播完以后以 TSX 为准");
}

#[test]
fn bad_tween_commands_are_dropped() {
    let mut app = app();
    scene(&mut app, json!([{ "id": "a", "w": 10, "body": {} }, { "id": "b" }]));
    tick(&mut app, 0.0);
    app.command("tween", &json!({ "id": "a", "to": { "x": 1, "y": 1, "rotation": 1 } }));
    app.command("tween", &json!({ "id": "b", "to": { "opacity": 0 } }));
    app.command("tween", &json!({ "id": "b", "to": {} }));
    tick(&mut app, 0.0);
    let world = app.world_mut();
    assert_eq!(world.query::<&Tweens>().iter(world).count(), 0, "dynamic 只写位置 / 旋转、写错字段、空的 to 都不建补间");
}

#[test]
fn same_frame_later_wins_and_waiting_tweens_are_not_taken_over() {
    let mut app = app();
    scene(&mut app, json!([{ "id": "a" }]));
    tick(&mut app, 0.0);
    app.command("tween", &json!({ "id": "a", "to": { "x": 100 }, "duration": 1, "ease": "linear", "tag": "first" }));
    app.command("tween", &json!({ "id": "a", "to": { "x": -100 }, "duration": 1, "ease": "linear", "tag": "second" }));
    app.command("tween", &json!({ "id": "a", "to": { "y": 50 }, "duration": 0, "delay": 2, "tag": "later" }));
    app.command("tween", &json!({ "id": "a", "to": { "y": 10 }, "duration": 1, "ease": "linear", "tag": "y" }));
    let events = tick(&mut app, 0.0);
    assert_eq!(names(&events, "tweenEnd"), [json!({ "id": "a", "tag": "first", "cancelled": true })], "同一帧开始的后发的赢");
    tick(&mut app, 0.5);
    let t = get(&mut app, "a").0;
    assert!((t.translation.x + 50.0).abs() < 1e-3 && (t.translation.y - 5.0).abs() < 1e-3, "还在 delay 里的不影响别人");
    let events = tick(&mut app, 1.6);
    assert_eq!(
        names(&events, "tweenEnd"),
        [
            json!({ "id": "a", "tag": "y", "cancelled": true }),
            json!({ "id": "a", "tag": "second", "cancelled": false }),
            json!({ "id": "a", "tag": "later", "cancelled": false }),
        ],
        "later 在这一帧开始，接管了本来也要在这一帧播完的 y，y 只发 cancelled"
    );
    assert_eq!(get(&mut app, "a").0.translation.y, 50.0);
}
