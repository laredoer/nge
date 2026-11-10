//! 关掉了 `physics3d` / `ai` feature 时，场景里还声明了它们：记一次 warn，不再静悄悄地什么都不做。

use bevy_ecs::prelude::*;

use crate::protocol::Scene;

/// 场景里用到了没编进来的功能：每种只 warn 一次。
pub(crate) fn warn_disabled(scene: Res<Scene>, mut warned: Local<[bool; 2]>) {
    let uses = [
        cfg!(not(feature = "physics3d")) && (scene.physics3d.is_some() || scene.models.iter().any(|m| m.body.is_some())),
        cfg!(not(feature = "ai")) && scene.sprites.iter().any(|s| s.chase.is_some()),
    ];
    for (i, (used, name)) in uses.into_iter().zip(["physics3d", "ai"]).enumerate() {
        if used && !warned[i] {
            warned[i] = true;
            log::warn!("场景用到了 {name}，但 nge-scene 编译时没开 feature `{name}`，不生效");
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::app;

    #[test]
    fn plugins_of_disabled_features_are_not_added() {
        let app = app();
        assert_eq!(app.is_plugin_added::<nge_physics3d::Physics3dPlugin>(), cfg!(feature = "physics3d"));
        assert_eq!(app.is_plugin_added::<nge_ai::AiPlugin>(), cfg!(feature = "ai"));
    }
}
