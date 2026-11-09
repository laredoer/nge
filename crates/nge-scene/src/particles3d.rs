//! 3D 粒子发射器和模型的拖尾（见 docs/particles3d.md）。

use bevy_ecs::prelude::*;
use glam::{Vec3, Vec4};
use nge_assets::ImageId;
use nge_particles::{ParticleEmitter3d, Particles3d, Trail3d, TrailPoints};
use nge_render2d::SpriteImage;
use nge_render3d::{Blend, Transform3d};
use serde::Deserialize;

use crate::protocol::{Color, Scene};
use crate::resolve::SceneAssets;
use crate::sync::{Declared, Node, sync_list};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlendNode {
    #[default]
    Alpha,
    Add,
}

impl From<BlendNode> for Blend {
    fn from(b: BlendNode) -> Self {
        match b {
            BlendNode::Alpha => Blend::Alpha,
            BlendNode::Add => Blend::Add,
        }
    }
}

/// 一直发射的 3D 粒子发射器。范围写成 `[最小, 最大]`，角度用度。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Emitter3dNode {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// 每秒发射多少个。
    pub rate: f32,
    pub lifetime: [f32; 2],
    pub speed: [f32; 2],
    /// 发射方向，默认朝上。
    pub direction: [f32; 3],
    /// 方向锥的半角（度），默认 180（全方向）。
    pub spread: f32,
    pub gravity: [f32; 3],
    pub size: [f32; 2],
    /// 方片转动的速度（度 / 秒）。
    pub spin: [f32; 2],
    /// [出生, 死亡]；不填是白色淡出。
    pub color: Option<[Color; 2]>,
    pub texture: Option<String>,
    pub blend: BlendNode,
    pub max: Option<usize>,
}

impl Default for Emitter3dNode {
    fn default() -> Self {
        let d = ParticleEmitter3d::default();
        Self {
            id: String::new(),
            x: 0.0,
            y: 0.0,
            z: 0.0,
            rate: 0.0,
            lifetime: [d.lifetime.0, d.lifetime.1],
            speed: [d.speed.0, d.speed.1],
            direction: d.direction.into(),
            spread: 180.0,
            gravity: [0.0; 3],
            size: [d.size.0, d.size.1],
            spin: [0.0; 2],
            color: None,
            texture: None,
            blend: BlendNode::Alpha,
            max: None,
        }
    }
}

impl Node for Emitter3dNode {
    fn id(&self) -> &str {
        &self.id
    }
}

/// 模型的拖尾。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TrailNode {
    /// 一个点留多久（秒），默认 0.5。
    pub lifetime: f32,
    /// [新, 旧] 两头的宽。
    pub width: [f32; 2],
    /// [新, 旧]；不填是白色淡出。
    pub color: Option<[Color; 2]>,
    /// 相邻两个点至少隔多远。
    pub min_distance: f32,
    /// 最多记多少个点，默认 64。
    pub max_points: usize,
    pub texture: Option<String>,
    pub blend: BlendNode,
}

impl Default for TrailNode {
    fn default() -> Self {
        let d = Trail3d::default();
        Self { lifetime: d.lifetime, width: [d.width.0, d.width.1], color: None, min_distance: d.min_distance, max_points: d.max_points, texture: None, blend: BlendNode::Alpha }
    }
}

/// 3D 粒子和拖尾只用单张贴图；图集当成白色。
fn texture(assets: &mut SceneAssets, owner: &str, name: Option<&str>) -> ImageId {
    match assets.texture(owner, name, None, 0) {
        SpriteImage::Texture(t) => t,
        SpriteImage::Atlas(..) => {
            log::warn!("节点 {owner:?} 的 3D 粒子 / 拖尾不支持图集，画成白色");
            ImageId::WHITE
        }
    }
}

fn colors(color: &Option<[Color; 2]>, default: (Vec4, Vec4)) -> (Vec4, Vec4) {
    color.as_ref().map_or(default, |[a, b]| (a.0, b.0))
}

pub(crate) fn emitter3d(node: &Emitter3dNode, assets: &mut SceneAssets) -> ParticleEmitter3d {
    let d = ParticleEmitter3d::default();
    ParticleEmitter3d {
        rate: node.rate,
        lifetime: (node.lifetime[0], node.lifetime[1]),
        speed: (node.speed[0], node.speed[1]),
        direction: Vec3::from(node.direction),
        spread: node.spread.to_radians(),
        gravity: Vec3::from(node.gravity),
        size: (node.size[0], node.size[1]),
        spin: (node.spin[0].to_radians(), node.spin[1].to_radians()),
        color: colors(&node.color, d.color),
        texture: texture(assets, &node.id, node.texture.as_deref()),
        blend: node.blend.into(),
        max: node.max.unwrap_or(d.max),
        ..d
    }
}

pub(crate) fn sync_emitters3d(mut commands: Commands, scene: Res<Scene>, mut assets: SceneAssets, nodes: Query<(Entity, &Declared<Emitter3dNode>)>) {
    sync_list(&mut commands, &scene.emitters3d, &nodes, |commands, entity, before, node| {
        let emitter = emitter3d(node, &mut assets);
        let mut entity = commands.entity(entity);
        // 只改了参数、没换贴图时保留已经发射出去的粒子
        if before.is_none_or(|b| b.texture != node.texture) {
            entity.remove::<Particles3d>();
        }
        entity.insert((emitter, Transform3d::from_xyz(node.x, node.y, node.z)));
    });
}

/// 模型同步时调：按 `trail` 加上、更新或去掉拖尾。只改参数时保留已经记下的点；`teleported` 时清掉，不拉出一条长带子。
pub(crate) fn apply_trail(entity: &mut EntityCommands, owner: &str, trail: Option<&TrailNode>, teleported: bool, assets: &mut SceneAssets) {
    let Some(t) = trail else {
        entity.remove::<(Trail3d, TrailPoints)>();
        return;
    };
    let d = Trail3d::default();
    entity.insert(Trail3d {
        lifetime: t.lifetime,
        width: (t.width[0], t.width[1]),
        color: colors(&t.color, d.color),
        min_distance: t.min_distance,
        max_points: t.max_points,
        texture: texture(assets, owner, t.texture.as_deref()),
        blend: t.blend.into(),
    });
    if teleported {
        entity.insert(TrailPoints::default());
    }
}

#[cfg(test)]
mod tests {
    use nge_render3d::Billboards;
    use serde_json::json;

    use super::*;
    use crate::sync::SceneId;
    use crate::testing::{app, frame};

    fn entity(app: &mut nge_ecs::App, id: &str) -> Option<Entity> {
        let world = app.world_mut();
        world.query::<(Entity, &SceneId)>().iter(world).find(|(_, s)| s.0 == id).map(|(e, _)| e)
    }

    #[test]
    fn emitters3d_fire_billboards_and_keep_particles_when_only_tuned() {
        let mut app = app();
        let scene = |rate: f32, texture: Option<&str>| json!({ "emitters3d": [{ "id": "fire", "x": 1, "y": 2, "z": 3, "rate": rate, "lifetime": [100, 100], "blend": "add", "spin": [90, 90], "spread": 90, "texture": texture }] });
        frame(&mut app, Some(scene(10.0, None)));
        frame(&mut app, None);
        let fire = entity(&mut app, "fire").unwrap();
        let e = app.world().get::<ParticleEmitter3d>(fire).unwrap().clone();
        assert_eq!((e.rate, e.blend, e.spin.0, e.spread), (10.0, Blend::Add, std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2));
        assert_eq!(app.world().get::<Transform3d>(fire).unwrap().translation, Vec3::new(1.0, 2.0, 3.0));
        let alive = |app: &nge_ecs::App| app.world().get::<Particles3d>(fire).unwrap().len();
        let before = alive(&app);
        assert!(before > 0);
        assert_eq!(app.world().resource::<Billboards>().quads(), before);
        frame(&mut app, Some(scene(20.0, None)));
        let tuned = alive(&app);
        assert!(tuned > before, "只改 rate 不清掉已经发射的");
        frame(&mut app, Some(scene(20.0, Some("asset:/@g/spark.png"))));
        assert!(alive(&app) < tuned, "换贴图重新来");
        frame(&mut app, Some(json!({})));
        assert!(entity(&mut app, "fire").is_none());
    }

    #[test]
    fn model_trails_follow_and_go_away_with_the_declaration() {
        let mut app = app();
        let scene = |x: f32, trail: serde_json::Value| json!({ "models": [{ "id": "box", "model": "asset:/@g/a.glb", "x": x, "trail": trail }] });
        frame(&mut app, Some(scene(0.0, json!({ "lifetime": 100, "width": [6, 0], "blend": "add" }))));
        frame(&mut app, Some(scene(10.0, json!({ "lifetime": 100, "width": [6, 0], "blend": "add" }))));
        let b = entity(&mut app, "box").unwrap();
        assert_eq!(app.world().get::<Trail3d>(b).unwrap().width, (6.0, 0.0));
        assert_eq!(app.world().get::<TrailPoints>(b).unwrap().len(), 2);
        assert_eq!(app.world().resource::<Billboards>().ribbons(), 1);
        frame(&mut app, Some(scene(20.0, json!({ "lifetime": 100, "width": [8, 0], "maxPoints": 7 }))));
        assert_eq!(app.world().get::<Trail3d>(b).unwrap().max_points, 7);
        assert_eq!(app.world().get::<TrailPoints>(b).unwrap().len(), 3, "只改参数时保留记下的点");
        assert_eq!(app.world().get::<Trail3d>(b).unwrap().blend, Blend::Alpha);
        frame(&mut app, Some(json!({ "models": [{ "id": "box", "model": "asset:/@g/a.glb", "x": 20 }] })));
        assert!(app.world().get::<Trail3d>(b).is_none() && app.world().get::<TrailPoints>(b).is_none(), "去掉 trail 马上消失");
        assert_eq!(app.world().resource::<Billboards>().ribbons(), 0);
    }

    #[test]
    fn redeclaring_a_body_position_clears_the_trail() {
        let mut app = app();
        let scene = |x: f32| json!({ "models": [{ "id": "box", "model": "asset:/@g/a.glb", "x": x, "body": { "gravity": 0 }, "trail": { "lifetime": 100 } }] });
        frame(&mut app, Some(scene(0.0)));
        let b = entity(&mut app, "box").unwrap();
        app.world_mut().get_mut::<Transform3d>(b).unwrap().translation.x = 5.0;
        frame(&mut app, None);
        assert!(app.world().get::<TrailPoints>(b).unwrap().len() >= 2);
        frame(&mut app, Some(scene(300.0)));
        assert_eq!(app.world().get::<TrailPoints>(b).unwrap().len(), 1, "瞬移后从新位置重新记");
    }

    #[test]
    fn nodes_parse_with_defaults() {
        let e: Emitter3dNode = serde_json::from_value(json!({ "id": "fire", "rate": 30, "spread": 20, "blend": "add", "gravity": [0, 1, 0] })).unwrap();
        assert_eq!((e.rate, e.spread, e.blend, e.gravity, e.direction), (30.0, 20.0, BlendNode::Add, [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]));
        assert_eq!(Emitter3dNode::default().spread, 180.0);
        let t: TrailNode = serde_json::from_value(json!({ "minDistance": 4, "maxPoints": 9, "width": [6, 0], "color": ["#ffff00", "#ffff0000"] })).unwrap();
        assert_eq!((t.min_distance, t.max_points, t.width, t.lifetime, t.blend), (4.0, 9, [6.0, 0.0], 0.5, BlendNode::Alpha));
        assert_eq!(TrailNode::default().max_points, 64);
        assert_eq!(t.color.unwrap()[1].0.w, 0.0);
        assert!(serde_json::from_value::<Emitter3dNode>(json!({ "id": "x", "blend": "multiply" })).is_err());
    }
}
