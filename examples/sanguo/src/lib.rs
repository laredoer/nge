//! 长坂坡：2.5D 横版清版动作。玩法（移动、连击、敌人、波次、镜头）全在 TSX 里按固定步长算，每帧把场景声明下来；
//! Rust 只注册插件、把资源打包进去。

use nge_assets::AssetPlugin;
use nge_audio::AudioPlugin;
use nge_ecs::App;
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
        .add_plugins(ScenePlugin::new());
    app
}

pub fn factory() -> nge_core::GameFactory {
    nge_ecs::factory(app)
}

/// `nsh run` / `nsh package` 生成的宿主要求应用的 crate 有它；TSX 不从这里导入东西，只带上资源（见 docs/native-components.md）。
/// web 上生成的运行时不调组件注册函数，游戏视图在这里注册（App.tsx 要 `import "sanguo/native"`，见 docs/web.md）。
#[nsh_core::prelude::native_module("sanguo/native")]
fn native(registry: &mut nsh_core::prelude::Registry) {
    registry.assets(assets!());
    #[cfg(target_arch = "wasm32")]
    nge_platform::web::register(factory(), assets!().iter().map(|f| f.uri));
    // Android 同样不调组件注册函数（见 docs/android.md）
    #[cfg(target_os = "android")]
    nge_platform::android::register(factory());
}

/// 把游戏视图注册进生成的宿主。
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
    use bevy_ecs::prelude::Mut;
    use nge_assets::Assets;
    use nge_core::Viewport;
    use nge_render3d::{Animator, Assets3d, Image, Model3d, Pose, Transform3d};
    use serde_json::Value;

    use super::*;

    const VIEWPORT: Viewport = Viewport { size: [800.0, 600.0], pixels: [800, 600] };
    const GLBS: [&str; 7] = ["ground", "wall", "cart", "cart_fire", "banner", "barricade", "bridge"];
    const PNGS: [&str; 5] = ["player", "troops", "boss", "fx", "hills"];
    const WAVS: [&str; 6] = ["swing", "hit", "arrow", "hooves", "win", "lose"];

    fn ts() -> String {
        [include_str!("../view.ts"), include_str!("../effects.ts"), include_str!("../sheets.json")].concat()
    }

    /// TS 里用到的资源都打包了，打包的也都用到了；生成脚本放在 tools/，不打包。
    #[test]
    fn assets_are_bundled_and_used() {
        let uris: Vec<_> = assets!().iter().map(|f| f.uri).collect();
        let ts = ts();
        let names = GLBS.iter().map(|n| format!("{n}.glb")).chain(PNGS.iter().map(|n| format!("{n}.png"))).chain(WAVS.iter().map(|n| format!("{n}.wav")));
        for name in names {
            let uri = format!("asset:/@sanguo/{name}");
            assert!(uris.contains(&uri.as_str()), "{uri} 没打包 {uris:?}");
            let stem = name.split('.').next().unwrap();
            assert!(ts.contains(&uri) || (name.ends_with(".glb") && ts.contains(&format!("\"{stem}\""))), "{uri} 没用到");
        }
        assert_eq!(uris.len(), GLBS.len() + PNGS.len() + WAVS.len(), "{uris:?}");
        assert!(!uris.iter().any(|u| u.ends_with(".py")));
    }

    /// 图集的大小和 sheets.json 写的格子对得上。
    #[test]
    fn atlases_match_the_sheets() {
        let sheets: Value = serde_json::from_str(include_str!("../sheets.json")).unwrap();
        let files: [(&str, &[u8]); 4] = [
            ("player", include_bytes!("../assets/player.png")),
            ("troops", include_bytes!("../assets/troops.png")),
            ("boss", include_bytes!("../assets/boss.png")),
            ("fx", include_bytes!("../assets/fx.png")),
        ];
        for (name, bytes) in files {
            let image = nge_assets::Image::decode(bytes).unwrap();
            let s = &sheets[name];
            let w = s["grid"][0].as_u64().unwrap() * s["size"][0].as_u64().unwrap();
            let h = s["grid"][1].as_u64().unwrap() * s["size"][1].as_u64().unwrap();
            assert_eq!((image.size.x as u64, image.size.y as u64), (w, h), "{name}");
        }
    }

    fn load(app: &mut App, bytes: &[u8]) -> nge_render3d::ModelId {
        app.world_mut().resource_scope(|world, mut assets: Mut<Assets3d>| assets.load_gltf(&mut world.resource_mut::<Assets<Image>>(), bytes)).unwrap()
    }

    /// 物件都能加载；着火的粮车有发光的火星；旌旗带蒙皮，播 wave 时旗尾在动。
    #[test]
    fn models_load_and_the_banner_waves() {
        let mut app = App::new();
        app.add_plugins(Render3dPlugin);
        let bytes: [&[u8]; 7] = [
            include_bytes!("../assets/ground.glb"),
            include_bytes!("../assets/wall.glb"),
            include_bytes!("../assets/cart.glb"),
            include_bytes!("../assets/cart_fire.glb"),
            include_bytes!("../assets/banner.glb"),
            include_bytes!("../assets/barricade.glb"),
            include_bytes!("../assets/bridge.glb"),
        ];
        let models: Vec<_> = bytes.iter().map(|b| load(&mut app, b)).collect();
        let assets = app.world().resource::<Assets3d>();
        let emissive = |i: usize| assets.model(models[i]).unwrap().parts.iter().any(|p| assets.material(p.material).emissive.max_element() > 0.5);
        assert!(emissive(3) && !emissive(2), "着火的粮车才有火星");
        let banner = assets.model(models[4]).unwrap();
        assert_eq!(banner.animations.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["wave"]);
        assert!(banner.parts.iter().any(|p| p.skin == Some(0)));
        let flag = app.world_mut().spawn((Model3d(models[4]), Transform3d::default(), Animator::new("wave"))).id();
        // 各节点姿势在几帧之间变了多少（骨骼的旋转、位移都算）
        let mut poses = (0..8).map(|_| {
            app.tick(VIEWPORT, 0.15);
            app.world().get::<Pose>(flag).unwrap().nodes.clone()
        });
        let first = poses.next().unwrap();
        let moved = poses.map(|p| p.iter().zip(&first).map(|(a, b)| (*a - *b).abs().to_cols_array().into_iter().fold(0.0, f32::max)).fold(0.0, f32::max)).fold(0.0, f32::max);
        assert!(moved > 0.05, "{moved}");
    }

    /// effects.ts 里自己写的着色器都编得过。
    #[test]
    fn shaders_compile() {
        let ts = include_str!("../effects.ts");
        let start = ts.find("export const SHADERS").unwrap();
        let end = start + ts[start..].find("\n};").unwrap();
        let mut found = 0;
        for chunk in ts[start..end].split('`').skip(1).step_by(2) {
            let checked = if chunk.contains("fn post(") { nge_render2d::Shaders::check_post(chunk) } else { nge_render2d::Shaders::check_sprite(chunk) };
            if let Err(e) = checked {
                panic!("{chunk}\n编不过：{e}");
            }
            found += 1;
        }
        assert_eq!(found, 3, "sky、blob、blur");
    }

    /// 一个和游戏里一样摆的场景（地面、着火的粮车、旌旗、人和影子、背景层模糊和 bloom）能同步进来、跑几帧不出错。
    #[test]
    fn a_scene_like_the_game_runs() {
        let mut app = build(AudioPlugin::mock());
        let scene = serde_json::json!({
            "camera": { "x": 400, "y": 0 },
            "camera3d": { "match2d": true, "interleave": true },
            "lights": [{ "id": "sun", "kind": "directional", "direction": [-1.5, -0.6, -0.4], "shadows": true },
                       { "id": "fire", "kind": "point", "x": 556, "y": -40, "z": 296, "intensity": 9600, "range": 280 }],
            "models": [{ "id": "g", "model": "asset:/@sanguo/ground.glb", "x": 0, "y": -200, "z": 400 },
                       { "id": "c", "model": "asset:/@sanguo/cart_fire.glb", "x": 560, "y": -96, "z": 296 },
                       { "id": "b", "model": "asset:/@sanguo/banner.glb", "x": 230, "y": -60, "z": 260, "animation": { "clip": "wave" } }],
            "emitters3d": [{ "id": "flame", "x": 556, "y": -60, "z": 296, "rate": 70, "blend": "add" }],
            "sprites": [{ "id": "player", "texture": "asset:/@sanguo/player.png", "grid": [8, 4], "frame": 0, "x": 120, "y": -72, "z": 339, "w": 240, "h": 144 }],
            "post": [{ "shader": "bloom" }],
        });
        app.prop("scene", Some(&scene));
        for _ in 0..10 {
            app.tick(VIEWPORT, 1.0 / 60.0);
        }
        let world = app.world_mut();
        let ids: Vec<String> = world.query::<&nge_scene::SceneId>().iter(world).map(|id| id.0.clone()).collect();
        for id in ["g", "c", "b", "player"] {
            assert!(ids.iter().any(|x| x == id), "{id} {ids:?}");
        }
    }
}
