//! 节点上的内置行为：自转、点击（精灵和 3D 模型）、名字牌跟随模型，以及一次性命令 `burst` / `floatText`。

use bevy_ecs::prelude::*;
use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use nge_ecs::{Input, Outbox, Screen, Time};
use nge_particles::ParticleEmitter;
use nge_render2d::{Assets, Atlases, Image, Sprite, Transform2d};
use nge_render3d::{Assets3d, Camera3d, Model3d, Transform3d};
use nge_text::{FloatingText, Text2d};

use crate::nodes::{Burst, FloatText, ModelNode};
use crate::protocol::{TAP, Tap};
use crate::resolve::SceneAssets;
use crate::sync::{Declared, SceneId};
use crate::sync_nodes::emitter;
use crate::SceneCamera;

/// 每秒转多少弧度（精灵绕 z 轴，模型绕 y 轴）。
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Spin(pub f32);

/// 按下时发 `tap` 事件。
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tappable;

/// 屏幕文字跟着模型（按 id）投影到屏幕上的位置走。
#[derive(Component, Debug, Clone, PartialEq)]
pub(crate) struct Follow {
    pub model: String,
    /// 往上抬的 3D 距离。
    pub lift: f32,
    /// 屏幕上的偏移（y 朝上）。
    pub offset: Vec2,
}

/// 看不见的文字挪到这里。
const OFFSCREEN: Vec2 = Vec2::splat(-1e6);

pub(crate) fn spin(time: Res<Time>, mut sprites: Query<(&Spin, &mut Transform2d)>, mut models: Query<(&Spin, &mut Transform3d)>) {
    for (spin, mut t) in &mut sprites {
        t.rotation = (t.rotation + spin.0 * time.delta) % std::f32::consts::TAU;
    }
    for (spin, mut t) in &mut models {
        t.rotation = (Quat::from_rotation_y(spin.0 * time.delta) * t.rotation).normalize();
    }
}

/// 场景相机的投影矩阵。
fn view_proj(camera: &Query<(&Camera3d, &Transform3d), With<SceneCamera>>, screen: &Screen) -> Option<Mat4> {
    let (camera, transform) = camera.iter().next()?;
    (screen.size.y > 0.0).then(|| camera.view_proj(transform, screen.size.x / screen.size.y))
}

pub(crate) fn tap(
    input: Res<Input>,
    screen: Res<Screen>,
    images: Res<Assets<Image>>,
    atlases: Res<Atlases>,
    assets3d: Res<Assets3d>,
    sprites: Query<(&SceneId, &Transform2d, &Sprite), With<Tappable>>,
    models: Query<(&SceneId, &Transform3d, &Model3d), With<Tappable>>,
    camera: Query<(&Camera3d, &Transform3d), With<SceneCamera>>,
    mut outbox: ResMut<Outbox>,
) {
    for p in input.pointers_pressed() {
        // 精灵画在 3D 上面，先看精灵
        let sprite = sprites
            .iter()
            .filter(|(_, t, s)| contains(t, atlases.sprite_size(&images, s), p.world))
            .max_by(|a, b| a.1.z.total_cmp(&b.1.z));
        if let Some((id, ..)) = sprite {
            outbox.send(TAP, &Tap { id: id.0.clone(), x: p.world.x, y: p.world.y, point: None });
            continue;
        }
        let Some(ray) = view_proj(&camera, &screen).and_then(|vp| Ray::through(vp, screen.size, p.screen)) else { continue };
        let hit = models
            .iter()
            .filter_map(|(id, t, m)| Some((id, ray.hit(t.matrix(), model_bounds(&assets3d, m)?)?)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((id, t)) = hit {
            let point = ray.at(t).to_array();
            outbox.send(TAP, &Tap { id: id.0.clone(), x: p.world.x, y: p.world.y, point: Some(point) });
        }
    }
}

/// 世界坐标 `p` 是否落在精灵（考虑旋转和缩放）里面。
fn contains(t: &Transform2d, size: Vec2, p: Vec2) -> bool {
    if t.scale.x == 0.0 || t.scale.y == 0.0 {
        return false;
    }
    let local = Vec2::from_angle(-t.rotation).rotate(p - t.translation) / t.scale;
    local.abs().cmple(size / 2.0).all()
}

/// 模型本地空间里所有网格的包围盒。
fn model_bounds(assets: &Assets3d, model: &Model3d) -> Option<(Vec3, Vec3)> {
    let model = assets.model(model.0)?;
    let points = model.parts.iter().flat_map(|part| {
        let mesh = assets.mesh(part.mesh);
        mesh.into_iter().flat_map(move |m| m.vertices.iter().map(move |v| part.local.transform_point3(Vec3::from(v.position))))
    });
    points.fold(None, |b, p| Some(b.map_or((p, p), |(lo, hi): (Vec3, Vec3)| (lo.min(p), hi.max(p)))))
}

/// 从近平面到远平面的一段射线（世界坐标）。
#[derive(Debug, Clone, Copy)]
struct Ray {
    near: Vec3,
    far: Vec3,
}

impl Ray {
    /// 经过屏幕点 `screen`（左上角原点，y 朝下）的射线。
    fn through(view_proj: Mat4, size: Vec2, screen: Vec2) -> Option<Self> {
        let ndc = Vec2::new(screen.x / size.x * 2.0 - 1.0, 1.0 - screen.y / size.y * 2.0);
        let inv = view_proj.inverse();
        let (near, far) = (inv.project_point3(ndc.extend(0.0)), inv.project_point3(ndc.extend(1.0)));
        (near.is_finite() && far.is_finite()).then_some(Self { near, far })
    }

    fn at(&self, t: f32) -> Vec3 {
        self.near + (self.far - self.near) * t
    }

    /// 和 `matrix` 变换后的包围盒相交时，返回最近的参数 t（0 是近平面，1 是远平面）。
    fn hit(&self, matrix: Mat4, (lo, hi): (Vec3, Vec3)) -> Option<f32> {
        let inv = matrix.inverse();
        if !inv.is_finite() {
            return None;
        }
        // 仿射变换下线段参数不变，在模型本地空间里做 slab 相交
        let (a, b) = (inv.transform_point3(self.near), inv.transform_point3(self.far));
        let d = b - a;
        let (mut t0, mut t1) = (0.0_f32, 1.0_f32);
        for axis in 0..3 {
            if d[axis].abs() < 1e-12 {
                if a[axis] < lo[axis] || a[axis] > hi[axis] {
                    return None;
                }
                continue;
            }
            let (mut near, mut far) = ((lo[axis] - a[axis]) / d[axis], (hi[axis] - a[axis]) / d[axis]);
            if near > far {
                std::mem::swap(&mut near, &mut far);
            }
            t0 = t0.max(near);
            t1 = t1.min(far);
            if t0 > t1 {
                return None;
            }
        }
        Some(t0)
    }
}

/// 名字牌：把模型位置投影到屏幕。模型不存在或在相机后面时挪出屏幕。
pub(crate) fn follow(
    screen: Res<Screen>,
    camera: Query<(&Camera3d, &Transform3d), With<SceneCamera>>,
    models: Query<(&Declared<ModelNode>, &Transform3d)>,
    mut texts: Query<(&Follow, &mut Transform2d)>,
) {
    let vp = view_proj(&camera, &screen);
    for (follow, mut t) in &mut texts {
        let target = models.iter().find(|(d, _)| d.0.id == follow.model).map(|(_, m)| m.translation + Vec3::Y * follow.lift);
        let at = vp.zip(target).and_then(|(vp, p)| {
            let clip = vp * p.extend(1.0);
            (clip.w > 0.0).then(|| {
                let ndc = clip.truncate() / clip.w;
                Vec2::new((ndc.x + 1.0) / 2.0 * screen.size.x, (1.0 - ndc.y) / 2.0 * screen.size.y)
            })
        });
        t.translation = at.map_or(OFFSCREEN, |at| at + Vec2::new(follow.offset.x, -follow.offset.y));
    }
}

pub(crate) fn burst(mut commands: Commands, mut reader: MessageReader<Burst>, mut assets: SceneAssets) {
    for b in reader.read() {
        let emitter = ParticleEmitter { burst: b.count, despawn_when_done: true, ..emitter(&b.particles, &mut assets, "burst") };
        commands.spawn((emitter, Transform2d::from_xy(b.particles.x, b.particles.y).with_z(b.particles.z)));
    }
}

pub(crate) fn float_text(mut commands: Commands, mut reader: MessageReader<FloatText>, mut assets: SceneAssets) {
    for f in reader.read() {
        let text = Text2d { font: assets.font(f.font.as_deref()), size: f.size, color: f.color.map_or(Vec4::ONE, |c| c.0), ..Text2d::new(f.text.clone()) };
        commands.spawn((FloatingText::new(f.duration.max(0.0), f.rise), text, Transform2d::from_xy(f.x, f.y)));
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use serde_json::json;

    use super::*;
    use crate::testing::{app, frame};

    #[test]
    fn hit_test_respects_rotation_and_scale() {
        let t = Transform2d { translation: Vec2::new(10.0, 0.0), rotation: FRAC_PI_2, scale: Vec2::new(2.0, 1.0), z: 0.0 };
        // 4×2 的精灵横向放大 2 倍再转 90°：世界里是竖着的 2 宽 8 高
        let size = Vec2::new(4.0, 2.0);
        assert!(contains(&t, size, Vec2::new(10.0, 3.9)));
        assert!(!contains(&t, size, Vec2::new(10.0, 4.1)));
        assert!(!contains(&t, size, Vec2::new(11.1, 0.0)));
        assert!(!contains(&Transform2d { scale: Vec2::ZERO, ..t }, size, t.translation));
    }

    #[test]
    fn spin_advances_rotation_in_degrees_per_second() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "spin": 90 }] })));
        frame(&mut app, None);
        let world = app.world_mut();
        let t = world.query::<&Transform2d>().iter(world).find(|t| t.rotation != 0.0).copied().unwrap();
        // 每帧 0.5s，两帧共 90°
        assert!((t.rotation - FRAC_PI_2).abs() < 1e-5, "{}", t.rotation);
    }

    #[test]
    fn tap_hits_topmost_tappable_sprite() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [
            { "id": "low", "w": 20, "tap": true },
            { "id": "high", "w": 20, "z": 1, "tap": true },
            { "id": "top_not_tappable", "w": 20, "z": 2 },
        ] })));
        // 视口 100×100：(50, 50) 是世界原点
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 50, "y": 50 }));
        let events = frame(&mut app, None);
        let taps: Vec<Tap> = events.iter().filter(|(n, _)| n == "tap").map(|(_, d)| TAP.decode(d).unwrap()).collect();
        assert_eq!(taps, [Tap { id: "high".into(), x: 0.0, y: 0.0, point: None }]);

        app.command("touch", &json!({ "phase": "end", "id": 1, "x": 50, "y": 50 }));
        app.command("touch", &json!({ "phase": "start", "id": 2, "x": 0, "y": 0 }));
        assert!(frame(&mut app, None).iter().all(|(n, _)| n != "tap"), "点在空白处不发");
    }

    fn taps(events: &[(String, serde_json::Value)]) -> Vec<Tap> {
        events.iter().filter(|(n, _)| n == "tap").map(|(_, d)| TAP.decode(d).unwrap()).collect()
    }

    #[test]
    fn tap_maps_screen_y_down_to_world_y_up() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [
            { "id": "up", "w": 10, "y": 30, "tap": true },
            { "id": "down", "w": 10, "y": -30, "tap": true },
        ] })));
        // 屏幕 y=20 在中心上方 30 point
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 50, "y": 20 }));
        assert_eq!(taps(&frame(&mut app, None)), [Tap { id: "up".into(), x: 0.0, y: 30.0, point: None }]);
    }

    #[test]
    fn tap_uses_camera_changed_in_same_frame() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "w": 10, "x": 110, "tap": true }] })));
        // 相机和点击同一帧到达：zoom 2 时屏幕右移 20 point = 世界右移 10
        app.prop("scene", Some(&json!({ "camera": { "x": 100, "zoom": 2 }, "sprites": [{ "id": "a", "w": 10, "x": 110, "tap": true }] })));
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 70, "y": 50 }));
        assert_eq!(taps(&frame(&mut app, None)), [Tap { id: "a".into(), x: 110.0, y: 0.0, point: None }]);
    }

    /// 场景里放一个已经展开的立方体模型（边长 2）。
    fn with_cube(app: &mut nge_ecs::App, node: serde_json::Value) -> Entity {
        frame(app, Some(json!({ "camera3d": { "x": 0, "y": 0, "z": 10 }, "models": [node] })));
        let world = app.world_mut();
        let entity = world.query_filtered::<Entity, With<Declared<ModelNode>>>().single(world).unwrap();
        let model = crate::testing::cube_model(world);
        world.entity_mut(entity).insert(Model3d(model));
        entity
    }

    #[test]
    fn tap_picks_models_by_bounding_box() {
        let mut app = app();
        with_cube(&mut app, json!({ "id": "box", "model": "asset:/@t/a.glb", "tap": true }));
        // 视口 100×100，正中间朝 -Z 打过去，打在立方体前表面 z = 1
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 50, "y": 50 }));
        let hit = taps(&frame(&mut app, None));
        assert_eq!(hit.len(), 1);
        assert_eq!((hit[0].id.as_str(), hit[0].x, hit[0].y), ("box", 0.0, 0.0));
        let p = Vec3::from(hit[0].point.unwrap());
        assert!((p - Vec3::new(0.0, 0.0, 1.0)).length() < 1e-3, "{p}");

        app.command("touch", &json!({ "phase": "end", "id": 1, "x": 50, "y": 50 }));
        app.command("touch", &json!({ "phase": "start", "id": 2, "x": 2, "y": 2 }));
        assert!(taps(&frame(&mut app, None)).is_empty(), "点到旁边不算");
    }

    #[test]
    fn tap_picks_the_nearest_of_overlapping_models() {
        let mut app = app();
        frame(&mut app, Some(json!({ "camera3d": { "x": 0, "y": 0, "z": 10 }, "models": [
            { "id": "far", "model": "asset:/@t/a.glb", "z": -4, "tap": true },
            { "id": "near", "model": "asset:/@t/a.glb", "tap": true },
        ] })));
        let world = app.world_mut();
        let entities: Vec<Entity> = world.query_filtered::<Entity, With<Declared<ModelNode>>>().iter(world).collect();
        let model = crate::testing::cube_model(world);
        for e in entities {
            world.entity_mut(e).insert(Model3d(model));
        }
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 50, "y": 50 }));
        let hit = taps(&frame(&mut app, None));
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].id, "near");
    }

    #[test]
    fn sprites_win_over_models_and_untappable_models_are_ignored() {
        let mut app = app();
        let entity = with_cube(&mut app, json!({ "id": "box", "model": "asset:/@t/a.glb" }));
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 50, "y": 50 }));
        assert!(taps(&frame(&mut app, None)).is_empty());
        app.world_mut().entity_mut(entity).insert(Tappable);
        app.world_mut().spawn((SceneId("btn".into()), Sprite::color(Vec2::splat(20.0), Vec4::ONE), Transform2d::default(), Tappable));
        app.command("touch", &json!({ "phase": "end", "id": 1, "x": 50, "y": 50 }));
        app.command("touch", &json!({ "phase": "start", "id": 2, "x": 50, "y": 50 }));
        assert_eq!(taps(&frame(&mut app, None))[0].id, "btn", "精灵画在上面，先判断精灵");
    }

    #[test]
    fn ray_hits_nearest_face_and_misses_behind() {
        let ray = Ray { near: Vec3::new(0.0, 0.0, 10.0), far: Vec3::new(0.0, 0.0, -10.0) };
        let cube = (Vec3::splat(-1.0), Vec3::ONE);
        assert!((ray.hit(Mat4::IDENTITY, cube).unwrap() - 0.45).abs() < 1e-6);
        let moved = Mat4::from_translation(Vec3::new(0.0, 0.0, 5.0)) * Mat4::from_scale(Vec3::splat(2.0));
        assert!((ray.at(ray.hit(moved, cube).unwrap()).z - 7.0).abs() < 1e-5, "按模型的变换");
        assert!(ray.hit(Mat4::from_translation(Vec3::new(0.0, 0.0, 20.0)), cube).is_none(), "在近平面后面");
        assert!(ray.hit(Mat4::from_translation(Vec3::new(3.0, 0.0, 0.0)), cube).is_none());
        assert!(ray.hit(Mat4::ZERO, cube).is_none(), "缩放为 0 不可逆");
    }

    #[test]
    fn follow_projects_the_model_and_hides_when_behind() {
        let mut app = app();
        frame(&mut app, Some(json!({
            "camera3d": { "x": 0, "y": 0, "z": 10 },
            "models": [{ "id": "box", "model": "asset:/@t/a.glb" }, { "id": "behind", "model": "asset:/@t/a.glb", "z": 20 }],
            "texts": [
                { "id": "name", "text": "box", "follow": "box", "x": 5, "y": 3 },
                { "id": "lifted", "text": "box", "follow": "box", "lift": 2 },
                { "id": "back", "text": "?", "follow": "behind" },
                { "id": "gone", "text": "?", "follow": "nobody" },
            ],
        })));
        frame(&mut app, None);
        let world = app.world_mut();
        let at: std::collections::HashMap<String, Vec2> =
            world.query_filtered::<(&SceneId, &Transform2d), With<Follow>>().iter(world).map(|(id, t)| (id.0.clone(), t.translation)).collect();
        assert!((at["name"] - Vec2::new(55.0, 47.0)).length() < 1e-3, "屏幕中心加偏移（y 朝上）{}", at["name"]);
        assert!(at["lifted"].y < 50.0 && (at["lifted"].x - 50.0).abs() < 1e-3, "往上抬后在屏幕上方 {}", at["lifted"]);
        assert_eq!((at["back"], at["gone"]), (OFFSCREEN, OFFSCREEN));
    }

    #[test]
    fn burst_and_float_text_commands() {
        let mut app = app();
        frame(&mut app, Some(json!({})));
        app.command("burst", &json!({ "x": 10, "y": 20, "count": 30, "lifetime": [0.4, 0.4], "texture": "plain" }));
        app.command("floatText", &json!({ "x": 1, "y": 2, "text": "+10", "duration": 0.8 }));
        frame(&mut app, None);
        let world = app.world_mut();
        let (emitter, t) = world.query::<(&ParticleEmitter, &Transform2d)>().single(world).map(|(e, t)| (e.clone(), *t)).unwrap();
        assert_eq!((emitter.burst, emitter.despawn_when_done, t.translation), (30, true, Vec2::new(10.0, 20.0)));
        assert!(!matches!(emitter.texture, nge_render2d::SpriteImage::Texture(t) if t == nge_render2d::TextureId::WHITE), "按纹理名解析");
        let text = world.query_filtered::<&Text2d, With<FloatingText>>().single(world).unwrap();
        assert_eq!(text.text, "+10");
        frame(&mut app, None);
        frame(&mut app, None);
        let world = app.world_mut();
        assert_eq!(world.query::<&ParticleEmitter>().iter(world).count(), 0, "粒子死完自动删除");
        assert_eq!(world.query::<&FloatingText>().iter(world).count(), 0);
    }

    #[test]
    fn tap_flag_can_be_removed() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "w": 20, "tap": true, "spin": 10 }] })));
        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "w": 20 }] })));
        let world = app.world_mut();
        assert_eq!(world.query::<&Tappable>().iter(world).count(), 0);
        assert_eq!(world.query::<&Spin>().iter(world).count(), 0);
    }
}
