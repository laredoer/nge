//! 3D 粒子发射器和模型的拖尾（见 docs/particles3d.md）。

use bevy_ecs::prelude::*;
use glam::{Vec3, Vec4};
use nge_assets::ImageId;
use glam::{EulerRot, Quat};
use nge_particles::{
    Curve, EmitDirection, EmitShape, Gradient, MeshSource, Noise3d, ParticleCollision, ParticleEmitter3d, ParticleFacing, Particles3d, SheetMode, SpriteSheet, SubEmitter3d, SubTrigger, Trail3d,
    TrailPoints,
};
use nge_render2d::SpriteImage;
use nge_render3d::{Blend, Transform3d};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::lenient::Fields;

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
/// 原来的字段写错时整个节点跳过；3D 粒子补齐（见 docs/particles3d-more.md）加的字段逐字段解析，写错的 warn 后当没写。
#[derive(Debug, Clone, PartialEq)]
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
    /// 出生时的转角范围（度）。
    pub angle: Option<[f32; 2]>,
    pub shape: Option<ShapeNode>,
    pub emit_direction: Option<EmitDirectionNode>,
    pub size_curve: Option<Vec<[f32; 2]>>,
    pub alpha_curve: Option<Vec<[f32; 2]>>,
    pub speed_curve: Option<Vec<[f32; 2]>>,
    pub color_gradient: Option<Vec<(f32, Color)>>,
    pub drag: Option<f32>,
    pub noise: Option<NoiseNode>,
    pub facing: Option<FacingNode>,
    pub stretch: Option<f32>,
    pub sheet: Option<SheetNode>,
    pub soft: Option<f32>,
    pub lit: Option<bool>,
    pub collision: Option<CollisionNode>,
    pub sub_emitters: Vec<SubEmitterNode>,
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
            angle: None,
            shape: None,
            emit_direction: None,
            size_curve: None,
            alpha_curve: None,
            speed_curve: None,
            color_gradient: None,
            drag: None,
            noise: None,
            facing: None,
            stretch: None,
            sheet: None,
            soft: None,
            lit: None,
            collision: None,
            sub_emitters: Vec::new(),
        }
    }
}

const EMITTER: &str = "emitters3d";

impl<'de> Deserialize<'de> for Emitter3dNode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut f = Fields::new(EMITTER, Value::deserialize(d)?).map_err(serde::de::Error::custom)?;
        let n = Self::default();
        // 原来的字段：写错整个节点跳过（和以前一样）
        macro_rules! strict {
            ($key:literal, $default:expr) => {
                f.strict($key).map_err(serde::de::Error::custom)?.unwrap_or($default)
            };
        }
        let node = Self {
            id: strict!("id", n.id),
            x: strict!("x", n.x),
            y: strict!("y", n.y),
            z: strict!("z", n.z),
            rate: strict!("rate", n.rate),
            lifetime: strict!("lifetime", n.lifetime),
            speed: strict!("speed", n.speed),
            direction: strict!("direction", n.direction),
            spread: strict!("spread", n.spread),
            gravity: strict!("gravity", n.gravity),
            size: strict!("size", n.size),
            spin: strict!("spin", n.spin),
            color: f.strict("color").map_err(serde::de::Error::custom)?,
            texture: f.strict("texture").map_err(serde::de::Error::custom)?,
            blend: strict!("blend", n.blend),
            max: f.strict("max").map_err(serde::de::Error::custom)?,
            angle: f.get("angle"),
            shape: f.get("shape"),
            emit_direction: f.get("emitDirection"),
            size_curve: f.get("sizeCurve"),
            alpha_curve: f.get("alphaCurve"),
            speed_curve: f.get("speedCurve"),
            color_gradient: f.get("colorGradient"),
            drag: f.get("drag"),
            noise: f.get("noise"),
            facing: f.get("facing"),
            stretch: f.get("stretch"),
            sheet: f.get("sheet"),
            soft: f.get("soft"),
            lit: f.get("lit"),
            collision: f.get("collision"),
            sub_emitters: f.get::<Value>("subEmitters").map(|v| crate::lenient::items("emitters3d.subEmitters", v)).unwrap_or_default(),
        };
        f.finish();
        Ok(node)
    }
}

/// 发射形状（`kind` 区分），角度用度。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ShapeNode {
    Point,
    Sphere {
        radius: f32,
        #[serde(default)]
        surface: bool,
    },
    Hemisphere {
        radius: f32,
        #[serde(default)]
        surface: bool,
    },
    Box {
        size: [f32; 3],
        #[serde(default)]
        surface: bool,
    },
    Circle {
        radius: f32,
        #[serde(default)]
        inner: f32,
    },
    Cone {
        radius: f32,
        angle: f32,
    },
    Mesh {
        model: String,
        #[serde(default)]
        scale: Option<ScaleNode>,
    },
}

/// 缩放：一个数或 `[x, y, z]`。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ScaleNode {
    Uniform(f32),
    Axes([f32; 3]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmitDirectionNode {
    Cone,
    Normal,
    Random,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FacingMode {
    Camera,
    Velocity,
    Horizontal,
}

/// 朝向：`"camera"`、`"velocity"`、`"horizontal"`，或固定朝向的欧拉角（度）。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum FacingNode {
    Mode(FacingMode),
    Fixed([f32; 3]),
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NoiseNode {
    pub strength: f32,
    pub frequency: f32,
    pub scroll: f32,
}

impl Default for NoiseNode {
    fn default() -> Self {
        let d = Noise3d::default();
        Self { strength: d.strength, frequency: d.frequency, scroll: d.scroll }
    }
}

/// 序列帧：`grid` 是 [列, 行]；不写 `fps` 时按寿命播完。逐字段解析。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SheetNode {
    pub grid: [u32; 2],
    pub frames: Option<u32>,
    pub fps: Option<f32>,
    pub random_start: Option<bool>,
    pub blend: Option<bool>,
}

impl<'de> Deserialize<'de> for SheetNode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut f = Fields::new("emitters3d.sheet", Value::deserialize(d)?).map_err(serde::de::Error::custom)?;
        // 没有格子就切不了帧：整个 sheet 当没写
        let grid = f.strict("grid").map_err(serde::de::Error::custom)?.ok_or_else(|| serde::de::Error::custom("sheet 要写 grid"))?;
        let node = Self { grid, frames: f.get("frames"), fps: f.get("fps"), random_start: f.get("randomStart"), blend: f.get("blend") };
        f.finish();
        Ok(node)
    }
}

/// 碰撞，逐字段解析。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CollisionNode {
    pub ground: Option<f32>,
    pub scene: Option<bool>,
    pub bounce: Option<f32>,
    pub friction: Option<f32>,
    pub kill: Option<bool>,
    pub radius: Option<f32>,
}

impl<'de> Deserialize<'de> for CollisionNode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut f = Fields::new("emitters3d.collision", Value::deserialize(d)?).map_err(serde::de::Error::custom)?;
        let node = Self { ground: f.get("ground"), scene: f.get("scene"), bounce: f.get("bounce"), friction: f.get("friction"), kill: f.get("kill"), radius: f.get("radius") };
        f.finish();
        Ok(node)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubTriggerNode {
    Birth,
    Death,
    Collision,
}

/// 子发射器：`on` 和 `count` 写错时这一项跳过，`emitter` 和发射器节点一样解析（不用 `id`、位置）。
#[derive(Debug, Clone, PartialEq)]
pub struct SubEmitterNode {
    pub on: SubTriggerNode,
    pub count: u32,
    pub inherit: Option<f32>,
    pub emitter: Box<Emitter3dNode>,
}

impl<'de> Deserialize<'de> for SubEmitterNode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut f = Fields::new("emitters3d.subEmitters", Value::deserialize(d)?).map_err(serde::de::Error::custom)?;
        fn required<T>(v: Result<Option<T>, String>, key: &str) -> Result<T, String> {
            v.and_then(|v| v.ok_or(format!("要写 {key}")))
        }
        let on = required(f.strict("on"), "on").map_err(serde::de::Error::custom)?;
        let count = required(f.strict("count"), "count").map_err(serde::de::Error::custom)?;
        let emitter = f.strict("emitter").map_err(serde::de::Error::custom)?.unwrap_or_default();
        let node = Self { on, count, inherit: f.get("inherit"), emitter: Box::new(emitter) };
        f.finish();
        Ok(node)
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

fn curve(keys: &Option<Vec<[f32; 2]>>) -> Option<Curve> {
    keys.as_ref().map(|k| Curve::new(k.iter().map(|[t, v]| (*t, *v)))).filter(|c| !c.keys().is_empty())
}

fn shape(node: &ShapeNode, assets: &mut SceneAssets) -> EmitShape {
    match *node {
        ShapeNode::Point => EmitShape::Point,
        ShapeNode::Sphere { radius, surface } => EmitShape::Sphere { radius, surface },
        ShapeNode::Hemisphere { radius, surface } => EmitShape::Hemisphere { radius, surface },
        ShapeNode::Box { size, surface } => EmitShape::Box { size: Vec3::from(size), surface },
        ShapeNode::Circle { radius, inner } => EmitShape::Circle { radius, inner },
        ShapeNode::Cone { radius, angle } => EmitShape::Cone { radius, angle: angle.to_radians() },
        ShapeNode::Mesh { ref model, scale } => {
            let scale = match scale {
                None => Vec3::ONE,
                Some(ScaleNode::Uniform(s)) => Vec3::splat(s),
                Some(ScaleNode::Axes(s)) => Vec3::from(s),
            };
            EmitShape::Mesh(MeshSource::Gltf { gltf: assets.model(model), scale })
        }
    }
}

pub(crate) fn emitter3d(node: &Emitter3dNode, assets: &mut SceneAssets) -> ParticleEmitter3d {
    let d = ParticleEmitter3d::default();
    let facing = match node.facing {
        None | Some(FacingNode::Mode(FacingMode::Camera)) => ParticleFacing::Camera,
        Some(FacingNode::Mode(FacingMode::Velocity)) => ParticleFacing::Velocity { stretch: node.stretch.unwrap_or(0.1) },
        Some(FacingNode::Mode(FacingMode::Horizontal)) => ParticleFacing::Horizontal,
        Some(FacingNode::Fixed([x, y, z])) => ParticleFacing::Fixed(Quat::from_euler(EulerRot::XYZ, x.to_radians(), y.to_radians(), z.to_radians())),
    };
    let sheet = node.sheet.map(|s| SpriteSheet {
        columns: s.grid[0],
        rows: s.grid[1],
        frames: s.frames.unwrap_or(0),
        mode: s.fps.map_or(SheetMode::Lifetime, SheetMode::Fps),
        random_start: s.random_start.unwrap_or(false),
        blend: s.blend.unwrap_or(false),
    });
    let collision = node.collision.map(|c| {
        let d = ParticleCollision::default();
        ParticleCollision {
            ground: c.ground,
            scene: c.scene.unwrap_or(d.scene),
            bounce: c.bounce.unwrap_or(d.bounce),
            friction: c.friction.unwrap_or(d.friction),
            kill: c.kill.unwrap_or(d.kill),
            radius: c.radius.unwrap_or(d.radius),
        }
    });
    let sub_emitters = node
        .sub_emitters
        .iter()
        .map(|s| SubEmitter3d {
            on: match s.on {
                SubTriggerNode::Birth => SubTrigger::Birth,
                SubTriggerNode::Death => SubTrigger::Death,
                SubTriggerNode::Collision => SubTrigger::Collision,
            },
            count: s.count,
            inherit: s.inherit.unwrap_or(0.0),
            emitter: emitter3d(&s.emitter, assets),
        })
        .collect();
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
        angle: node.angle.map(|[a, b]| (a.to_radians(), b.to_radians())),
        shape: node.shape.as_ref().map_or(EmitShape::Point, |s| shape(s, assets)),
        emit_direction: match node.emit_direction {
            None | Some(EmitDirectionNode::Cone) => EmitDirection::Cone,
            Some(EmitDirectionNode::Normal) => EmitDirection::Normal,
            Some(EmitDirectionNode::Random) => EmitDirection::Random,
        },
        size_curve: curve(&node.size_curve),
        alpha_curve: curve(&node.alpha_curve),
        speed_curve: curve(&node.speed_curve),
        color_gradient: node.color_gradient.as_ref().map(|g| Gradient::new(g.iter().map(|(t, c)| (*t, c.0)))).filter(|g| !g.keys().is_empty()),
        drag: node.drag.unwrap_or(0.0),
        noise: node.noise.map(|n| Noise3d { strength: n.strength, frequency: n.frequency, scroll: n.scroll }),
        facing,
        sheet,
        soft: node.soft.unwrap_or(0.0),
        lit: node.lit.unwrap_or(false),
        collision,
        sub_emitters,
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
    use nge_particles::{EmitDirection, EmitShape, Noise3d, ParticleCollision, ParticleFacing, SheetMode, SpriteSheet, SubTrigger};
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
    fn new_emitter_fields_parse_into_the_emitter() {
        let mut app = app();
        let node = json!({
            "id": "all", "rate": 5, "lifetime": [10, 10],
            "shape": { "kind": "cone", "radius": 2, "angle": 30 },
            "emitDirection": "normal",
            "sizeCurve": [[0, 1], [1, 3]], "alphaCurve": [[0, 1], [1, 0]], "speedCurve": [[0, 2]],
            "colorGradient": [[0, "#ff0000ff"], [1, "#0000ff00"]],
            "drag": 1.5, "noise": { "strength": 4, "frequency": 0.2 },
            "facing": "velocity", "stretch": 0.3,
            "sheet": { "grid": [4, 2], "frames": 6, "fps": 12, "randomStart": true, "blend": true },
            "soft": 8, "lit": true, "angle": [0, 90],
            "collision": { "ground": -5, "bounce": 0.2, "friction": 0.1, "kill": true, "radius": 1 },
            "subEmitters": [{ "on": "collision", "count": 4, "inherit": 0.5, "emitter": { "lifetime": [0.5, 0.5], "facing": "horizontal" } }],
        });
        frame(&mut app, Some(json!({ "emitters3d": [node] })));
        let all = entity(&mut app, "all").unwrap();
        let e = app.world().get::<ParticleEmitter3d>(all).unwrap().clone();
        assert_eq!(e.shape, EmitShape::Cone { radius: 2.0, angle: 30f32.to_radians() });
        assert_eq!(e.emit_direction, EmitDirection::Normal);
        assert_eq!((e.size_curve.unwrap().eval(0.5), e.alpha_curve.unwrap().eval(1.0), e.speed_curve.unwrap().eval(0.3)), (Some(2.0), Some(0.0), Some(2.0)));
        assert_eq!(e.color_gradient.unwrap().eval(1.0), Some(Vec4::new(0.0, 0.0, 1.0, 0.0)));
        assert_eq!((e.drag, e.noise), (1.5, Some(Noise3d { strength: 4.0, frequency: 0.2, scroll: 0.0 })));
        assert_eq!(e.facing, ParticleFacing::Velocity { stretch: 0.3 });
        assert_eq!(e.sheet, Some(SpriteSheet { columns: 4, rows: 2, frames: 6, mode: SheetMode::Fps(12.0), random_start: true, blend: true }));
        assert_eq!((e.soft, e.lit, e.angle), (8.0, true, Some((0.0, std::f32::consts::FRAC_PI_2))));
        assert_eq!(e.collision, Some(ParticleCollision { ground: Some(-5.0), scene: false, bounce: 0.2, friction: 0.1, kill: true, radius: 1.0 }));
        let sub = &e.sub_emitters[0];
        assert_eq!((sub.on, sub.count, sub.inherit, sub.emitter.lifetime, sub.emitter.facing), (SubTrigger::Collision, 4, 0.5, (0.5, 0.5), ParticleFacing::Horizontal));
        // 固定朝向是欧拉角度；网格形状认 glTF
        let fixed: Emitter3dNode = serde_json::from_value(json!({ "facing": [90, 0, 0], "shape": { "kind": "mesh", "model": "asset:/@g/a.glb", "scale": 2 } })).unwrap();
        let mut assets_state: bevy_ecs::system::SystemState<SceneAssets> = bevy_ecs::system::SystemState::new(app.world_mut());
        let mut assets = assets_state.get_mut(app.world_mut()).unwrap();
        let fixed = emitter3d(&fixed, &mut assets);
        let ParticleFacing::Fixed(q) = fixed.facing else { panic!("{:?}", fixed.facing) };
        assert!((q * Vec3::Z - Vec3::NEG_Y).length() < 1e-5, "绕 x 转 90°：+Z 转到 -Y");
        assert!(matches!(fixed.shape, EmitShape::Mesh(nge_particles::MeshSource::Gltf { scale, .. }) if scale == Vec3::splat(2.0)));
    }

    #[test]
    fn a_bad_new_field_drops_alone() {
        let node: Emitter3dNode = serde_json::from_value(json!({
            "id": "x", "rate": 7, "soft": "very", "shape": { "kind": "blob" }, "sofft": 3, "lit": true,
            "sheet": { "grid": [2, 2], "fps": "fast" },
            "subEmitters": [{ "on": "boom", "count": 1, "emitter": {} }, { "on": "death", "count": 2, "emitter": { "blend": "add" } }, { "on": "death", "count": 2, "emitter": { "blend": "nope" } }],
        }))
        .unwrap();
        assert_eq!((node.rate, node.soft, node.shape.is_none(), node.lit), (7.0, None, true, Some(true)), "写错的只丢那一项");
        assert_eq!(node.sheet.map(|s| (s.grid, s.fps)), Some(([2, 2], None)), "sheet 里也逐字段");
        assert_eq!(node.sub_emitters.len(), 1, "子发射器逐项");
        assert_eq!(node.sub_emitters[0].emitter.blend, BlendNode::Add);
        assert!(serde_json::from_value::<Emitter3dNode>(json!({ "id": "x", "rate": "fast" })).is_err(), "原来的字段照旧严格");
    }

    #[test]
    fn sub_emitter_particles_survive_tuning() {
        let mut app = app();
        let scene = |drag: f32| {
            json!({ "emitters3d": [{ "id": "fw", "rate": 20, "lifetime": [0.05, 0.05], "drag": drag,
                "subEmitters": [{ "on": "death", "count": 3, "emitter": { "lifetime": [100, 100] } }] }] })
        };
        frame(&mut app, Some(scene(0.0)));
        for _ in 0..6 {
            frame(&mut app, None);
        }
        let fw = entity(&mut app, "fw").unwrap();
        let alive = |app: &nge_ecs::App| app.world().get::<Particles3d>(fw).unwrap().len();
        let before = alive(&app);
        assert!(before > app.world().get::<Particles3d>(fw).unwrap().iter().count(), "有子发射器的粒子");
        frame(&mut app, Some(scene(2.0)));
        assert!(alive(&app) >= before, "只改参数时子发射器的粒子也留着");
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
