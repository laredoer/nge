//! 场景完全由 `App.tsx` 声明（地面、墙、箱子、弹球的刚体，音乐和音效），Rust 侧只建 App、加插件，
//! 外加空间查询的可视化（[`queries`]）。

mod queries;

use nge_assets::AssetPlugin;
use nge_audio::AudioPlugin;
use nge_ecs::{App, Update};
use nge_render2d::Render2dPlugin;
use nge_render3d::Render3dPlugin;
use nge_scene::ScenePlugin;
use nsh_core::prelude::assets;

pub fn app() -> App {
    build(AudioPlugin::default())
}

fn build(audio: AudioPlugin) -> App {
    let mut app = App::new();
    app.add_plugins(AssetPlugin::new().files(assets!().iter().map(|f| (f.uri, f.source))))
        .add_plugins(audio)
        .add_plugins(Render3dPlugin)
        .add_plugins(Render2dPlugin)
        .add_plugins(ScenePlugin::new())
        .add_prop(queries::SHOW_QUERIES)
        .add_systems(Update, queries::show_queries);
    app
}

pub fn factory() -> nge_core::GameFactory {
    nge_ecs::factory(app)
}

#[cfg(test)]
mod tests {
    use nge_core::Viewport;
    use nge_render2d::Transform2d;
    use nge_scene::SceneId;
    use serde_json::{json, Value};

    use super::*;

    /// 测试不出声。
    fn app() -> App {
        build(AudioPlugin::mock())
    }

    const VIEWPORT: Viewport = Viewport { size: [800.0, 600.0], pixels: [800, 600] };
    const FLOOR: f32 = -260.0;
    const WALL: f32 = 320.0;

    /// App.tsx 里 `scene()` 的样子（TS 序列化后），`balls` 是 (x, y, r)。
    fn scene(balls: &[(f32, f32, f32)]) -> Value {
        let mut sprites = vec![json!({ "id": "sky", "w": 4000, "h": 4000, "z": -10, "color": "#00000000", "tap": true })];
        sprites.extend((0..8).map(|i| {
            let (x, y) = (120.0 + (i % 2) as f32 * 44.0 - 22.0, FLOOR + 22.0 + (i / 2) as f32 * 44.0);
            json!({ "id": format!("crate-{i}"), "x": x, "y": y, "w": 40, "body": { "friction": 0.8 } })
        }));
        sprites.extend(balls.iter().enumerate().map(|(i, &(x, y, r))| {
            json!({ "id": format!("ball-{i}"), "x": x, "y": y, "w": r * 2.0, "z": 1, "tap": true, "texture": "asset:/@physics/spark.png",
                "body": { "shape": "circle", "restitution": 0.5, "collide": true } })
        }));
        json!({ "background": "#10131c", "sprites": sprites, "tilemaps": [level()],
            "audio": { "music": "asset:/@physics/music.wav", "musicVolume": 0.5, "preload": ["asset:/@physics/ding.wav", "asset:/@physics/pop.wav"] } })
    }

    /// App.tsx 里的 `LEVEL`：18 列 26 行，最底下一行地面，两边是墙，两块平台。
    fn level() -> Value {
        let (tile, cols, rows) = (40.0, 18usize, 26usize);
        let platforms = [(21, 3, 6), (18, 11, 14)];
        let data: Vec<i32> = (0..cols * rows)
            .map(|i| {
                let (c, r) = (i % cols, i / cols);
                if r == rows - 1 {
                    1
                } else if c == 0 || c == cols - 1 {
                    0
                } else if platforms.iter().any(|&(row, from, to)| r == row && (from..=to).contains(&c)) {
                    2
                } else {
                    -1
                }
            })
            .collect();
        json!({ "id": "level", "texture": "asset:/@physics/tiles.png", "grid": [3, 1], "tile": tile, "cols": cols,
            "x": -WALL - tile, "y": FLOOR + tile * (rows - 1) as f32, "data": data, "solid": [0, 1, 2], "collide": true })
    }

    fn position(app: &mut App, id: &str) -> nge_ecs::glam::Vec2 {
        let world = app.world_mut();
        world.query::<(&SceneId, &Transform2d)>().iter(world).find(|(s, _)| s.0 == id).map(|(_, t)| t.translation).unwrap()
    }

    fn run(app: &mut App, frames: usize) -> Vec<(String, Value)> {
        (0..frames).flat_map(|_| app.tick(VIEWPORT, 1.0 / 60.0)).collect()
    }

    #[test]
    fn crates_stay_stacked_and_balls_land_and_collide() {
        let mut app = app();
        app.prop("scene", Some(&scene(&[(0.0, 100.0, 10.0), (-160.0, 100.0, 10.0)])));
        let events = run(&mut app, 180);
        for i in 0..8 {
            let p = position(&mut app, &format!("crate-{i}"));
            let want = nge_ecs::glam::Vec2::new(120.0 + (i % 2) as f32 * 44.0 - 22.0, FLOOR + 20.0 + (i / 2) as f32 * 40.0);
            assert!((p - want).length() < 5.0, "箱子 {i} 没倒: {p} vs {want}");
        }
        let ball = position(&mut app, "ball-0");
        assert!((ball.y - (FLOOR + 10.0)).abs() < 2.0, "球停在地上: {ball}");
        let ball = position(&mut app, "ball-1");
        assert!((ball.y - (-100.0 + 10.0)).abs() < 2.0, "球停在平台上: {ball}");
        assert!(events.iter().any(|(n, d)| n == "collide" && d["started"] == true && (d["a"] == "level" || d["b"] == "level")), "撞地图发事件");
    }

    #[test]
    fn tapping_empty_space_hits_the_sky_and_balls_can_be_pushed() {
        let mut app = app();
        app.prop("scene", Some(&scene(&[])));
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 100, "y": 100 }));
        let events = run(&mut app, 1);
        let tap = events.iter().find(|(n, _)| n == "tap").expect("点空地");
        assert_eq!(tap.1["id"], "sky");
        app.command("touch", &json!({ "phase": "end", "id": 1, "x": 100, "y": 100 }));

        app.prop("scene", Some(&scene(&[(0.0, FLOOR + 10.0, 10.0)])));
        run(&mut app, 30);
        app.command("push", &json!({ "id": "ball-0", "x": 0, "y": 600 }));
        run(&mut app, 10);
        assert!(position(&mut app, "ball-0").y > FLOOR + 50.0, "被弹起来");
        assert!(position(&mut app, "crate-0").y < FLOOR + 30.0, "加球不动箱子");
    }

    #[test]
    fn music_loops_and_sounds_play() {
        let mut app = app();
        app.prop("scene", Some(&scene(&[])));
        run(&mut app, 1);
        nge_assets::AssetServer::apply(app.world_mut(), true);
        run(&mut app, 1);
        assert_eq!(app.world().resource::<nge_audio::Audio>().voices(), 1, "背景音乐");
        app.command("sound", &json!({ "src": "asset:/@physics/ding.wav", "volume": 0.35, "pitch": 1.4 }));
        run(&mut app, 1);
        assert_eq!(app.world().resource::<nge_audio::Audio>().voices(), 2);
    }

    fn overlays(app: &mut App) -> usize {
        let world = app.world_mut();
        world.query_filtered::<(), bevy_ecs::query::With<queries::Overlay>>().iter(world).count()
    }

    #[test]
    fn queries_are_drawn_only_while_switched_on() {
        let mut app = app();
        app.prop("scene", Some(&scene(&[(0.0, FLOOR + 20.0, 10.0)])));
        run(&mut app, 30);
        assert_eq!(overlays(&mut app), 0);
        app.prop("queries", Some(&json!(true)));
        run(&mut app, 2);
        // 射线至少碰到地面（线、法线、交点三块），框和点各一块，加上点的十字
        assert!(overlays(&mut app) >= 7, "{}", overlays(&mut app));
        let n = overlays(&mut app);
        run(&mut app, 10);
        assert!(overlays(&mut app).abs_diff(n) < 10, "每帧重画，不越攒越多");
        app.prop("queries", Some(&json!(false)));
        run(&mut app, 2);
        assert_eq!(overlays(&mut app), 0, "关掉后清干净");
    }
}
