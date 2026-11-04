//! 场景完全由 `App.tsx` 声明（地面、墙、箱子、弹球的刚体），Rust 侧只建 App、加插件。

use nge_assets::AssetPlugin;
use nge_ecs::App;
use nge_render2d::Render2dPlugin;
use nge_render3d::Render3dPlugin;
use nge_scene::ScenePlugin;
use nsh_core::prelude::assets;

pub fn app() -> App {
    let mut app = App::new();
    app.add_plugins(AssetPlugin::new().files(assets!().iter().map(|f| (f.uri, f.source))))
        .add_plugins(Render3dPlugin)
        .add_plugins(Render2dPlugin)
        .add_plugins(ScenePlugin::new());
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

    const VIEWPORT: Viewport = Viewport { size: [800.0, 600.0], pixels: [800, 600] };
    const FLOOR: f32 = -260.0;
    const WALL: f32 = 320.0;

    /// App.tsx 里 `scene()` 的样子（TS 序列化后），`balls` 是 (x, y, r)。
    fn scene(balls: &[(f32, f32, f32)]) -> Value {
        let wall = |id: &str, x: f32| json!({ "id": id, "x": x, "y": FLOOR + 480.0, "w": 20, "h": 1000, "body": { "kind": "fixed" } });
        let mut sprites = vec![
            json!({ "id": "sky", "w": 4000, "h": 4000, "z": -10, "color": "#00000000", "tap": true }),
            json!({ "id": "floor", "y": FLOOR - 20.0, "w": WALL * 2.0 + 40.0, "h": 40, "body": { "kind": "fixed" } }),
            wall("wall-l", -WALL - 10.0),
            wall("wall-r", WALL + 10.0),
        ];
        sprites.extend((0..8).map(|i| {
            let (x, y) = (120.0 + (i % 2) as f32 * 44.0 - 22.0, FLOOR + 22.0 + (i / 2) as f32 * 44.0);
            json!({ "id": format!("crate-{i}"), "x": x, "y": y, "w": 40, "body": { "friction": 0.8 } })
        }));
        sprites.extend(balls.iter().enumerate().map(|(i, &(x, y, r))| {
            json!({ "id": format!("ball-{i}"), "x": x, "y": y, "w": r * 2.0, "z": 1, "tap": true, "texture": "asset:/@physics/spark.png",
                "body": { "shape": "circle", "restitution": 0.5, "collide": true } })
        }));
        json!({ "background": "#10131c", "sprites": sprites })
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
        app.prop("scene", Some(&scene(&[(-100.0, 100.0, 10.0)])));
        let events = run(&mut app, 180);
        for i in 0..8 {
            let p = position(&mut app, &format!("crate-{i}"));
            let want = nge_ecs::glam::Vec2::new(120.0 + (i % 2) as f32 * 44.0 - 22.0, FLOOR + 20.0 + (i / 2) as f32 * 40.0);
            assert!((p - want).length() < 5.0, "箱子 {i} 没倒: {p} vs {want}");
        }
        let ball = position(&mut app, "ball-0");
        assert!((ball.y - (FLOOR + 10.0)).abs() < 2.0, "球停在地上: {ball}");
        assert!(events.iter().any(|(n, d)| n == "collide" && d["started"] == true && (d["a"] == "floor" || d["b"] == "floor")), "撞地面发事件");
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
}
