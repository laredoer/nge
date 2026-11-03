//! M3 验收：场景完全由 `App.tsx` 声明，Rust 侧只注册纹理。

use nge_ecs::App;
use nge_render2d::{Grid, Render2dPlugin};
use nge_scene::ScenePlugin;

pub fn factory() -> nge_core::GameFactory {
    nge_ecs::factory(|| {
        let mut app = App::new();
        app.add_plugins(Render2dPlugin).add_plugins(ScenePlugin::new().atlas(
            "ships",
            include_bytes!("../assets/ships.png"),
            Grid { cols: 4, rows: 4 },
        ));
        app
    })
}
