//! M3 验收：场景完全由 `App.tsx` 声明，Rust 侧只注册纹理。

use nge_ecs::App;
use nge_render2d::{Grid, Render2dPlugin};
use nge_render3d::Render3dPlugin;
use nge_scene::ScenePlugin;

pub fn factory() -> nge_core::GameFactory {
    nge_ecs::factory(|| {
        let mut app = App::new();
        app.add_plugins(Render3dPlugin).add_plugins(Render2dPlugin).add_plugins(ScenePlugin::new().atlas(
            "ships",
            include_bytes!("../assets/ships.png"),
            Grid { cols: 4, rows: 4 },
        ));
        app
    })
}

/// 生成的宿主（`nsh run` / `nsh package`）要求应用的 crate 有它；TSX 不从这里导入东西。
#[nsh_core::prelude::native_module("scene/native")]
fn native(_registry: &mut nsh_core::prelude::Registry) {
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
    use nge_render2d::{Sprite, SpriteImage, Transform2d};
    use nge_scene::SceneId;
    use serde_json::json;

    use super::*;

    const VIEWPORT: Viewport = Viewport { size: [400.0, 600.0], pixels: [400, 600] };

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(nge_audio::AudioPlugin::mock()).add_plugins(Render3dPlugin).add_plugins(Render2dPlugin).add_plugins(
            ScenePlugin::new().atlas("ships", include_bytes!("../assets/ships.png"), Grid { cols: 4, rows: 4 }),
        );
        app
    }

    fn sprite(app: &mut App, id: &str) -> (Transform2d, Sprite) {
        let world = app.world_mut();
        let (_, t, s) = world.query::<(&SceneId, &Transform2d, &Sprite)>().iter(world).find(|(i, ..)| i.0 == id).unwrap();
        (*t, *s)
    }

    /// 和 App.tsx 一样的声明：石头按帧换颜色，点飞船后补间到目标位置并发 tweenEnd，相机跟着飞船。
    #[test]
    fn rocks_animate_and_ship_tweens() {
        let mut app = app();
        let frames: Vec<u32> = (0..8).map(|k| 4 + k % 8).collect();
        let camera = json!({ "follow": "ship", "y": 120, "smooth": 0.25, "bounds": { "left": -400, "right": 400, "bottom": -500, "top": 500 } });
        app.prop("scene", Some(&json!({ "camera": camera, "sprites": [
            { "id": "rock-0", "texture": "ships", "anim": { "frames": frames, "fps": 4 }, "w": 40 },
            { "id": "ship", "texture": "ships", "frame": 0, "w": 64, "y": -200, "rotation": 180, "z": 10, "tap": true },
        ] })));
        app.tick(VIEWPORT, 0.0);
        app.tick(VIEWPORT, 0.3);
        assert!(matches!(sprite(&mut app, "rock-0").1.image, SpriteImage::Atlas(_, 5)));

        // 连点两次：第一次的移动和缩放被第二次接管
        let tap = |app: &mut App, x: f32| {
            app.command("tween", &json!({ "id": "ship", "to": { "x": x, "y": -150 }, "duration": 0.6, "ease": "easeOut", "tag": "move" }));
            let pulse = json!({ "id": "ship", "from": { "scale": 1 }, "to": { "scale": 1.4 }, "duration": 0.15, "ease": "easeOut", "yoyo": true, "repeat": 1 });
            app.command("tween", &pulse);
            app.command("shake", &json!({ "strength": 6, "duration": 0.25 }));
        };
        let mut ends = Vec::new();
        let mut run = |app: &mut App, frames: usize| {
            for _ in 0..frames {
                ends.extend(app.tick(VIEWPORT, 1.0 / 60.0).into_iter().filter(|(n, _)| n == "tweenEnd").map(|(_, d)| d));
            }
        };
        tap(&mut app, 100.0);
        run(&mut app, 6);
        tap(&mut app, -80.0);
        run(&mut app, 50);
        let end = |tag: Option<&str>, cancelled: bool| json!({ "id": "ship", "tag": tag, "cancelled": cancelled });
        assert_eq!(ends, [end(Some("move"), true), end(None, true), end(None, false), end(Some("move"), false)]);
        let t = sprite(&mut app, "ship").0;
        assert_eq!((t.translation.x, t.translation.y, t.scale.x), (-80.0, -150.0, 1.0), "停在第二次的目标，大小复原");
        for _ in 0..60 {
            app.tick(VIEWPORT, 1.0 / 60.0);
        }
        let world = app.world_mut();
        let cam = world.query::<&nge_render2d::Camera2d>().single(world).unwrap().position;
        assert!((cam - nge_ecs::glam::Vec2::new(-80.0, -30.0)).length() < 0.5, "相机追上飞船（偏移 y 120），震屏已停 {cam}");
    }
}
