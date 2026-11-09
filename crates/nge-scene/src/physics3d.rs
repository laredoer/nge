//! 模型节点上的 `body`：3D 刚体、碰撞体，3D 碰撞的 `collide` 事件和 `scene.physics3d`（见 docs/physics3d.md）。

use bevy_ecs::prelude::*;
use bevy_ecs::system::EntityCommands;
use glam::Vec3;
use nge_ecs::Outbox;
use nge_physics3d::{Collider, Collision3d, Impulse, Physics3d, RigidBody, Velocity};
use serde::Deserialize;

use crate::nodes::ModelNode;
use crate::physics::{COLLIDE, Collide};
use crate::protocol::Scene;
use crate::sync::{Declared, SceneId};

/// 场景用 point：重力默认往下 981，一米 100。
pub(crate) const GRAVITY: Vec3 = Vec3::new(0.0, -981.0, 0.0);
pub(crate) const LENGTH_UNIT: f32 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Body3dKind {
    #[default]
    Dynamic,
    Fixed,
    Kinematic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape3dName {
    #[default]
    Box,
    Ball,
    Capsule,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ModelBody {
    pub kind: Body3dKind,
    pub shape: Shape3dName,
    /// 盒子的长宽高（世界单位，不乘模型的 scale）。
    pub size: Option<[f32; 3]>,
    /// 球、胶囊的半径。
    pub r: Option<f32>,
    /// 胶囊中间圆柱的高。
    pub h: Option<f32>,
    pub restitution: f32,
    pub friction: f32,
    pub density: f32,
    pub sensor: bool,
    pub velocity: [f32; 3],
    pub collide: bool,
}

impl Default for ModelBody {
    fn default() -> Self {
        Self {
            kind: Body3dKind::Dynamic,
            shape: Shape3dName::Box,
            size: None,
            r: None,
            h: None,
            restitution: 0.0,
            friction: 0.5,
            density: 1.0,
            sensor: false,
            velocity: [0.0; 3],
            collide: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Physics3dNode {
    pub gravity: Option<[f32; 3]>,
}

/// 有 `body` 的模型：声明的位置、角度、缩放没变时不拉回去（位置归物理管）。
pub(crate) fn keeps_place(before: Option<&ModelNode>, node: &ModelNode) -> bool {
    before.is_some_and(|b| b.body.is_some() && node.body.is_some() && (b.x, b.y, b.z, b.rotation, b.scale) == (node.x, node.y, node.z, node.rotation, node.scale))
}

/// 按模型节点的 `body` 挂上或拿掉 3D 物理组件。
pub(crate) fn apply_body(entity: &mut EntityCommands, before: Option<&ModelNode>, node: &ModelNode) {
    let Some(body) = &node.body else {
        entity.remove::<(RigidBody, Collider, Velocity, Impulse)>();
        return;
    };
    let kind = match body.kind {
        Body3dKind::Dynamic => RigidBody::Dynamic,
        Body3dKind::Fixed => RigidBody::Fixed,
        Body3dKind::Kinematic => RigidBody::Kinematic,
    };
    entity.insert((kind, collider(body, node.scale))).insert_if_new(Impulse::default());
    // 初速度只在生成、改了 velocity、或者重新放了一次时设置，不打断已经在动的刚体
    let same = keeps_place(before, node) && before.and_then(|b| b.body.as_ref()).is_some_and(|b| b.velocity == body.velocity);
    if !same {
        entity.insert(Velocity { linear: Vec3::from(body.velocity), angular: Vec3::ZERO });
    }
}

/// 没写大小时按模型的 `scale`（引擎里的模型大多是一个单位大小）：盒子边长 `scale`，球半径 `scale / 2`，
/// 胶囊半径 `scale / 4`、中间高 `scale / 2`（总高 `scale`）。
fn collider(body: &ModelBody, scale: f32) -> Collider {
    let mut c = match body.shape {
        Shape3dName::Box => Collider::cuboid(body.size.map_or(Vec3::splat(scale), Vec3::from)),
        Shape3dName::Ball => Collider::ball(body.r.unwrap_or(scale / 2.0)),
        Shape3dName::Capsule => Collider::capsule(body.h.unwrap_or(scale / 2.0) / 2.0, body.r.unwrap_or(scale / 4.0)),
    };
    c.restitution = body.restitution;
    c.friction = body.friction;
    c.density = body.density;
    c.sensor = body.sensor;
    c.events = body.collide;
    c
}

/// 两个都是模型节点时把 3D 碰撞转成 `collide` 事件。
pub(crate) fn collide(mut reader: MessageReader<Collision3d>, ids: Query<&SceneId, With<Declared<ModelNode>>>, mut outbox: ResMut<Outbox>) {
    for c in reader.read() {
        if let (Ok(a), Ok(b)) = (ids.get(c.a), ids.get(c.b)) {
            outbox.send(COLLIDE, &Collide { a: a.0.clone(), b: b.0.clone(), started: c.started });
        }
    }
}

/// `scene.physics3d.gravity`：变了才覆盖；不写时回到默认。
pub(crate) fn sync_gravity(scene: Res<Scene>, mut physics: ResMut<Physics3d>, mut last: Local<Option<Option<[f32; 3]>>>) {
    let gravity = scene.physics3d.as_ref().and_then(|p| p.gravity);
    if last.as_ref() == Some(&gravity) {
        return;
    }
    physics.set_gravity(gravity.map_or(GRAVITY, Vec3::from));
    *last = Some(gravity);
}

#[cfg(test)]
mod tests {
    use glam::Quat;
    use nge_physics3d::Shape;
    use nge_render3d::Transform3d;
    use serde_json::json;

    use super::*;
    use crate::testing::{app, frame};

    fn get<C: Component + Clone>(app: &mut nge_ecs::App, id: &str) -> Option<C> {
        let world = app.world_mut();
        world.query::<(&SceneId, &C)>().iter(world).find(|(s, _)| s.0 == id).map(|(_, c)| c.clone())
    }

    fn at(app: &mut nge_ecs::App, id: &str) -> Vec3 {
        get::<Transform3d>(app, id).unwrap().translation
    }

    fn model(id: &str, x: f32, y: f32, body: serde_json::Value) -> serde_json::Value {
        json!({ "id": id, "model": "asset:/@g/a.glb", "x": x, "y": y, "scale": 20, "body": body })
    }

    #[test]
    fn model_bodies_fall_and_size_from_scale() {
        let mut app = app();
        let scene = |tap: bool| json!({ "models": [
            model("floor", 0.0, -10.0, json!({ "kind": "fixed", "size": [400, 20, 400] })),
            { "id": "box", "model": "asset:/@g/a.glb", "y": 100, "scale": 20, "tap": tap, "body": {} },
            model("ball", 100.0, 100.0, json!({ "shape": "ball", "restitution": 0.5 })),
            model("pill", 200.0, 100.0, json!({ "shape": "capsule", "kind": "kinematic" })),
        ] });
        frame(&mut app, Some(scene(false)));
        assert_eq!(get::<Collider>(&mut app, "box").unwrap().shape, Shape::Box(Vec3::splat(20.0)), "盒子边长按 scale");
        assert_eq!(get::<Collider>(&mut app, "floor").unwrap().shape, Shape::Box(Vec3::new(400.0, 20.0, 400.0)));
        let ball = get::<Collider>(&mut app, "ball").unwrap();
        assert_eq!((ball.shape, ball.restitution), (Shape::Ball(10.0), 0.5));
        assert_eq!(get::<Collider>(&mut app, "pill").unwrap().shape, Shape::Capsule { half_height: 5.0, radius: 5.0 });
        assert_eq!(get::<RigidBody>(&mut app, "pill"), Some(RigidBody::Kinematic));
        assert_eq!(get::<Transform3d>(&mut app, "box").unwrap().scale, Vec3::splat(20.0), "缩放留给画面");
        for _ in 0..40 {
            frame(&mut app, None);
        }
        let y = at(&mut app, "box").y;
        assert!((y - 10.0).abs() < 1.0, "落到地上 {y}");
        // 改别的属性不拉回去
        frame(&mut app, Some(scene(true)));
        frame(&mut app, None);
        assert!((at(&mut app, "box").y - 10.0).abs() < 1.0, "没被拉回 y = 100");
        // 去掉 body 就删
        frame(&mut app, Some(json!({ "models": [{ "id": "box", "model": "asset:/@g/a.glb", "y": 100 }] })));
        assert!(get::<RigidBody>(&mut app, "box").is_none() && get::<Collider>(&mut app, "box").is_none());
        assert_eq!(at(&mut app, "box").y, 100.0, "回到声明的位置");
    }

    #[test]
    fn moving_the_declared_place_teleports_and_restarts_velocity() {
        let mut app = app();
        let scene = |x: f32, rotation: f32| json!({ "physics3d": { "gravity": [0, 0, 0] }, "models": [
            { "id": "box", "model": "asset:/@g/a.glb", "x": x, "rotation": [0, rotation, 0], "scale": 20, "body": { "velocity": [0, 0, 120] } },
        ] });
        frame(&mut app, Some(scene(0.0, 0.0)));
        for _ in 0..10 {
            frame(&mut app, None);
        }
        assert!(at(&mut app, "box").z > 10.0, "初速度 {}", at(&mut app, "box"));
        frame(&mut app, Some(scene(50.0, 90.0)));
        let t = get::<Transform3d>(&mut app, "box").unwrap();
        assert!((t.translation.x - 50.0).abs() < 1e-3 && t.translation.z.abs() < 10.0, "瞬移回去重新走（这一帧走了 4 步） {}", t.translation);
        assert!(t.rotation.angle_between(Quat::from_rotation_y(90f32.to_radians())) < 0.01);
    }

    #[test]
    fn push_and_collide_work_on_models() {
        let mut app = app();
        let scene = json!({ "physics3d": { "gravity": [0, 0, 0] }, "models": [
            model("wall", 0.0, 0.0, json!({ "kind": "fixed", "size": [100, 100, 10] })).as_object().cloned().map(|mut m| { m.insert("z".into(), json!(40)); m }).unwrap(),
            model("ball", 0.0, 0.0, json!({ "shape": "ball", "collide": true })),
        ] });
        let mut events = frame(&mut app, Some(scene));
        app.command("push", &json!({ "id": "ball", "x": 0, "y": 0, "z": 300 }));
        for _ in 0..20 {
            events.extend(frame(&mut app, None));
        }
        let hits: Vec<Collide> = events.iter().filter(|(n, _)| n == "collide").map(|(_, d)| COLLIDE.decode(d).unwrap()).collect();
        let first = hits.first().expect("撞到墙");
        assert!(first.started && [first.a.as_str(), first.b.as_str()].contains(&"wall") && [first.a.as_str(), first.b.as_str()].contains(&"ball"));
        assert!(at(&mut app, "ball").z < 30.0, "被墙挡住 {}", at(&mut app, "ball"));
    }

    #[test]
    fn turning_or_adding_a_body_places_it_again() {
        let mut app = app();
        let scene = |spin: f32, yaw: f32, body: bool| {
            let mut m = json!({ "id": "m", "model": "asset:/@g/a.glb", "y": 50, "spin": spin, "rotation": [0, yaw, 0], "scale": 10 });
            if body {
                m["body"] = json!({ "kind": "kinematic" });
            }
            json!({ "models": [m] })
        };
        // 先转着，再加上刚体：回到声明的角度
        frame(&mut app, Some(scene(90.0, 0.0, false)));
        frame(&mut app, None);
        assert!(get::<Transform3d>(&mut app, "m").unwrap().rotation.angle_between(Quat::IDENTITY) > 0.1, "转起来了");
        frame(&mut app, Some(scene(0.0, 0.0, true)));
        assert!(get::<Transform3d>(&mut app, "m").unwrap().rotation.angle_between(Quat::IDENTITY) < 1e-3, "加上刚体时放回声明的角度");
        // 只改角度也重新放
        frame(&mut app, Some(scene(0.0, 45.0, true)));
        frame(&mut app, None);
        assert!(get::<Transform3d>(&mut app, "m").unwrap().rotation.angle_between(Quat::from_rotation_y(45f32.to_radians())) < 0.01);
    }

    #[test]
    fn gravity_follows_the_scene() {
        let mut app = app();
        assert_eq!(app.world().resource::<Physics3d>().gravity(), GRAVITY);
        frame(&mut app, Some(json!({ "physics3d": { "gravity": [0, 0, -5] } })));
        assert_eq!(app.world().resource::<Physics3d>().gravity(), Vec3::new(0.0, 0.0, -5.0));
        frame(&mut app, Some(json!({})));
        assert_eq!(app.world().resource::<Physics3d>().gravity(), GRAVITY);
    }

    #[test]
    fn bodies_override_attach_and_spin() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [{ "id": "s", "w": 10 }], "models": [
            { "id": "m", "model": "asset:/@g/a.glb", "attach": "s", "spin": 90, "scale": 10, "body": { "kind": "fixed" } },
        ] })));
        assert!(get::<crate::attach::Attach>(&mut app, "m").is_none());
        assert!(get::<crate::behave::Spin>(&mut app, "m").is_none());
    }
}
