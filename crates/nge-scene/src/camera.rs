//! 2D 相机：`scene.camera` 的位置、跟随、边界，和 `shake` command。

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_core::{CommandKey, wgpu};
use nge_ecs::{Screen, Time};
use nge_render2d::{Camera2d, ClearColor, DrawAt, Sprite, Transform2d};
use serde::Deserialize;

use crate::SceneCamera;
use crate::protocol::{Bounds, CameraNode, Color, Scene};
use crate::sync::SceneId;

pub const SHAKE: CommandKey<Shake> = CommandKey::new("shake");

/// 命令 `shake`：相机抖动 `duration` 秒，幅度 `strength`（屏幕 point）线性衰减。
#[derive(Message, Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shake {
    pub strength: f32,
    pub duration: f32,
}

impl Default for Shake {
    fn default() -> Self {
        Self { strength: 8.0, duration: 0.3 }
    }
}

#[derive(Debug, Clone, Copy)]
struct Shaking {
    strength: f32,
    duration: f32,
    left: f32,
}

impl Shaking {
    fn amplitude(&self) -> f32 {
        self.strength * (self.left / self.duration).clamp(0.0, 1.0)
    }
}

/// 相机节点和上一次写进相机的震屏偏移（相机位置减掉它就是不含震屏的位置）。
#[derive(Resource, Debug, Default)]
pub(crate) struct CameraRig {
    node: CameraNode,
    applied: Vec2,
    /// 上一次写进相机的位置；相机位置和它不一样说明 Rust 系统改过（偏移已经被覆盖掉了）。
    written: Vec2,
    /// 上一帧在跟随（目标存在）。
    tracking: bool,
    shake: Option<Shaking>,
    /// 上一帧的视口大小，变了要重新按边界限制。
    size: Vec2,
}

impl CameraRig {
    /// 相机不含震屏的位置。
    fn unshaken(&self, position: Vec2) -> Vec2 {
        if position == self.written { position - self.applied } else { position }
    }
}

/// 让视野不超出 `bounds`；视野比范围大的那一轴对准范围中心。
fn clamp(p: Vec2, bounds: Option<Bounds>, size: Vec2, zoom: f32) -> Vec2 {
    let Some(b) = bounds else { return p };
    let half = size / (2.0 * zoom.max(1e-4));
    let axis = |v: f32, lo: f32, hi: f32, h: f32| if hi - lo <= 2.0 * h { (lo + hi) / 2.0 } else { v.clamp(lo + h, hi - h) };
    Vec2::new(axis(p.x, b.left, b.right, half.x), axis(p.y, b.bottom, b.top, half.y))
}

/// 按时间的正弦组合，长度不超过 `amplitude`。
fn shake_offset(t: f32, amplitude: f32) -> Vec2 {
    let x = (t * 47.0).sin() + 0.5 * (t * 91.0 + 1.3).sin();
    let y = (t * 53.0 + 0.7).sin() + 0.5 * (t * 79.0).sin();
    (Vec2::new(x, y) / 1.5).clamp_length_max(1.0) * amplitude
}

/// 2D 相机和背景色。和节点一样：声明没变就不碰，Rust 系统可以自己驱动相机。
/// 不跟随时马上放到声明的位置，同一帧的点击就用新相机。
pub(crate) fn sync_camera(
    scene: Res<Scene>,
    screen: Res<Screen>,
    mut rig: ResMut<CameraRig>,
    mut camera: Query<&mut Camera2d, With<SceneCamera>>,
    mut clear: ResMut<ClearColor>,
    mut last: Local<Option<(Option<CameraNode>, Option<Color>)>>,
) {
    let declared = (scene.camera.clone(), scene.background);
    if last.as_ref() == Some(&declared) {
        return;
    }
    let node = declared.0.clone().unwrap_or_default();
    let fixed = node.follow.is_none();
    // 换了跟随目标也算重新开始跟随，直接对准
    if fixed || node.follow != rig.node.follow {
        rig.tracking = false;
    }
    for mut camera in &mut camera {
        let position = if fixed { clamp(Vec2::new(node.x, node.y), node.bounds, screen.size, node.zoom) } else { rig.unshaken(camera.position) };
        *camera = Camera2d { position, zoom: node.zoom, ..*camera };
        rig.written = position;
    }
    rig.applied = Vec2::ZERO;
    rig.node = node;
    rig.size = screen.size;
    *clear = declared.1.map_or_else(ClearColor::default, |c| {
        let c = c.0.as_dvec4();
        ClearColor(wgpu::Color { r: c.x, g: c.y, b: c.z, a: c.w })
    });
    *last = Some(declared);
}

/// 物理和补间之后：跟随目标、按边界限制、加上震屏。
/// 不跟随、不抖、视口没变的帧不写相机；写的时候以相机当前位置为准，Rust 系统驱动的相机不会被拉回声明的位置。
pub(crate) fn camera_rig(
    time: Res<Time>,
    screen: Res<Screen>,
    mut rig: ResMut<CameraRig>,
    mut shakes: MessageReader<Shake>,
    sprites: Query<(&SceneId, &Transform2d, Option<&DrawAt>), With<Sprite>>,
    mut camera: Query<&mut Camera2d, With<SceneCamera>>,
) {
    let rig = &mut *rig;
    for s in shakes.read() {
        let valid = |v: f32| v.is_finite() && v > 0.0;
        if !(valid(s.strength) && valid(s.duration)) {
            log::warn!("shake：strength 和 duration 要是有限的正数，收到 {s:?}");
            continue;
        }
        let new = Shaking { strength: s.strength, duration: s.duration, left: s.duration };
        if rig.shake.is_none_or(|old| new.amplitude() >= old.amplitude()) {
            rig.shake = Some(new);
        }
    }
    let Ok(mut cam) = camera.single_mut() else { return };
    let node = &rig.node;
    let resized = rig.size != screen.size;
    rig.size = screen.size;
    let current = rig.unshaken(cam.position);
    // 有刚体的精灵跟着画的位置（插值过的），不然相机和画面差一步，跟随时一抖一抖
    let target = node
        .follow
        .as_deref()
        .and_then(|id| sprites.iter().find(|(s, ..)| s.0 == id))
        .map(|(_, t, draw)| draw.map_or(t.translation, |d| d.translation) + Vec2::new(node.x, node.y));
    // 跟随时（目标在不在都）由场景管相机；不跟随时只在视口变了要重新限制边界时动
    let mut moved = node.follow.is_some() || (resized && node.bounds.is_some()) || rig.applied != Vec2::ZERO;
    let base = match target {
        Some(target) => {
            let want = clamp(target, node.bounds, screen.size, cam.zoom);
            let k = if rig.tracking && node.smooth > 0.0 { 1.0 - (-time.delta / node.smooth).exp() } else { 1.0 };
            rig.tracking = true;
            current.lerp(want, k)
        }
        None => {
            rig.tracking = false;
            clamp(current, node.bounds, screen.size, cam.zoom)
        }
    };
    let mut offset = Vec2::ZERO;
    if let Some(s) = &mut rig.shake {
        // 先按剩余时间抖，再扣时间：比一帧还短的也能抖一帧
        moved = true;
        offset = shake_offset((time.elapsed % 1000.0) as f32, s.amplitude() / cam.zoom.max(1e-4));
        s.left -= time.delta;
        if s.left <= 0.0 {
            rig.shake = None;
        }
    }
    if moved {
        cam.position = base + offset;
        rig.applied = offset;
        rig.written = cam.position;
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;
    use nge_core::Viewport;
    use nge_ecs::App;
    use nge_render2d::{Camera2d, ClearColor};
    use serde_json::json;

    use crate::testing::{app, frame};

    fn camera(app: &mut App) -> Camera2d {
        let world = app.world_mut();
        *world.query::<&Camera2d>().single(world).unwrap()
    }

    fn tick(app: &mut App, dt: f32) {
        app.tick(crate::testing::VIEWPORT, dt);
    }

    fn near(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn camera_and_background() {
        let mut app = app();
        frame(&mut app, Some(json!({ "background": "#ffffff", "camera": { "x": 3, "zoom": 2 }, "sprites": [] })));
        {
            let world = app.world_mut();
            let cams: Vec<Camera2d> = world.query::<&Camera2d>().iter(world).copied().collect();
            assert_eq!(cams.len(), 1);
            assert_eq!((cams[0].position, cams[0].zoom), (Vec2::new(3.0, 0.0), 2.0));
            assert_eq!(world.resource::<ClearColor>().0.r, 1.0);
        }
        frame(&mut app, Some(json!({ "sprites": [] })));
        let world = app.world_mut();
        assert_eq!(world.query::<&Camera2d>().single(world).unwrap().zoom, 1.0);
        assert_eq!(world.resource::<ClearColor>().0, ClearColor::default().0);
    }

    #[test]
    fn camera_untouched_when_only_sprites_change() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a" }] })));
        {
            let world = app.world_mut();
            world.query::<&mut Camera2d>().single_mut(world).unwrap().position.x = 77.0;
            world.resource_mut::<ClearColor>().0.r = 0.5;
        }
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "y": 1 }] })));
        let world = app.world_mut();
        assert_eq!(world.query::<&Camera2d>().single(world).unwrap().position.x, 77.0, "Rust 驱动的相机不被重置");
        assert_eq!(world.resource::<ClearColor>().0.r, 0.5);
    }

    #[test]
    fn follow_with_offset_tracks_the_sprite() {
        let mut app = app();
        let scene = |x: f32| json!({ "camera": { "follow": "p", "y": 10 }, "sprites": [{ "id": "p", "x": x, "y": 40 }] });
        frame(&mut app, Some(scene(30.0)));
        assert_eq!(camera(&mut app).position, Vec2::new(30.0, 50.0));
        frame(&mut app, Some(scene(60.0)));
        assert_eq!(camera(&mut app).position, Vec2::new(60.0, 50.0));
        let world = app.world_mut();
        world.query::<&mut nge_render2d::Transform2d>().iter_mut(world).for_each(|mut t| t.translation.x = -5.0);
        tick(&mut app, 0.1);
        assert_eq!(camera(&mut app).position.x, -5.0, "Rust 系统改的位置也跟");
    }

    #[test]
    fn follow_uses_where_physics_draws_the_sprite() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera": { "follow": "p" }, "sprites": [{ "id": "p", "x": 30 }] })));
        let world = app.world_mut();
        let p = world.query_filtered::<bevy_ecs::entity::Entity, bevy_ecs::query::With<nge_render2d::Sprite>>().iter(world).next().unwrap();
        world.entity_mut(p).insert(nge_render2d::DrawAt { translation: Vec2::new(25.0, 3.0), rotation: 0.0 });
        tick(&mut app, 0.1);
        assert_eq!(camera(&mut app).position, Vec2::new(25.0, 3.0), "跟着画的位置，不然插值的刚体在相机里一抖一抖");
    }

    #[test]
    fn smooth_follow_eases_and_snaps_on_start() {
        let mut app = app();
        let scene = |x: f32| json!({ "camera": { "follow": "p", "smooth": 1 }, "sprites": [{ "id": "p", "x": x }] });
        frame(&mut app, Some(scene(40.0)));
        assert_eq!(camera(&mut app).position.x, 40.0, "刚开始跟随直接对准");
        app.prop("scene", Some(&scene(140.0)));
        tick(&mut app, 1.0);
        let x = camera(&mut app).position.x;
        assert!((x - (40.0 + 100.0 * (1.0 - (-1.0f32).exp()))).abs() < 1e-3, "{x}");
    }

    #[test]
    fn missing_target_holds_then_snaps() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera": { "x": 5 } })));
        frame(&mut app, Some(json!({ "camera": { "x": 5, "follow": "ghost", "smooth": 1 } })));
        assert_eq!(camera(&mut app).position.x, 5.0, "目标不存在时停在原地");
        frame(&mut app, Some(json!({ "camera": { "x": 5, "follow": "ghost", "smooth": 1 }, "sprites": [{ "id": "ghost", "x": 50 }] })));
        assert_eq!(camera(&mut app).position.x, 55.0, "目标出现后直接对准");
    }

    #[test]
    fn bounds_keep_the_view_inside() {
        let mut app = app();
        let bounds = json!({ "left": -100, "right": 100, "bottom": -20, "top": 20 });
        frame(&mut app, Some(json!({ "camera": { "x": -1000, "y": 7, "bounds": bounds } })));
        assert_eq!(camera(&mut app).position, Vec2::new(-50.0, 0.0), "视口 100 宽；y 方向范围比视野小就对准中心");
        frame(&mut app, Some(json!({ "camera": { "x": -1000, "zoom": 2, "bounds": bounds } })));
        assert_eq!(camera(&mut app).position.x, -75.0);
        frame(&mut app, Some(json!({ "camera": { "follow": "p", "bounds": bounds }, "sprites": [{ "id": "p", "x": 999 }] })));
        assert_eq!(camera(&mut app).position.x, 50.0, "跟随时也限制");
        app.tick(Viewport { size: [200.0, 100.0], pixels: [200, 100] }, 0.1);
        assert_eq!(camera(&mut app).position.x, 0.0, "视口变宽后重新限制");
    }

    #[test]
    fn shake_decays_back_and_stronger_wins() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera": { "x": 3, "zoom": 2 } })));
        app.command("shake", &json!({ "strength": 8, "duration": 1 }));
        tick(&mut app, 0.25);
        let d = camera(&mut app).position - Vec2::new(3.0, 0.0);
        assert!(d.length() > 0.0 && d.length() <= 8.0 / 2.0 + 1e-4, "幅度按 point 算、随 zoom 缩小 {d}");
        tick(&mut app, 0.25);
        app.command("shake", &json!({ "strength": 2, "duration": 1 }));
        app.command("shake", &json!({ "duration": 0 }));
        tick(&mut app, 0.6);
        tick(&mut app, 0.01);
        assert!(near(camera(&mut app).position, Vec2::new(3.0, 0.0)), "弱的没有接管，到时间回到原位");
        app.command("shake", &json!({}));
        tick(&mut app, 0.1);
        assert!(!near(camera(&mut app).position, Vec2::new(3.0, 0.0)), "默认参数也抖");
    }

    #[test]
    fn stronger_shake_takes_over() {
        let mut app = app();
        frame(&mut app, Some(json!({})));
        app.command("shake", &json!({ "strength": 2, "duration": 0.2 }));
        tick(&mut app, 0.1);
        app.command("shake", &json!({ "strength": 8, "duration": 1 }));
        tick(&mut app, 0.1);
        tick(&mut app, 0.3);
        assert!(camera(&mut app).position.length() > 0.0, "强的接管后 0.5 秒还在抖");
        app.command("shake", &json!({ "strength": 1, "duration": 0.001 }));
        tick(&mut app, 0.6);
        tick(&mut app, 0.01);
        assert!(near(camera(&mut app).position, Vec2::ZERO));
        app.command("shake", &json!({ "strength": 50, "duration": 0.001 }));
        tick(&mut app, 1.0 / 60.0);
        assert!(camera(&mut app).position.length() > 0.0, "比一帧还短的也抖一帧");
        tick(&mut app, 1.0 / 60.0);
        assert!(near(camera(&mut app).position, Vec2::ZERO));
    }

    #[test]
    fn rust_driven_camera_keeps_its_place_through_shake_and_resize() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera": { "x": 1, "bounds": { "left": -500, "right": 500, "bottom": -500, "top": 500 } } })));
        let world = app.world_mut();
        world.query::<&mut Camera2d>().single_mut(world).unwrap().position.x = 77.0;
        app.command("shake", &json!({ "duration": 0.1 }));
        tick(&mut app, 0.2);
        tick(&mut app, 0.01);
        assert!(near(camera(&mut app).position, Vec2::new(77.0, 0.0)), "抖完回到 Rust 设的位置");
        app.tick(Viewport { size: [200.0, 100.0], pixels: [200, 100] }, 0.1);
        assert_eq!(camera(&mut app).position.x, 77.0, "视口变了只按边界限制，不回到声明的位置");
    }

    #[test]
    fn rust_setting_absolute_position_during_shake_is_not_shifted() {
        let mut app = app();
        frame(&mut app, Some(json!({})));
        app.command("shake", &json!({ "duration": 0.1 }));
        tick(&mut app, 0.05);
        let world = app.world_mut();
        world.query::<&mut Camera2d>().single_mut(world).unwrap().position = Vec2::new(40.0, 0.0);
        tick(&mut app, 0.2);
        let world = app.world_mut();
        world.query::<&mut Camera2d>().single_mut(world).unwrap().position = Vec2::new(40.0, 0.0);
        tick(&mut app, 0.01);
        assert!(near(camera(&mut app).position, Vec2::new(40.0, 0.0)), "Rust 每帧重设位置时不再多减一次偏移");
    }

    #[test]
    fn follow_bounds_apply_before_target_appears_and_switching_snaps() {
        let mut app = app();
        let bounds = json!({ "left": 0, "right": 1000, "bottom": 0, "top": 1000 });
        frame(&mut app, Some(json!({ "camera": { "follow": "p", "smooth": 1, "bounds": bounds } })));
        assert_eq!(camera(&mut app).position, Vec2::new(50.0, 50.0), "目标还没出现也不拍出边界");
        let sprites = json!([{ "id": "a", "x": 200, "y": 200 }, { "id": "b", "x": 600, "y": 600 }]);
        frame(&mut app, Some(json!({ "camera": { "follow": "a", "smooth": 1, "bounds": bounds }, "sprites": sprites })));
        assert_eq!(camera(&mut app).position, Vec2::new(200.0, 200.0));
        frame(&mut app, Some(json!({ "camera": { "follow": "b", "smooth": 1, "bounds": bounds }, "sprites": sprites })));
        assert_eq!(camera(&mut app).position, Vec2::new(600.0, 600.0), "换目标直接对准");
        frame(&mut app, Some(json!({ "camera": { "follow": "b", "smooth": 1, "bounds": bounds }, "sprites": [] })));
        assert_eq!(camera(&mut app).position, Vec2::new(600.0, 600.0), "目标没了停在原地");
    }

    #[test]
    fn rust_driven_camera_left_alone_without_follow_or_shake() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera": { "x": 1 }, "sprites": [] })));
        let world = app.world_mut();
        world.query::<&mut Camera2d>().single_mut(world).unwrap().position.x = 77.0;
        tick(&mut app, 0.1);
        tick(&mut app, 0.1);
        assert_eq!(camera(&mut app).position.x, 77.0);
    }
}
