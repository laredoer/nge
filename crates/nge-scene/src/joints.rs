//! `scene.joints`：按精灵 id 把两个刚体（或一个刚体和世界）连起来（见 docs/joints.md）。

use std::collections::{HashMap, HashSet};

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_physics2d::{Joint, JointKind, Motor};
use serde::Deserialize;

use crate::protocol::{Scene, SpriteNode};
use crate::sync::{Declared, Node, sync_list};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JointKindName {
    Fixed,
    Revolute,
    Prismatic,
    Rope,
    Spring,
}

/// 一个关节。锚点、轴都是世界坐标，按两头精灵**声明的**位置和角度换算到各自身上。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JointNode {
    pub id: String,
    /// 精灵 id；不写时连到世界上。
    pub a: Option<String>,
    pub b: String,
    pub kind: JointKindName,
    /// a 上的锚点。转动、固定、滑动关节只要一个支点：只写 `anchorA` 或 `anchorB` 时两头都用它，都不写是 b 的位置。
    /// 绳子、弹簧两头各一个：`anchorA` 不写时是 a 的位置（连世界时必须写），`anchorB` 不写时是 b 的位置。
    pub anchor_a: Option<[f32; 2]>,
    /// b 上的锚点。
    pub anchor_b: Option<[f32; 2]>,
    /// 滑动关节的方向，默认 `[1, 0]`。
    pub axis: Option<[f32; 2]>,
    /// 转动关节：b 相对 a 能转的角度范围（度，从声明时的角度算起）；滑动关节：沿轴的范围（point，从锚点算起）。
    pub limits: Option<[f32; 2]>,
    pub motor: Option<MotorNode>,
    /// 绳子的最大长度、弹簧的静止长度；不写时是两个锚点声明时的距离。
    pub length: Option<f32>,
    /// 弹簧的劲度（按加速度算，和质量无关），默认 100。
    #[serde(default = "stiffness")]
    pub stiffness: f32,
    /// 弹簧的阻尼，默认 10。
    #[serde(default = "damping")]
    pub damping: f32,
    /// 连着的两个刚体之间还碰撞，默认不碰。
    #[serde(default)]
    pub collide: bool,
}

fn stiffness() -> f32 {
    100.0
}

fn damping() -> f32 {
    10.0
}

/// 马达：转动关节的 `speed` 是度 / 秒，滑动关节是 point / s。`maxForce` 不写时不限。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MotorNode {
    pub speed: f32,
    pub max_force: Option<f32>,
}

impl Node for JointNode {
    fn id(&self) -> &str {
        &self.id
    }
}

pub(crate) fn sync_joints(mut commands: Commands, scene: Res<Scene>, nodes: Query<(Entity, &Declared<JointNode>)>) {
    // 只记声明，`resolve` 每帧按声明算出 Joint
    sync_list(&mut commands, &scene.joints, &nodes, |_, _, _, _| {});
}

/// 精灵声明的位置和角度（弧度）。
type Pose = (Vec2, f32);

/// 每帧按 id 找两头的精灵，算出 `Joint`：精灵后来才有、换了声明、删掉了，都跟着改。
pub(crate) fn resolve(
    mut commands: Commands,
    joints: Query<(Entity, &Declared<JointNode>, Option<&Joint>)>,
    sprites: Query<(Entity, &Declared<SpriteNode>)>,
    mut warned: Local<HashSet<Entity>>,
) {
    if joints.is_empty() {
        warned.clear();
        return;
    }
    let by_id: HashMap<&str, (Entity, &SpriteNode)> = sprites.iter().map(|(e, d)| (d.0.id.as_str(), (e, &d.0))).collect();
    let mut seen = HashSet::new();
    for (entity, declared, current) in &joints {
        seen.insert(entity);
        let node = &declared.0;
        let found = |id: &str| by_id.get(id).copied().filter(|(_, s)| s.body.is_some());
        let a = node.a.as_deref().map(|id| found(id).ok_or(id));
        let b = found(&node.b).ok_or(node.b.as_str());
        let joint = match (a.transpose(), b) {
            (Ok(a), Ok(b)) => joint(node, a.map(|(e, s)| (e, pose(s))), (b.0, pose(b.1))),
            (Err(missing), _) | (_, Err(missing)) => Err(format!("没有 id 为 {missing:?} 的带 body 的精灵，先不连")),
        };
        // 每种情况只 warn 一次：连上过以后再出问题再 warn
        let joint = match joint {
            Ok(j) => {
                warned.remove(&entity);
                Some(j)
            }
            Err(why) => {
                if warned.insert(entity) {
                    log::warn!("关节 {:?}：{why}", node.id);
                }
                None
            }
        };
        match joint {
            Some(j) if current != Some(&j) => drop(commands.entity(entity).insert(j)),
            None if current.is_some() => drop(commands.entity(entity).remove::<Joint>()),
            _ => {}
        }
    }
    warned.retain(|e| seen.contains(e));
}

fn pose(sprite: &SpriteNode) -> Pose {
    (Vec2::new(sprite.x, sprite.y), sprite.rotation.to_radians())
}

/// 世界坐标的点换到刚体自己的坐标里。
fn local(point: Vec2, (center, rotation): Pose) -> Vec2 {
    Vec2::from_angle(-rotation).rotate(point - center)
}

fn joint(node: &JointNode, a: Option<(Entity, Pose)>, (b, b_pose): (Entity, Pose)) -> Result<Joint, String> {
    // 世界当成在原点、没转过的刚体
    let a_pose = a.map_or((Vec2::ZERO, 0.0), |(_, p)| p);
    let (given_a, given_b) = (node.anchor_a.map(Vec2::from), node.anchor_b.map(Vec2::from));
    let (anchor_a, anchor_b) = if matches!(node.kind, JointKindName::Rope | JointKindName::Spring) {
        let Some(anchor_a) = given_a.or(a.map(|_| a_pose.0)) else {
            return Err("连到世界上的绳子 / 弹簧要写 anchorA".to_owned());
        };
        (anchor_a, given_b.unwrap_or(b_pose.0))
    } else {
        // 只有一个支点：只写了一个就两头都用它，都不写是 b 的位置
        let pivot = given_a.or(given_b).unwrap_or(b_pose.0);
        (given_a.unwrap_or(pivot), given_b.unwrap_or(pivot))
    };
    let length = || node.length.unwrap_or(anchor_a.distance(anchor_b));
    let motor = |scale: f32| node.motor.map(|m| Motor { speed: m.speed * scale, max_force: m.max_force.unwrap_or(f32::INFINITY) });
    let kind = match node.kind {
        JointKindName::Fixed => JointKind::Fixed,
        JointKindName::Revolute => JointKind::Revolute { limits: node.limits.map(|l| l.map(f32::to_radians)), motor: motor(1f32.to_radians()) },
        JointKindName::Prismatic => {
            let axis = Vec2::from_angle(-a_pose.1).rotate(node.axis.map_or(Vec2::X, Vec2::from));
            JointKind::Prismatic { axis, limits: node.limits, motor: motor(1.0) }
        }
        JointKindName::Rope => JointKind::Rope { length: length() },
        JointKindName::Spring => JointKind::Spring { length: length(), stiffness: node.stiffness, damping: node.damping },
    };
    Ok(Joint {
        a: a.map(|(e, _)| e),
        b,
        kind,
        anchor_a: local(anchor_a, a_pose),
        anchor_b: local(anchor_b, b_pose),
        angle: b_pose.1 - a_pose.1,
        collide: node.collide,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use nge_physics2d::Physics;
    use nge_render2d::Transform2d;
    use serde_json::json;

    use super::*;
    use crate::sync::SceneId;
    use crate::testing::{app, frame};

    fn pos(app: &mut nge_ecs::App, id: &str) -> Vec2 {
        let world = app.world_mut();
        world.query::<(&SceneId, &Transform2d)>().iter(world).find(|(s, _)| s.0 == id).map(|(_, t)| t.translation).unwrap()
    }

    fn joint(app: &mut nge_ecs::App, id: &str) -> Option<Joint> {
        let world = app.world_mut();
        world.query::<(&SceneId, &Joint)>().iter(world).find(|(s, _)| s.0 == id).map(|(_, j)| *j)
    }

    fn linked(app: &nge_ecs::App) -> usize {
        app.world().resource::<Physics>().joints()
    }

    const BOB: &str = r#"{ "id": "bob", "x": 100, "y": 0, "w": 10, "body": { "shape": "circle" } }"#;

    fn bob() -> serde_json::Value {
        serde_json::from_str(BOB).unwrap()
    }

    /// 单摆：连到世界上的 (0, 0)，摆长 100 不变。
    #[test]
    fn pendulum_keeps_its_length() {
        let mut app = app();
        let scene = json!({ "sprites": [bob()], "joints": [{ "id": "j", "b": "bob", "kind": "revolute", "anchorA": [0, 0] }] });
        frame(&mut app, Some(scene));
        // 一帧最多补 4 步：8 帧约半秒，摆到最低点附近
        for _ in 0..7 {
            frame(&mut app, None);
        }
        let p = pos(&mut app, "bob");
        assert!((p.length() - 100.0).abs() < 1.0 && p.y < -50.0, "{p}");
        assert_eq!(linked(&app), 1);
    }

    #[test]
    fn anchors_are_converted_from_declared_poses() {
        let mut app = app();
        frame(&mut app, Some(json!({
            "sprites": [
                { "id": "a", "x": 10, "y": 0, "rotation": 90, "w": 10, "body": { "kind": "fixed" } },
                { "id": "b", "x": 10, "y": 50, "w": 10, "body": {} },
            ],
            "joints": [
                { "id": "hinge", "a": "a", "b": "b", "kind": "revolute", "anchorB": [10, 20], "limits": [-30, 30], "motor": { "speed": 90 } },
                { "id": "slide", "a": "a", "b": "b", "kind": "prismatic", "axis": [0, 1], "limits": [0, 40], "motor": { "speed": 5, "maxForce": 7 } },
                { "id": "rope", "a": "a", "b": "b", "kind": "rope" },
                { "id": "spring", "b": "b", "kind": "spring", "anchorA": [10, 80], "length": 20 },
            ],
        })));
        let hinge = joint(&mut app, "hinge").unwrap();
        // 锚点 (10, 20) 在 a 上：离 a 中心 (0, 20)，a 转了 90°，所以在 a 自己的坐标里是 (20, 0)
        assert!((hinge.anchor_a - Vec2::new(20.0, 0.0)).length() < 1e-4, "{}", hinge.anchor_a);
        assert!((hinge.anchor_b - Vec2::new(0.0, -30.0)).length() < 1e-4, "{}", hinge.anchor_b);
        assert!((hinge.angle + std::f32::consts::FRAC_PI_2).abs() < 1e-6, "b 相对 a 转了 -90°");
        let JointKind::Revolute { limits: Some([lo, hi]), motor: Some(m) } = hinge.kind else { panic!("{hinge:?}") };
        assert!((lo + 30f32.to_radians()).abs() < 1e-6 && (hi - 30f32.to_radians()).abs() < 1e-6);
        assert!((m.speed - std::f32::consts::FRAC_PI_2).abs() < 1e-6 && m.max_force == f32::INFINITY, "度换成弧度，不写 maxForce 不限");

        let slide = joint(&mut app, "slide").unwrap();
        let JointKind::Prismatic { axis, limits, motor } = slide.kind else { panic!() };
        assert!((axis - Vec2::X).length() < 1e-5, "世界的 y 在转了 90° 的 a 上是 x：{axis}");
        assert_eq!((limits, motor), (Some([0.0, 40.0]), Some(Motor { speed: 5.0, max_force: 7.0 })));
        assert!((slide.anchor_a - Vec2::new(50.0, 0.0)).length() < 1e-4, "不写 anchorA 时和 anchorB（b 的位置）一样");

        let rope = joint(&mut app, "rope").unwrap();
        assert_eq!((rope.anchor_a, rope.anchor_b), (Vec2::ZERO, Vec2::ZERO), "绳子不写锚点时两头各在中心");
        assert_eq!(rope.kind, JointKind::Rope { length: 50.0 }, "不写 length 时是声明时的距离");

        let spring = joint(&mut app, "spring").unwrap();
        assert_eq!((spring.a, spring.anchor_a), (None, Vec2::new(10.0, 80.0)), "连世界时锚点就是世界坐标");
        assert_eq!(spring.kind, JointKind::Spring { length: 20.0, stiffness: 100.0, damping: 10.0 });
    }

    /// 精灵后出现、没有 body、删掉，关节跟着连上和拆掉；关节声明改了就重建。
    #[test]
    fn joints_follow_sprites_and_declarations() {
        let mut app = app();
        let j = json!({ "id": "j", "b": "bob", "kind": "revolute", "anchorA": [0, 0] });
        frame(&mut app, Some(json!({ "joints": [j] })));
        assert!(joint(&mut app, "j").is_none(), "还没有 bob");

        frame(&mut app, Some(json!({ "sprites": [{ "id": "bob", "x": 100, "w": 10 }], "joints": [j] })));
        assert!(joint(&mut app, "j").is_none(), "bob 没有 body");

        frame(&mut app, Some(json!({ "sprites": [bob()], "joints": [j] })));
        assert!(joint(&mut app, "j").is_some());
        frame(&mut app, None);
        assert_eq!(linked(&app), 1);

        frame(&mut app, Some(json!({ "joints": [j] })));
        assert!(joint(&mut app, "j").is_none(), "bob 删掉了");
        assert_eq!(linked(&app), 0);

        // 精灵换了声明的位置：锚点按新位置重算
        frame(&mut app, Some(json!({ "sprites": [bob()], "joints": [j] })));
        assert_eq!(joint(&mut app, "j").unwrap().anchor_b, Vec2::new(-100.0, 0.0));
        let moved = json!({ "id": "bob", "x": 0, "y": -40, "w": 10, "body": { "shape": "circle" } });
        frame(&mut app, Some(json!({ "sprites": [moved], "joints": [j] })));
        assert_eq!(joint(&mut app, "j").unwrap().anchor_b, Vec2::new(0.0, 40.0));

        frame(&mut app, Some(json!({ "sprites": [bob()], "joints": [{ "id": "j", "b": "bob", "kind": "rope", "anchorA": [0, 0], "length": 300 }] })));
        assert!(matches!(joint(&mut app, "j").unwrap().kind, JointKind::Rope { length: 300.0 }));

        frame(&mut app, Some(json!({ "joints": [j] })));
        assert!(joint(&mut app, "j").is_none(), "bob 删掉了");
        frame(&mut app, Some(json!({})));
        let world = app.world_mut();
        assert_eq!(world.query::<&Declared<JointNode>>().iter(world).count(), 0);
        assert_eq!(linked(&app), 0);
    }

    /// 平台跳跃的铁链：4 节轻链子挂一个重球甩起来，两秒后每个连接点都没被拉开。
    #[test]
    fn a_heavy_chain_holds_together() {
        let mut app = app();
        let (x, y, link) = (240.0, 150.0, 18.0);
        let mut sprites: Vec<_> = (0..4).map(|i| json!({ "id": format!("l{i}"), "x": x, "y": y - link * (i as f32 + 0.5), "w": 6, "h": link, "body": { "density": 0.5 } })).collect();
        sprites.push(json!({ "id": "ball", "x": x, "y": y - link * 4.0 - 10.0, "w": 20, "body": { "shape": "circle", "density": 2, "velocity": [150, 0] } }));
        let ids = ["l0", "l1", "l2", "l3", "ball"];
        let joints: Vec<_> = ids.iter().enumerate().map(|(i, b)| json!({ "id": format!("j{i}"), "a": i.checked_sub(1).map(|p| ids[p]), "b": b, "kind": "revolute", "anchorA": [x, y - link * i as f32] })).collect();
        frame(&mut app, Some(json!({ "sprites": sprites, "joints": joints })));
        for _ in 0..30 {
            frame(&mut app, None);
        }
        let world = app.world_mut();
        let poses: HashMap<String, (Vec2, f32)> = world.query::<(&SceneId, &Transform2d)>().iter(world).map(|(s, t)| (s.0.clone(), (t.translation, t.rotation))).collect();
        // 第 i 个连接点：上一节的下端（或世界上的支点）和这一节的上端
        let end = |id: &str, local: Vec2| poses[id].0 + Vec2::from_angle(poses[id].1).rotate(local);
        for (i, id) in ids.iter().enumerate() {
            let top = if *id == "ball" { Vec2::new(0.0, 10.0) } else { Vec2::new(0.0, link / 2.0) };
            let above = if i == 0 { Vec2::new(x, y) } else { end(ids[i - 1], Vec2::new(0.0, -link / 2.0)) };
            let gap = end(id, top).distance(above);
            assert!(gap < 2.0, "第 {i} 个连接点拉开了 {gap}");
        }
        assert!(poses["ball"].0.distance(Vec2::new(x, y)) > 50.0, "球还挂在下面");
    }

    #[test]
    fn rope_to_the_world_needs_an_anchor() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [bob()], "joints": [{ "id": "j", "b": "bob", "kind": "rope" }] })));
        assert!(joint(&mut app, "j").is_none());
    }

    #[test]
    fn unknown_fields_and_kinds_are_rejected() {
        for bad in [json!({ "id": "j", "b": "x", "kind": "weld" }), json!({ "id": "j", "b": "x", "kind": "rope", "lenght": 3 })] {
            assert!(serde_json::from_value::<JointNode>(bad.clone()).is_err(), "{bad}");
        }
    }
}
