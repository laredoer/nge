//! 场景完全由 `App.tsx` 声明（模型、名字牌、烟囱粒子、点击效果），Rust 侧只建 App、加插件。

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

/// 生成的宿主（`nsh run` / `nsh package`）要求应用的 crate 有它；TSX 不从这里导入东西，只带上资源。
#[nsh_core::prelude::native_module("scene3d/native")]
fn native(registry: &mut nsh_core::prelude::Registry) {
    registry.assets(assets!());
}

/// 把游戏视图注册进生成的宿主（见 docs/generated-hosts.md）。
#[cfg(target_os = "macos")]
#[nsh_core::prelude::native_components(macos)]
fn components(components: &nsh_platform_macos::Components) {
    components.register(nge_core::GAME_COMPONENT, nge_platform::macos::component(factory()));
}

#[cfg(target_os = "ios")]
#[nsh_core::prelude::native_components(ios)]
fn components(components: &nsh_platform_ios::Components) {
    components.register(nge_core::GAME_COMPONENT, nge_platform::ios::component(factory()));
}

#[cfg(test)]
mod tests {
    use nge_core::Viewport;
    use nge_render3d::Model3d;
    use serde_json::{json, Value};

    use super::*;

    const VIEWPORT: Viewport = Viewport { size: [400.0, 300.0], pixels: [400, 300] };
    const SPARK: &str = "asset:/@scene3d/spark.png";
    const FONT: &str = "asset:/@scene3d/Inter-Bold.ttf";

    /// App.tsx 里 `scene()` 的样子（TS 序列化后），名字牌只留一个。
    fn scene() -> Value {
        let crates = [("red", -2.2, 25), ("green", 0.0, 40), ("blue", 2.2, 60)];
        json!({
            "background": "#0a0f1e",
            "camera3d": { "x": 0, "y": 3, "z": 7.5, "target": [0, 0.4, 0] },
            "lights": [{ "id": "sun", "kind": "directional", "direction": [-1, -2, -1.5] }],
            "ambient": { "brightness": 0.25 },
            "models": crates.map(|(id, x, spin)| json!({ "id": id, "model": "asset:/@scene3d/crate.glb", "x": x, "y": 0.5, "spin": spin, "tap": true })),
            "texts": [
                { "id": "name-green", "text": "green", "follow": "green", "lift": 1.1, "font": FONT, "size": 18 },
                { "id": "hint", "text": "tap a crate", "screen": true, "x": 16, "y": 16, "anchor": "top-left", "font": FONT, "color": "#a0b0d0" }
            ],
            "sprites": [{ "id": "chimney", "color": "#40404a", "w": 28, "h": 90, "x": -230, "y": -150 }],
            "emitters": [{
                "id": "smoke", "x": -230, "y": -105, "rate": 40, "angle": [80, 100], "speed": [30, 70],
                "lifetime": [1.5, 2.5], "gravity": [6, 10], "size": [10, 36], "color": ["#b0b0b880", "#60606800"], "texture": SPARK
            }]
        })
    }

    fn loaded_app() -> App {
        let mut app = app();
        app.prop("scene", Some(&scene()));
        for _ in 0..500 {
            app.tick(VIEWPORT, 0.016);
            let world = app.world_mut();
            if world.query::<&Model3d>().iter(world).count() == 3 {
                return app;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("模型一直没加载完");
    }

    fn tap(app: &mut App, x: f32, y: f32) -> Vec<(String, Value)> {
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": x, "y": y }));
        let mut events = app.tick(VIEWPORT, 0.016);
        app.command("touch", &json!({ "phase": "end", "id": 1, "x": x, "y": y }));
        events.extend(app.tick(VIEWPORT, 0.016));
        events
    }

    #[test]
    fn tapping_a_crate_reports_the_hit_and_empty_space_does_not() {
        let mut app = loaded_app();
        let events = tap(&mut app, 200.0, 150.0);
        let (_, hit) = events.iter().find(|(name, _)| name == "tap").expect("点中中间的箱子");
        assert_eq!(hit["id"], "green");
        assert!(hit["point"].is_array(), "{hit}");
        assert!(tap(&mut app, 200.0, 5.0).iter().all(|(name, _)| name != "tap"), "点天空没反应");
    }

    #[test]
    fn burst_and_float_text_commands_from_tsx() {
        let mut app = loaded_app();
        app.command("burst", &json!({
            "count": 250, "lifetime": [0.4, 0.9], "speed": [80, 280], "gravity": [0, -320], "size": [14, 2],
            "color": ["#ffd060", "#ff300000"], "texture": SPARK, "x": 10, "y": 20
        }));
        app.command("floatText", &json!({ "x": 10, "y": 44, "text": "+10", "font": FONT, "color": "#ffe680" }));
        app.tick(VIEWPORT, 0.016);
        let world = app.world_mut();
        assert_eq!(world.query::<&nge_text::FloatingText>().iter(world).count(), 1);
        assert_eq!(world.query::<&nge_particles::ParticleEmitter>().iter(world).count(), 2, "烟囱 + 爆炸");
    }
}
