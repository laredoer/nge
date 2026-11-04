//! 精灵节点上的 `body`：刚体、碰撞体，`collide` 事件、`push` command 和 `scene.physics`。

use std::collections::{HashMap, HashSet};

use bevy_ecs::prelude::*;
use bevy_ecs::system::EntityCommands;
use glam::Vec2;
use nge_core::{CommandKey, EventKey};
use nge_ecs::Outbox;
use nge_physics2d::{Collider, Collision, Impulse, LockRotation, Physics, RigidBody, Velocity};
use nge_render2d::Sprite;
use serde::{Deserialize, Serialize};

use crate::protocol::{Scene, SpriteNode};
use crate::sync::{Declared, SceneId};
use crate::tilemap::{TileSolid, TilemapNode};

pub const COLLIDE: EventKey<Collide> = EventKey::new("collide");
pub const PUSH: CommandKey<Push> = CommandKey::new("push");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BodyKind {
    #[default]
    Dynamic,
    Fixed,
    Kinematic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShapeName {
    #[default]
    Box,
    Circle,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BodyNode {
    pub kind: BodyKind,
    pub shape: ShapeName,
    pub w: Option<f32>,
    pub h: Option<f32>,
    pub r: Option<f32>,
    pub restitution: f32,
    pub friction: f32,
    pub density: f32,
    pub sensor: bool,
    pub velocity: [f32; 2],
    /// 度 / 秒。
    pub angular_velocity: f32,
    pub collide: bool,
    /// 不让碰撞把它转起来（平台游戏的角色）。
    pub fixed_rotation: bool,
}

impl Default for BodyNode {
    fn default() -> Self {
        Self {
            kind: BodyKind::Dynamic,
            shape: ShapeName::Box,
            w: None,
            h: None,
            r: None,
            restitution: 0.0,
            friction: 0.5,
            density: 1.0,
            sensor: false,
            velocity: [0.0; 2],
            angular_velocity: 0.0,
            collide: false,
            fixed_rotation: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PhysicsNode {
    pub gravity: Option<[f32; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Collide {
    pub a: String,
    pub b: String,
    pub started: bool,
}

/// 给节点的刚体加上 (x, y) 的速度（point / s，和质量无关）。
#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
pub struct Push {
    pub id: String,
    pub x: f32,
    pub y: f32,
}

/// 按精灵节点的 `body` 挂上或拿掉物理组件。`size` 是精灵声明的大小（没乘缩放）。
pub(crate) fn apply_body(entity: &mut EntityCommands, before: Option<&SpriteNode>, node: &SpriteNode, size: Option<Vec2>) {
    let Some(body) = &node.body else {
        entity.remove::<(RigidBody, Collider, Velocity, Impulse, LockRotation)>();
        return;
    };
    let kind = match body.kind {
        BodyKind::Dynamic => RigidBody::Dynamic,
        BodyKind::Fixed => RigidBody::Fixed,
        BodyKind::Kinematic => RigidBody::Kinematic,
    };
    // 不覆盖 Rust 系统设了还没用掉的冲量
    entity.insert(kind).insert_if_new(Impulse::default());
    if body.fixed_rotation {
        entity.insert(LockRotation);
    } else {
        entity.remove::<LockRotation>();
    }
    match collider(body, size, node.scale) {
        Some(c) => entity.insert(c),
        None => {
            log::warn!("节点 {:?} 的 body 不知道多大（精灵和 body 都没写 w / h / r），只建刚体", node.id);
            entity.remove::<Collider>()
        }
    };
    // 初速度只在生成、改了 velocity、或者声明的位置变了（重新放一次）时设置，不打断已经在动的刚体
    let old = before.and_then(|b| b.body.as_ref().map(|body| (b.x, b.y, b.rotation, body.velocity, body.angular_velocity)));
    if old != Some((node.x, node.y, node.rotation, body.velocity, body.angular_velocity)) {
        entity.insert(Velocity { linear: Vec2::from(body.velocity), angular: body.angular_velocity.to_radians() });
    }
}

/// 碰撞体大小按精灵本地的大小算，再乘精灵的缩放。
fn collider(body: &BodyNode, size: Option<Vec2>, scale: f32) -> Option<Collider> {
    let mut c = match body.shape {
        ShapeName::Box => Collider::rect(square(body.w, body.h).or(size)? * scale),
        ShapeName::Circle => Collider::circle(body.r.or(size.map(|s| s.min_element() / 2.0))? * scale),
    };
    c.restitution = body.restitution;
    c.friction = body.friction;
    c.density = body.density;
    c.sensor = body.sensor;
    c.events = body.collide;
    Some(c)
}

/// 只给 w 或 h 时按正方形，都不给时是 `None`。
pub(crate) fn square(w: Option<f32>, h: Option<f32>) -> Option<Vec2> {
    match (w, h) {
        (Some(w), Some(h)) => Some(Vec2::new(w, h)),
        (Some(s), None) | (None, Some(s)) => Some(Vec2::splat(s)),
        (None, None) => None,
    }
}

/// 瓦片地图的接触：一张地图有好几个碰撞体，按（地图, 另一方）记正接触着的碰撞体，
/// 从没有到有才发 started、从有到没有才发 stopped。碰撞体删掉以后还要靠这里记的归属找到地图。
#[derive(Default)]
pub(crate) struct TileContacts {
    touching: HashMap<(Entity, Entity), HashSet<Entity>>,
    owner: HashMap<Entity, Entity>,
}

/// 两个都是场景节点（精灵或瓦片地图）时把碰撞转成 `collide` 事件（rapier 只给开了 `collide` 的碰撞体发）。
pub(crate) fn collide(
    mut reader: MessageReader<Collision>,
    ids: Query<&SceneId, Or<(With<Sprite>, With<Declared<TilemapNode>>)>>,
    tiles: Query<&ChildOf, With<TileSolid>>,
    mut contacts: Local<TileContacts>,
    mut outbox: ResMut<Outbox>,
) {
    let contacts = &mut *contacts;
    let mut send = |a: Entity, b: Entity, started: bool| {
        if let (Ok(a), Ok(b)) = (ids.get(a), ids.get(b)) {
            outbox.send(COLLIDE, &Collide { a: a.0.clone(), b: b.0.clone(), started });
        }
    };
    for c in reader.read() {
        let mut map_of = |e: Entity| match tiles.get(e) {
            Ok(child) => Some(*contacts.owner.entry(e).or_insert(child.parent())),
            Err(_) => contacts.owner.get(&e).copied(),
        };
        // 地图那一方换成地图实体，另一方照旧
        let (tile, map, other) = match (map_of(c.a), map_of(c.b)) {
            (Some(map), _) => (c.a, map, c.b),
            (None, Some(map)) => (c.b, map, c.a),
            (None, None) => {
                send(c.a, c.b, c.started);
                continue;
            }
        };
        let set = contacts.touching.entry((map, other)).or_default();
        if c.started {
            if set.is_empty() {
                send(map, other, true);
            }
            set.insert(tile);
        } else if set.remove(&tile) && set.is_empty() {
            contacts.touching.remove(&(map, other));
            send(map, other, false);
        }
    }
    // 一块碰撞体删掉时每个接触方各有一条 stopped，全部处理完再清归属
    if !contacts.owner.is_empty() {
        contacts.owner.retain(|e, _| tiles.contains(*e));
    }
}

pub(crate) fn push(mut reader: MessageReader<Push>, mut bodies: Query<(&Declared<SpriteNode>, &RigidBody, &mut Velocity)>) {
    for p in reader.read() {
        match bodies.iter_mut().find(|(d, ..)| d.0.id == p.id) {
            Some((_, RigidBody::Dynamic, mut v)) => v.linear += Vec2::new(p.x, p.y),
            Some(_) => log::warn!("push：{:?} 不是 dynamic 刚体，推不动", p.id),
            None => log::warn!("push：没有 id 为 {:?} 的刚体", p.id),
        }
    }
}

/// `scene.physics.gravity`：变了才覆盖；不写时回到插件的默认重力。
pub(crate) fn sync_gravity(scene: Res<Scene>, mut physics: ResMut<Physics>, mut default: Local<Option<Vec2>>, mut last: Local<Option<Option<[f32; 2]>>>) {
    let default = *default.get_or_insert(physics.gravity());
    let gravity = scene.physics.as_ref().and_then(|p| p.gravity);
    if last.as_ref() == Some(&gravity) {
        return;
    }
    physics.set_gravity(gravity.map_or(default, Vec2::from));
    *last = Some(gravity);
}

#[cfg(test)]
mod tests {
    use nge_render2d::Transform2d;
    use serde_json::json;

    use super::*;
    use crate::testing::{app, frame};

    fn get<C: Component + Clone>(app: &mut nge_ecs::App, id: &str) -> Option<C> {
        let world = app.world_mut();
        world.query::<(&SceneId, &C)>().iter(world).find(|(s, _)| s.0 == id).map(|(_, c)| c.clone())
    }

    fn y(app: &mut nge_ecs::App, id: &str) -> f32 {
        get::<Transform2d>(app, id).unwrap().translation.y
    }

    #[test]
    fn bodies_fall_and_keep_their_place_when_other_fields_change() {
        let mut app = app();
        let ball = |color: &str, x: f32| json!({ "id": "ball", "x": x, "y": 100, "w": 20, "h": 20, "color": color, "body": { "shape": "circle" } });
        frame(&mut app, Some(json!({ "sprites": [ball("#ffffff", 0.0)] })));
        frame(&mut app, None);
        let fallen = y(&mut app, "ball");
        assert!(fallen < 100.0, "{fallen}");
        assert_eq!(get::<Collider>(&mut app, "ball").unwrap().shape, nge_physics2d::Shape::Circle(10.0), "半径取短边的一半");

        frame(&mut app, Some(json!({ "sprites": [ball("#ff0000", 0.0)] })));
        assert!(y(&mut app, "ball") < fallen, "改颜色不把它拉回去");

        frame(&mut app, Some(json!({ "sprites": [ball("#ff0000", 50.0)] })));
        let t = get::<Transform2d>(&mut app, "ball").unwrap().translation;
        assert!((t.x - 50.0).abs() < 1e-3 && t.y > fallen, "声明的位置变了就瞬移过去，速度也重来: {t}");
    }

    #[test]
    fn collider_sizes_and_removal() {
        let mut app = app();
        frame(&mut app, Some(json!({ "sprites": [
            { "id": "a", "w": 10, "h": 20, "scale": 2, "body": { "kind": "fixed" } },
            { "id": "b", "w": 10, "body": { "kind": "kinematic", "w": 4, "h": 6, "friction": 0.1, "collide": true } },
            { "id": "c", "texture": "plain", "body": {} },
        ] })));
        let a = get::<Collider>(&mut app, "a").unwrap();
        assert_eq!((a.shape, get::<RigidBody>(&mut app, "a")), (nge_physics2d::Shape::Rect(Vec2::new(20.0, 40.0)), Some(RigidBody::Fixed)));
        let b = get::<Collider>(&mut app, "b").unwrap();
        assert_eq!((b.shape, b.friction, b.events), (nge_physics2d::Shape::Rect(Vec2::new(4.0, 6.0)), 0.1, true));
        assert!(get::<Collider>(&mut app, "c").is_none() && get::<RigidBody>(&mut app, "c").is_some(), "不知道多大时只建刚体");

        frame(&mut app, Some(json!({ "sprites": [{ "id": "a", "w": 10, "h": 20 }] })));
        assert!(get::<RigidBody>(&mut app, "a").is_none() && get::<Collider>(&mut app, "a").is_none());
        assert_eq!(app.world().resource::<Physics>().len(), (0, 0));
    }

    #[test]
    fn collide_events_and_push() {
        let mut app = app();
        let scene = json!({ "physics": { "gravity": [0, 0] }, "sprites": [
            { "id": "wall", "x": 30, "w": 10, "h": 100, "body": { "kind": "fixed" } },
            { "id": "ball", "w": 10, "body": { "shape": "circle", "collide": true } },
        ] });
        let mut events = frame(&mut app, Some(scene));
        app.command("push", &json!({ "id": "ball", "x": 300, "y": 0 }));
        for _ in 0..10 {
            events.extend(frame(&mut app, None));
        }
        let hits: Vec<Collide> = events.iter().filter(|(n, _)| n == "collide").map(|(_, d)| COLLIDE.decode(d).unwrap()).collect();
        let first = hits.first().expect("撞到墙");
        assert!(first.started);
        assert_eq!([first.a.as_str(), first.b.as_str()].map(|s| s == "ball" || s == "wall"), [true, true]);
        assert!(get::<Transform2d>(&mut app, "ball").unwrap().translation.x < 30.0, "被墙挡住");
    }

    #[test]
    fn fixed_rotation_locks_and_unlocks() {
        let mut app = app();
        let scene = |fixed: bool| json!({ "sprites": [{ "id": "p", "w": 10, "body": { "fixedRotation": fixed } }] });
        frame(&mut app, Some(scene(true)));
        assert!(get::<LockRotation>(&mut app, "p").is_some());
        frame(&mut app, Some(scene(false)));
        assert!(get::<LockRotation>(&mut app, "p").is_none());
    }

    #[test]
    fn gravity_follows_the_scene() {
        let mut app = app();
        let default = app.world().resource::<Physics>().gravity();
        frame(&mut app, Some(json!({ "physics": { "gravity": [0, 50] } })));
        assert_eq!(app.world().resource::<Physics>().gravity(), Vec2::new(0.0, 50.0));
        frame(&mut app, Some(json!({})));
        assert_eq!(app.world().resource::<Physics>().gravity(), default);
    }
}
