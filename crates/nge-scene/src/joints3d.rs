//! `scene.joints3d`：按模型 id 把两个 3D 刚体（或一个刚体和世界）连起来（见 docs/gameplay3d.md）。

use std::collections::{HashMap, HashSet};

use bevy_ecs::prelude::*;
use glam::{EulerRot, Quat, Vec3};
use nge_physics3d::{Joint3d, Joint3dKind, Motor3d};
use serde::Deserialize;

use crate::joints::MotorNode;
use crate::nodes::ModelNode;
use crate::protocol::Scene;
use crate::sync::{Declared, Node, sync_list};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Joint3dKindName {
    Fixed,
    Ball,
    Revolute,
    Prismatic,
    Rope,
    Spring,
}

/// 一个 3D 关节。锚点、轴都是世界坐标，按两头模型**声明的**位置和角度换算到各自身上。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Joint3dNode {
    pub id: String,
    /// 模型 id；不写时连到世界上。
    pub a: Option<String>,
    pub b: String,
    pub kind: Joint3dKindName,
    /// a 上的锚点。焊死、球窝、铰链、滑轨只要一个支点：只写 `anchorA` 或 `anchorB` 时两头都用它，都不写是 b 的位置。
    /// 绳子、弹簧两头各一个：`anchorA` 不写时是 a 的位置（连世界时必须写），`anchorB` 不写时是 b 的位置。
    pub anchor_a: Option<[f32; 3]>,
    /// b 上的锚点。
    pub anchor_b: Option<[f32; 3]>,
    /// 铰链绕的轴、滑轨的方向，默认 `[1, 0, 0]`。
    pub axis: Option<[f32; 3]>,
    /// 铰链：b 相对 a 能转的角度范围（度，从声明时的角度算起）；滑轨：沿轴的范围（point，从锚点算起）。
    pub limits: Option<[f32; 2]>,
    /// 铰链的 `speed` 是度 / 秒，滑轨是 point / s。
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

impl Node for Joint3dNode {
    fn id(&self) -> &str {
        &self.id
    }
}

#[cfg_attr(not(feature = "physics3d"), allow(dead_code))]
pub(crate) fn sync_joints3d(mut commands: Commands, scene: Res<Scene>, nodes: Query<(Entity, &Declared<Joint3dNode>)>) {
    // 只记声明，`resolve` 每帧按声明算出 Joint3d
    sync_list(&mut commands, &scene.joints3d, &nodes, |_, _, _, _| {});
}

/// 模型声明的位置和朝向。
type Pose = (Vec3, Quat);

/// 每帧按 id 找两头的模型，算出 `Joint3d`：模型后来才有、换了声明、删掉了，都跟着改。
#[cfg_attr(not(feature = "physics3d"), allow(dead_code))]
pub(crate) fn resolve(
    mut commands: Commands,
    joints: Query<(Entity, &Declared<Joint3dNode>, Option<&Joint3d>)>,
    models: Query<(Entity, &Declared<ModelNode>)>,
    mut warned: Local<HashSet<Entity>>,
) {
    if joints.is_empty() {
        warned.clear();
        return;
    }
    let by_id: HashMap<&str, (Entity, &ModelNode)> = models.iter().map(|(e, d)| (d.0.id.as_str(), (e, &d.0))).collect();
    let mut seen = HashSet::new();
    for (entity, declared, current) in &joints {
        seen.insert(entity);
        let node = &declared.0;
        let found = |id: &str| by_id.get(id).copied().filter(|(_, m)| m.body.is_some());
        let a = node.a.as_deref().map(|id| found(id).ok_or(id));
        let b = found(&node.b).ok_or(node.b.as_str());
        let joint = match (a.transpose(), b) {
            (Ok(a), Ok(b)) => joint(node, a.map(|(e, m)| (e, pose(m))), (b.0, pose(b.1))),
            (Err(missing), _) | (_, Err(missing)) => Err(format!("没有 id 为 {missing:?} 的带 body 的模型，先不连")),
        };
        // 每种情况只 warn 一次：连上过以后再出问题再 warn
        let joint = match joint {
            Ok(j) => {
                warned.remove(&entity);
                Some(j)
            }
            Err(why) => {
                if warned.insert(entity) {
                    log::warn!("3D 关节 {:?}：{why}", node.id);
                }
                None
            }
        };
        match joint {
            Some(j) if current != Some(&j) => drop(commands.entity(entity).insert(j)),
            None if current.is_some() => drop(commands.entity(entity).remove::<Joint3d>()),
            _ => {}
        }
    }
    warned.retain(|e| seen.contains(e));
}

/// 和 `sync_models` 一样换算角度。
fn pose(model: &ModelNode) -> Pose {
    let [rx, ry, rz] = model.rotation.map(f32::to_radians);
    (Vec3::new(model.x, model.y, model.z), Quat::from_euler(EulerRot::YXZ, ry, rx, rz))
}

/// 世界坐标的点换到刚体自己的坐标里。
fn local(point: Vec3, (center, rotation): Pose) -> Vec3 {
    rotation.inverse() * (point - center)
}

fn joint(node: &Joint3dNode, a: Option<(Entity, Pose)>, (b, b_pose): (Entity, Pose)) -> Result<Joint3d, String> {
    // 世界当成在原点、没转过的刚体
    let a_pose = a.map_or((Vec3::ZERO, Quat::IDENTITY), |(_, p)| p);
    let (given_a, given_b) = (node.anchor_a.map(Vec3::from), node.anchor_b.map(Vec3::from));
    let (anchor_a, anchor_b) = if matches!(node.kind, Joint3dKindName::Rope | Joint3dKindName::Spring) {
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
    let motor = |scale: f32| node.motor.map(|m| Motor3d { speed: m.speed * scale, max_force: m.max_force.unwrap_or(f32::INFINITY) });
    // 轴换到 a 的坐标里
    let axis = || a_pose.1.inverse() * node.axis.map_or(Vec3::X, Vec3::from);
    let kind = match node.kind {
        Joint3dKindName::Fixed => Joint3dKind::Fixed,
        Joint3dKindName::Ball => Joint3dKind::Ball,
        Joint3dKindName::Revolute => Joint3dKind::Revolute { axis: axis(), limits: node.limits.map(|l| l.map(f32::to_radians)), motor: motor(1f32.to_radians()) },
        Joint3dKindName::Prismatic => Joint3dKind::Prismatic { axis: axis(), limits: node.limits, motor: motor(1.0) },
        Joint3dKindName::Rope => Joint3dKind::Rope { length: length() },
        Joint3dKindName::Spring => Joint3dKind::Spring { length: length(), stiffness: node.stiffness, damping: node.damping },
    };
    Ok(Joint3d {
        a: a.map(|(e, _)| e),
        b,
        kind,
        anchor_a: local(anchor_a, a_pose),
        anchor_b: local(anchor_b, b_pose),
        rotation: a_pose.1.inverse() * b_pose.1,
        collide: node.collide,
    })
}

#[cfg(all(test, feature = "physics3d"))]
mod tests {
    use nge_physics3d::Physics3d;
    use nge_render3d::Transform3d;
    use serde_json::json;

    use super::*;
    use crate::sync::SceneId;
    use crate::testing::{app, frame};

    fn pos(app: &mut nge_ecs::App, id: &str) -> Vec3 {
        let world = app.world_mut();
        world.query::<(&SceneId, &Transform3d)>().iter(world).find(|(s, _)| s.0 == id).map(|(_, t)| t.translation).unwrap()
    }

    fn joint(app: &mut nge_ecs::App, id: &str) -> Option<Joint3d> {
        let world = app.world_mut();
        world.query::<(&SceneId, &Joint3d)>().iter(world).find(|(s, _)| s.0 == id).map(|(_, j)| *j)
    }

    fn bob() -> serde_json::Value {
        json!({ "id": "bob", "model": "asset:/@g/a.glb", "x": 100, "scale": 10, "body": { "shape": "ball" } })
    }

    /// 球窝的单摆：连到世界上的原点，摆长 100 不变。
    #[test]
    fn a_ball_pendulum_keeps_its_length() {
        let mut app = app();
        frame(&mut app, Some(json!({ "models": [bob()], "joints3d": [{ "id": "j", "b": "bob", "kind": "ball", "anchorA": [0, 0, 0] }] })));
        for _ in 0..7 {
            frame(&mut app, None);
        }
        let p = pos(&mut app, "bob");
        assert!((p.length() - 100.0).abs() < 1.0 && p.y < -50.0, "{p}");
        assert_eq!(app.world().resource::<Physics3d>().joints(), 1);
    }

    #[test]
    fn anchors_and_axes_are_converted_from_declared_poses() {
        let mut app = app();
        frame(&mut app, Some(json!({
            "models": [
                { "id": "a", "model": "asset:/@g/a.glb", "x": 10, "rotation": [0, 90, 0], "body": { "kind": "fixed" } },
                { "id": "b", "model": "asset:/@g/a.glb", "x": 10, "y": 50, "body": {} },
            ],
            "joints3d": [
                { "id": "hinge", "a": "a", "b": "b", "kind": "revolute", "anchorB": [10, 20, -20], "axis": [0, 0, -1], "limits": [-30, 30], "motor": { "speed": 90 } },
                { "id": "slide", "a": "a", "b": "b", "kind": "prismatic", "axis": [1, 0, 0], "limits": [0, 40], "motor": { "speed": 5, "maxForce": 7 } },
                { "id": "rope", "a": "a", "b": "b", "kind": "rope" },
                { "id": "spring", "b": "b", "kind": "spring", "anchorA": [10, 80, 0], "length": 20 },
            ],
        })));
        let hinge = joint(&mut app, "hinge").unwrap();
        // a 绕 y 转了 90°：世界的 -z 在 a 自己的坐标里是 -x
        assert!((hinge.anchor_a - Vec3::new(20.0, 20.0, 0.0)).length() < 1e-3, "{}", hinge.anchor_a);
        assert!((hinge.anchor_b - Vec3::new(0.0, -30.0, -20.0)).length() < 1e-3, "{}", hinge.anchor_b);
        assert!(hinge.rotation.angle_between(Quat::from_rotation_y(-90f32.to_radians())) < 1e-3, "b 相对 a 转了 -90°");
        let Joint3dKind::Revolute { axis, limits: Some([lo, hi]), motor: Some(m) } = hinge.kind else { panic!("{hinge:?}") };
        assert!((axis - Vec3::X).length() < 1e-5, "世界的 -z 在 a 上是 x：{axis}");
        assert!((lo + 30f32.to_radians()).abs() < 1e-6 && (hi - 30f32.to_radians()).abs() < 1e-6);
        assert!((m.speed - std::f32::consts::FRAC_PI_2).abs() < 1e-6 && m.max_force == f32::INFINITY, "度换成弧度，不写 maxForce 不限");

        let slide = joint(&mut app, "slide").unwrap();
        let Joint3dKind::Prismatic { axis, limits, motor } = slide.kind else { panic!() };
        assert!((axis - Vec3::Z).length() < 1e-5, "世界的 x 在 a 上是 z：{axis}");
        assert_eq!((limits, motor), (Some([0.0, 40.0]), Some(Motor3d { speed: 5.0, max_force: 7.0 })));

        let rope = joint(&mut app, "rope").unwrap();
        assert_eq!((rope.anchor_a, rope.anchor_b), (Vec3::ZERO, Vec3::ZERO), "绳子不写锚点时两头各在中心");
        assert_eq!(rope.kind, Joint3dKind::Rope { length: 50.0 });

        let spring = joint(&mut app, "spring").unwrap();
        assert_eq!((spring.a, spring.anchor_a), (None, Vec3::new(10.0, 80.0, 0.0)), "连世界时锚点就是世界坐标");
        assert_eq!(spring.kind, Joint3dKind::Spring { length: 20.0, stiffness: 100.0, damping: 10.0 });
    }

    #[test]
    fn joints_follow_models_and_declarations() {
        let mut app = app();
        let j = json!({ "id": "j", "b": "bob", "kind": "ball", "anchorA": [0, 0, 0] });
        frame(&mut app, Some(json!({ "joints3d": [j] })));
        assert!(joint(&mut app, "j").is_none(), "还没有 bob");
        frame(&mut app, Some(json!({ "models": [{ "id": "bob", "model": "asset:/@g/a.glb", "x": 100 }], "joints3d": [j] })));
        assert!(joint(&mut app, "j").is_none(), "bob 没有 body");
        frame(&mut app, Some(json!({ "models": [bob()], "joints3d": [j] })));
        assert_eq!(joint(&mut app, "j").unwrap().anchor_b, Vec3::new(-100.0, 0.0, 0.0));
        frame(&mut app, None);
        assert_eq!(app.world().resource::<Physics3d>().joints(), 1);
        frame(&mut app, Some(json!({ "joints3d": [j] })));
        assert!(joint(&mut app, "j").is_none(), "bob 删掉了");
        frame(&mut app, Some(json!({})));
        let world = app.world_mut();
        assert_eq!(world.query::<&Declared<Joint3dNode>>().iter(world).count(), 0);
        assert_eq!(app.world().resource::<Physics3d>().joints(), 0);
    }

    #[test]
    fn a_misspelled_joint_only_drops_that_joint() {
        let mut app = app();
        frame(&mut app, Some(json!({
            "models": [bob()],
            "joints3d": [
                { "id": "typo", "b": "bob", "kind": "ball", "anchroA": [0, 0, 0] },
                { "id": "j", "b": "bob", "kind": "ball", "anchorA": [0, 0, 0] },
            ],
        })));
        assert!(joint(&mut app, "typo").is_none(), "写错的那个跳过");
        assert!(joint(&mut app, "j").is_some(), "别的关节照常");
        assert!((pos(&mut app, "bob").x - 100.0).abs() < 1.0, "模型照常");
    }

    #[test]
    fn unknown_fields_and_kinds_are_rejected() {
        for bad in [json!({ "id": "j", "b": "x", "kind": "hinge" }), json!({ "id": "j", "b": "x", "kind": "rope", "anchor": [0, 0, 0] })] {
            assert!(serde_json::from_value::<Joint3dNode>(bad.clone()).is_err(), "{bad}");
        }
        assert!(serde_json::from_value::<Joint3dNode>(json!({ "id": "j", "b": "x", "kind": "ball", "anchorA": [0, 0] })).is_err(), "锚点要三个数");
    }
}
