//! 关掉了 `physics3d` / `ai` / `net` feature 时，场景里还声明了它们：记一次 warn，不再静悄悄地什么都不做。

use bevy_ecs::prelude::*;

use crate::protocol::Scene;

/// 场景里用到了没编进来的功能：每种只 warn 一次。
pub(crate) fn warn_disabled(scene: Res<Scene>, mut warned: Local<[bool; 3]>) {
    for name in unwarned(&scene, &mut warned) {
        log::warn!("场景用到了 {name}，但 nge-scene 编译时没开 feature `{name}`，不生效");
    }
}

/// `scene` 用到的、没编进来的、还没报过的功能，记成报过。
fn unwarned(scene: &Scene, warned: &mut [bool; 3]) -> Vec<&'static str> {
    let uses = [
        cfg!(not(feature = "physics3d")) && (scene.physics3d.is_some() || scene.models.iter().any(|m| m.body.is_some())),
        cfg!(not(feature = "ai")) && scene.sprites.iter().any(|s| s.chase.is_some()),
        cfg!(not(feature = "net")) && scene.net.is_some(),
    ];
    let mut out = Vec::new();
    for (i, (used, name)) in uses.into_iter().zip(["physics3d", "ai", "net"]).enumerate() {
        if used && !warned[i] {
            warned[i] = true;
            out.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::app;

    #[test]
    fn a_disabled_feature_the_scene_uses_is_reported_once() {
        let scene: Scene = serde_json::from_value(json!({ "physics3d": {}, "net": { "room": "r" } })).unwrap();
        let mut warned = [false; 3];
        let first: Vec<&str> = [("physics3d", cfg!(feature = "physics3d")), ("net", cfg!(feature = "net"))].into_iter().filter(|(_, on)| !on).map(|(n, _)| n).collect();
        assert_eq!(unwarned(&scene, &mut warned), first);
        assert_eq!(unwarned(&scene, &mut warned), Vec::<&str>::new(), "第二次不再报");
        assert_eq!(unwarned(&Scene::default(), &mut [false; 3]), Vec::<&str>::new(), "没用到不报");
    }

    #[test]
    fn plugins_of_disabled_features_are_not_added() {
        let app = app();
        assert_eq!(app.is_plugin_added::<nge_physics3d::Physics3dPlugin>(), cfg!(feature = "physics3d"));
        assert_eq!(app.is_plugin_added::<nge_ai::AiPlugin>(), cfg!(feature = "ai"));
        assert_eq!(app.is_plugin_added::<nge_net::NetPlugin>(), cfg!(feature = "net"));
    }
}
