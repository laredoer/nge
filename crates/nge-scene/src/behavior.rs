//! 精灵的 `behavior`：行为树，叶子是看见、走近、按地面寻路追 / 走到某处、读写黑板、发事件（见 docs/gameplay2d.md）。
//! 走的是 `kind: "character"` 的刚体：每帧写它的水平速度，跳的时候写竖直速度，从单向平台穿下去用 `drop_through`。

#![cfg_attr(not(feature = "ai"), allow(dead_code))]

use std::collections::HashMap;
use std::sync::Arc;

use bevy_ecs::prelude::*;
use glam::Vec2;
use nge_ai::{Blackboard, GroundGraph, GroundMap, Move, NavGrid, Node, Reach, Status, Tree, Value, Walker};
use nge_core::{CommandKey, EventKey};
use nge_ecs::{Outbox, Time};
use nge_physics2d::{Character, Physics, Velocity};
use nge_render2d::Transform2d;
use serde::{Deserialize, Serialize};

use crate::protocol::SpriteNode;
use crate::sync::SceneId;

pub const BEHAVIOR: EventKey<BehaviorEvent> = EventKey::new("behavior");
pub const BLACKBOARD: CommandKey<BlackboardSet> = CommandKey::new("blackboard");

/// `behavior`：树、黑板的初值、走多快（像素 / 秒）、起跳速度（像素 / 秒，0 不会跳）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BehaviorNode {
    pub tree: BtNode,
    #[serde(default)]
    pub blackboard: HashMap<String, serde_json::Value>,
    #[serde(default = "default_speed")]
    pub speed: f32,
    #[serde(default)]
    pub jump: f32,
}

fn default_speed() -> f32 {
    120.0
}

/// 树上的节点，`type` 是名字。前面几种是组合和装饰，后面的是叶子。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum BtNode {
    Sequence {
        children: Vec<BtNode>,
        #[serde(default)]
        reactive: bool,
    },
    Selector {
        children: Vec<BtNode>,
        #[serde(default)]
        reactive: bool,
    },
    Parallel {
        succeed: usize,
        children: Vec<BtNode>,
    },
    Invert {
        child: Box<BtNode>,
    },
    Repeat {
        times: Option<u32>,
        child: Box<BtNode>,
    },
    Cooldown {
        seconds: f32,
        child: Box<BtNode>,
    },
    Wait {
        seconds: f32,
    },
    /// 目标在 `range` 以内，中间没有墙挡着（有导航网格时查）。
    See {
        target: String,
        range: f32,
    },
    /// 目标在 `range` 以内（不管墙）。
    Near {
        target: String,
        range: f32,
    },
    /// 黑板上的 `key` 是真的。
    Check {
        key: String,
    },
    /// 往黑板上写（`null` 是删掉）。
    Set {
        key: String,
        value: serde_json::Value,
    },
    /// 沿地面走 / 跳 / 落去追目标：追到是成，走不到是失败，路上是在跑。
    Chase {
        target: String,
    },
    /// 走到某个点。
    Goto {
        x: f32,
        y: f32,
    },
    /// 走回出生的地方（声明的 x、y）。
    Home,
    /// 发一次 `behavior` 事件。
    Emit {
        name: String,
    },
}

/// 叶子（组合和装饰交给 `nge_ai::Node`）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Leaf {
    See(String, f32),
    Near(String, f32),
    Check(String),
    Set(String, Option<Value>),
    Chase(String),
    Goto(Vec2),
    Home,
    Emit(String),
}

/// JSON 值换成黑板的值：布尔、数、字符串、`[x, y]`；`null` 是 `None`，别的认不了。
fn value(v: &serde_json::Value) -> Result<Option<Value>, String> {
    Ok(Some(match v {
        serde_json::Value::Null => return Ok(None),
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => Value::Num(n.as_f64().unwrap_or(0.0) as f32),
        serde_json::Value::String(s) => Value::Text(s.clone()),
        serde_json::Value::Array(a) => match a.as_slice() {
            [serde_json::Value::Number(x), serde_json::Value::Number(y)] => Value::Pos(Vec2::new(x.as_f64().unwrap_or(0.0) as f32, y.as_f64().unwrap_or(0.0) as f32)),
            _ => return Err(format!("{v} 不是 [x, y]")),
        },
        other => return Err(format!("{other} 不能放进黑板")),
    }))
}

fn non_negative(name: &str, v: f32) -> Result<f32, String> {
    if v.is_finite() && v >= 0.0 { Ok(v) } else { Err(format!("{name} 要是不小于 0 的数，收到 {v}")) }
}

impl BtNode {
    /// 能跑但多半写错了的地方（比如并行要的成功数比孩子还多，永远成不了）。
    pub(crate) fn lint(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.lint_into(&mut out);
        out
    }

    fn lint_into(&self, out: &mut Vec<String>) {
        match self {
            BtNode::Parallel { succeed, children } if *succeed > children.len() => {
                out.push(format!("parallel 的 succeed 是 {succeed}，可只有 {} 个孩子，永远成不了", children.len()));
            }
            _ => {}
        }
        match self {
            BtNode::Sequence { children, .. } | BtNode::Selector { children, .. } | BtNode::Parallel { children, .. } => children.iter().for_each(|c| c.lint_into(out)),
            BtNode::Invert { child } | BtNode::Repeat { child, .. } | BtNode::Cooldown { child, .. } => child.lint_into(out),
            _ => {}
        }
    }

    pub(crate) fn build(&self) -> Result<Node<Leaf>, String> {
        let all = |c: &[BtNode]| c.iter().map(BtNode::build).collect::<Result<Vec<_>, _>>();
        Ok(match self {
            BtNode::Sequence { children, reactive } => {
                let n = Node::sequence(all(children)?);
                if *reactive { n.reactive() } else { n }
            }
            BtNode::Selector { children, reactive } => {
                let n = Node::selector(all(children)?);
                if *reactive { n.reactive() } else { n }
            }
            BtNode::Parallel { succeed, children } => Node::parallel(*succeed, all(children)?),
            BtNode::Invert { child } => Node::invert(child.build()?),
            BtNode::Repeat { times, child } => Node::repeat(*times, child.build()?),
            BtNode::Cooldown { seconds, child } => Node::cooldown(non_negative("cooldown 的 seconds", *seconds)?, child.build()?),
            BtNode::Wait { seconds } => Node::wait(non_negative("wait 的 seconds", *seconds)?),
            BtNode::See { target, range } => Node::leaf(Leaf::See(target.clone(), non_negative("see 的 range", *range)?)),
            BtNode::Near { target, range } => Node::leaf(Leaf::Near(target.clone(), non_negative("near 的 range", *range)?)),
            BtNode::Check { key } => Node::leaf(Leaf::Check(key.clone())),
            BtNode::Set { key, value: v } => Node::leaf(Leaf::Set(key.clone(), value(v)?)),
            BtNode::Chase { target } => Node::leaf(Leaf::Chase(target.clone())),
            BtNode::Goto { x, y } if x.is_finite() && y.is_finite() => Node::leaf(Leaf::Goto(Vec2::new(*x, *y))),
            BtNode::Goto { .. } => return Err("goto 的 x、y 要是数".into()),
            BtNode::Home => Node::leaf(Leaf::Home),
            BtNode::Emit { name } => Node::leaf(Leaf::Emit(name.clone())),
        })
    }
}

/// `behavior` 事件：树里的 `emit`。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BehaviorEvent {
    pub id: String,
    pub name: String,
    pub x: f32,
    pub y: f32,
}

/// TSX 的 `blackboard` command：往某个节点的黑板上写（`value` 是 `null` 时删掉；没写 `value` 时 warn，不改）。
#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlackboardSet {
    pub id: String,
    pub key: String,
    #[serde(default, deserialize_with = "present")]
    pub value: Option<serde_json::Value>,
}

/// 写了（哪怕是 `null`）就是 `Some`。
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<serde_json::Value>, D::Error> {
    serde_json::Value::deserialize(d).map(Some)
}

/// 跳在半空：横着的速度，往哪落（世界坐标的 x）。走出边落下时速度是 `None`，一路往落点挪。
/// `seen`：起跳以后离过地（物理这一帧可能还没动，接触还是旧的）；`since`：起跳的时刻。
#[derive(Debug, Clone, Copy, PartialEq)]
struct Air {
    vx: Option<f32>,
    to: f32,
    seen: bool,
    since: f64,
}

/// 地面寻路的一条连线（起点格、终点格），按格子记：地图重建以后编号会变。
type LinkCells = (nge_ai::Cell, nge_ai::Cell);

/// 组件：跑着的行为树和走路的状态。
#[derive(Component)]
pub(crate) struct Agent {
    tree: Tree<Leaf>,
    speed: f32,
    jump: f32,
    home: Vec2,
    /// 半宽、半高（point）。
    size: Vec2,
    air: Option<Air>,
    /// 正在试的跳 / 穿下去的连线。
    attempt: Option<LinkCells>,
    /// 每条连线接连失败了几次。
    fails: HashMap<LinkCells, u32>,
    /// 失败太多次、暂时不走的连线，到什么时候。
    banned: Vec<(LinkCells, f64)>,
    /// 这一帧放弃了一条连线：移动的叶子失败一次，树换别的做。
    gave_up: bool,
    clock: f64,
}

impl Agent {
    fn new(tree: Tree<Leaf>, speed: f32, jump: f32, home: Vec2, size: Vec2) -> Self {
        Self { tree, speed, jump, home, size, air: None, attempt: None, fails: HashMap::new(), banned: Vec::new(), gave_up: false, clock: 0.0 }
    }

    /// 试的连线有了结果：成了清掉失败次数；失败了记一次，够数就暂时不走它。
    fn settle(&mut self, link: LinkCells, ok: bool) {
        if ok {
            self.fails.remove(&link);
            return;
        }
        let n = self.fails.entry(link).or_default();
        *n += 1;
        if *n >= GIVE_UP {
            self.fails.remove(&link);
            self.banned.push((link, self.clock + BAN));
            self.gave_up = true;
        }
    }
}

/// 行为树变了才重建（树从头跑、黑板回到初值）；只是位置变了只换出生点。去掉就删。
pub(crate) fn apply_behavior(entity: &mut EntityCommands, before: Option<&SpriteNode>, node: &SpriteNode) {
    let home = Vec2::new(node.x, node.y);
    if before.is_some_and(|b| b.behavior == node.behavior) {
        entity.queue(move |mut e: EntityWorldMut| {
            if let Some(mut a) = e.get_mut::<Agent>() {
                a.home = home;
            }
        });
        return;
    }
    let Some(b) = &node.behavior else {
        entity.remove::<Agent>();
        return;
    };
    let built = b.tree.build().and_then(|root| {
        let mut tree = Tree::new(root);
        for (k, v) in &b.blackboard {
            if let Some(v) = value(v)? {
                tree.blackboard.set(k, v);
            }
        }
        Ok(tree)
    });
    let (Ok(speed), Ok(jump)) = (non_negative("speed", b.speed), non_negative("jump", b.jump)) else {
        crate::lenient::warn_once(format!("节点 {:?} 的 behavior 里 speed / jump 要是不小于 0 的数，没有行为", node.id));
        entity.remove::<Agent>();
        return;
    };
    for w in b.tree.lint() {
        crate::lenient::warn_once(format!("节点 {:?} 的 behavior：{w}", node.id));
    }
    match built {
        Ok(tree) => {
            let size = crate::physics::square(node.w, node.h).map_or(Vec2::ZERO, |s| s.abs() / 2.0);
            entity.insert(Agent::new(tree, speed, jump, home, size));
        }
        Err(e) => {
            crate::lenient::warn_once(format!("节点 {:?} 的 behavior 写错了，没有行为：{e}", node.id));
            entity.remove::<Agent>();
        }
    }
}

/// 行为树和 `blackboard` command 的系统（feature `ai`）。
#[cfg(feature = "ai")]
pub(crate) fn register(app: &mut nge_ecs::App) {
    use nge_ecs::Update;
    app.add_event(BEHAVIOR).add_command(BLACKBOARD).add_systems(Update, (blackboard, run).chain());
}

pub(crate) fn blackboard(mut reader: MessageReader<BlackboardSet>, mut agents: Query<(&SceneId, &mut Agent)>) {
    for c in reader.read() {
        let Some((_, mut agent)) = agents.iter_mut().find(|(id, _)| id.0 == c.id) else {
            log::warn!("blackboard：没有 id 为 {:?} 的节点，或它没有 behavior", c.id);
            continue;
        };
        let Some(v) = &c.value else {
            log::warn!("blackboard：{:?} 的 {:?} 没写 value，没改（要删掉写 null）", c.id, c.key);
            continue;
        };
        match value(v) {
            Ok(Some(v)) => agent.tree.blackboard.set(&c.key, v),
            Ok(None) => {
                agent.tree.blackboard.remove(&c.key);
            }
            Err(e) => log::warn!("blackboard：{:?} 的 {:?} {e}", c.id, c.key),
        }
    }
}


/// 追到的距离：横着差多少格以内、竖着差多少格以内。
const ARRIVE: f32 = 0.4;
const ARRIVE_Y: f32 = 1.0;
/// 跳之前离起跳点多近（格）。
const TAKEOFF: f32 = 0.15;
/// 跳跃能力打的折扣：建图按稍小的能力，跳得到的一定够得着。
const MARGIN: f32 = 0.85;
/// 落地多深以内找脚下的地面（格）。
const DEPTH: i32 = 64;
/// 一条跳 / 穿下去的连线接连失败几次就暂时不走，不走多少秒。
const GIVE_UP: u32 = 3;
const BAN: f64 = 10.0;
/// 起跳以后多久还没离地算没跳起来（秒）。
const LIFTOFF: f64 = 1.0;
/// 比所有地图的底边还低多少格算掉出地图。
const OUT: f32 = 2.0;
/// 掉出地图时发的 `behavior` 事件名。
pub const FELL: &str = "fell";

/// 跑一帧：每个有行为树的跑一遍树，叶子定下要去哪，再按地面寻路的下一步写速度。
#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    time: Res<Time>,
    mut physics: ResMut<Physics>,
    mut outbox: ResMut<Outbox>,
    mut commands: Commands,
    ids: Query<(&SceneId, &Transform2d)>,
    maps: Query<(Entity, Ref<GroundMap>)>,
    navs: Query<&NavGrid>,
    mut agents: Query<(Entity, &SceneId, &Transform2d, &mut Agent, Option<&mut Velocity>, Option<&Character>)>,
    mut graphs: Local<crate::ground_graphs::GroundGraphs>,
) {
    if agents.is_empty() {
        return;
    }
    let dt = time.delta;
    let changed: Vec<Entity> = maps.iter().filter(|(_, m)| m.is_changed()).map(|(e, _)| e).collect();
    // 地图改了先用旧图，后台 / 节流重建（见 docs/big-scene-perf.md）
    graphs.begin(|e| maps.contains(e), &changed);
    let now = time.elapsed;
    let targets: HashMap<&str, Vec2> = ids.iter().map(|(id, t)| (id.0.as_str(), t.translation)).collect();
    let gravity = physics.gravity().y.abs();
    // 所有地图最低的底边：比它还低就是掉出去了
    let floor = maps.iter().map(|(_, m)| m.bottom() - OUT * m.tile).reduce(f32::min);
    for (entity, id, transform, mut agent, velocity, character) in &mut agents {
        let Some(mut velocity) = velocity else {
            crate::lenient::warn_once(format!("节点 {:?} 有 behavior 但没有 kind 是 character 的 body，不会走", id.0));
            continue;
        };
        agent.clock += f64::from(dt);
        let clock = agent.clock;
        agent.banned.retain(|&(_, until)| until > clock);
        let me = transform.translation;
        if floor.is_some_and(|f| me.y < f) {
            // 掉出地图：说一声，回到出生点从头来
            outbox.send(BEHAVIOR, &BehaviorEvent { id: id.0.clone(), name: FELL.into(), x: me.x, y: me.y });
            commands.entity(entity).insert(Transform2d { translation: agent.home, ..*transform });
            velocity.linear = Vec2::ZERO;
            (agent.air, agent.attempt) = (None, None);
            agent.tree.halt();
            continue;
        }
        let grounded = physics.contacts(entity).iter().any(|c| c.normal.y > 0.5);
        let feet = me - Vec2::Y * agent.size.y;
        let max_slope = character.map_or(nge_physics2d::Character::default().max_slope, |c| c.max_slope);
        // 脚下有站的地方的那张图；都没有就用第一张盖住角色的
        let mut graph_of = |e: Entity, m: &GroundMap| {
            let walker = walker(&agent, m.tile, gravity, max_slope);
            let key = (e, [walker.reach.height, walker.reach.distance, walker.half, walker.height, walker.max_slope, m.tile].map(f32::to_bits));
            graphs.get(key, &m.terrain, walker, now)
        };
        let covering: Vec<(&GroundMap, Arc<GroundGraph>)> =
            maps.iter().filter(|(_, m)| m.covers(me)).map(|(e, m)| (m.into_inner(), e)).map(|(m, e)| (m, graph_of(e, m))).collect();
        let picked = covering.iter().position(|(m, g)| g.spot_below(m.local(feet), 2).is_some()).or((!covering.is_empty()).then_some(0));
        let map = picked.map(|i| (covering[i].0, &*covering[i].1));
        landed(&mut agent, grounded, map, feet);
        let nav = navs.iter().find(|n| n.contains(me));
        let banned: Vec<LinkCells> = agent.banned.iter().map(|&(l, _)| l).collect();
        let gave_up = std::mem::take(&mut agent.gave_up);
        let mut walk = Walk { me, feet, grounded, map, banned: &banned, gave_up, goal: None, plan: None };
        let mut emits = Vec::new();
        let home = agent.home;
        agent.tree.tick(dt, &mut |leaf: &Leaf, bb: &mut Blackboard| match leaf {
            Leaf::See(target, range) => Status::from(targets.get(target.as_str()).is_some_and(|&t| {
                t.distance(me) <= *range
                    && match map {
                        // 有地形时按地形看：斜坡格按顶边算，不整格当墙
                        Some((m, _)) if m.covers(t) => m.sees(me, t),
                        _ => nav.is_none_or(|n| n.sees(me, t)),
                    }
            })),
            Leaf::Near(target, range) => Status::from(targets.get(target.as_str()).is_some_and(|&t| t.distance(me) <= *range)),
            Leaf::Check(key) => Status::from(bb.get(key).is_some_and(Value::truthy)),
            Leaf::Set(key, v) => {
                match v {
                    Some(v) => bb.set(key, v.clone()),
                    None => {
                        bb.remove(key);
                    }
                }
                Status::Success
            }
            Leaf::Chase(target) => match targets.get(target.as_str()) {
                Some(&t) => walk.toward(t),
                None => Status::Failure,
            },
            Leaf::Goto(p) => walk.toward(*p),
            Leaf::Home => walk.toward(home),
            Leaf::Emit(name) => {
                emits.push(name.clone());
                Status::Success
            }
        });
        for name in emits {
            outbox.send(BEHAVIOR, &BehaviorEvent { id: id.0.clone(), name, x: me.x, y: me.y });
        }
        let (goal, plan) = (walk.goal, walk.plan);
        steer(entity, &mut agent, &mut velocity, &mut physics, Pose { me, feet, grounded }, gravity, dt, goal, plan, map);
    }
}

/// 角色的跳跃能力和大小换成格（跳跃能力打个折扣）。
fn walker(agent: &Agent, tile: f32, gravity: f32, max_slope: f32) -> Walker {
    let reach = Reach::from_physics(agent.jump, gravity, agent.speed, tile);
    let reach = Reach { height: reach.height * MARGIN, distance: reach.distance * MARGIN };
    let tile = tile.max(1e-3);
    Walker { reach, half: agent.size.x / tile, height: 2.0 * agent.size.y / tile, max_slope }
}

/// 跳起来或穿下去以后落了地：看落没落在要去的地方。
fn landed(agent: &mut Agent, grounded: bool, map: Option<(&GroundMap, &GroundGraph)>, feet: Vec2) {
    let Some(air) = agent.air.as_mut() else { return };
    if !grounded {
        air.seen = true;
        return;
    }
    if !air.seen && agent.clock - air.since < LIFTOFF {
        // 刚起跳：物理还没动
        return;
    }
    agent.air = None;
    let Some(link) = agent.attempt.take() else { return };
    let at = map.and_then(|(m, g)| g.spot_below(m.local(feet), 2).map(|i| g.spots[i].cell));
    agent.settle(link, at == Some(link.1));
}

/// 叶子定下的「这一帧往哪走」：目标点，和站在地上时找好的下一步。最后一个在跑的移动叶子说了算。
struct Walk<'a> {
    me: Vec2,
    feet: Vec2,
    grounded: bool,
    map: Option<(&'a GroundMap, &'a GroundGraph)>,
    banned: &'a [LinkCells],
    gave_up: bool,
    goal: Option<Vec2>,
    plan: Option<Step>,
}

/// 地面寻路的下一步：起点、终点（世界坐标，地面中间），怎么走，跳多高（point），哪条连线。
#[derive(Debug, Clone, Copy)]
struct Step {
    from: Vec2,
    to: Vec2,
    kind: Move,
    height: f32,
    link: LinkCells,
}

impl Walk<'_> {
    /// 去 `p`：到了是成；站在地上、地图里找不到路、或者刚放弃了一条连线是失败；别的是在跑（半空中接着按上一步走）。
    /// 只有在跑时才定下这一帧往哪走：失败、成了的不留下目标。
    fn toward(&mut self, p: Vec2) -> Status {
        let tile = self.map.map_or(32.0, |(m, _)| m.tile);
        let d = p - self.me;
        if d.x.abs() <= ARRIVE * tile && d.y.abs() <= ARRIVE_Y * tile {
            return Status::Success;
        }
        let Some((map, graph)) = self.map else { return self.go(p, None) };
        if !self.grounded {
            return self.go(p, None);
        }
        if self.gave_up {
            return Status::Failure;
        }
        let Some(from) = graph.spot_below(map.local(self.feet), 2) else {
            // 不在站的地方（比如站在不是瓦片的刚体上）：朝目标走，不走出脚下的边
            return self.go(p, None);
        };
        let avoid = |a, b| self.banned.contains(&(a, b));
        let hops = match graph.spot_below(map.local(p), DEPTH) {
            Some(to) => graph.find_avoiding(from, to, &avoid),
            // 目标脚下没有站的地方（悬在坑上、站在移动平台上）：去离它最近、走得到的地方
            None => match graph.find_near(from, map.local(p), &avoid) {
                Some(hops) if hops.is_empty() => {
                    // 已经在最近的地方了：站着等
                    self.goal = None;
                    self.plan = None;
                    return Status::Running;
                }
                hops => hops,
            },
        };
        let Some(hops) = hops else { return Status::Failure };
        let plan = hops.first().map(|h| Step {
            from: map.world(graph.spots[h.from].point()),
            to: map.world(graph.spots[h.to].point()),
            kind: h.kind,
            height: graph.jump_height(h.from, h.to).unwrap_or(0.0) * map.tile,
            link: (graph.spots[h.from].cell, graph.spots[h.to].cell),
        });
        self.go(p, plan)
    }

    fn go(&mut self, p: Vec2, plan: Option<Step>) -> Status {
        self.goal = Some(p);
        self.plan = plan;
        Status::Running
    }
}

/// 这一帧角色在哪：中心、脚底，站没站在地上。
#[derive(Clone, Copy)]
struct Pose {
    me: Vec2,
    feet: Vec2,
    grounded: bool,
}

/// 按目标和下一步写速度：走、到起跳点起跳、从单向平台穿下去；半空中按起跳时算好的速度飞。
#[allow(clippy::too_many_arguments)]
fn steer(
    entity: Entity,
    agent: &mut Agent,
    velocity: &mut Velocity,
    physics: &mut Physics,
    Pose { me, feet, grounded }: Pose,
    gravity: f32,
    dt: f32,
    goal: Option<Vec2>,
    plan: Option<Step>,
    map: Option<(&GroundMap, &GroundGraph)>,
) {
    let speed = agent.speed;
    // 朝 x 走，不冲过头
    let toward = |x: f32| ((x - me.x) / dt.max(1e-3)).clamp(-speed, speed);
    let v = &mut velocity.linear;
    if let Some(air) = agent.air {
        // 半空中（或者刚起跳、还没离地）：按起跳时定的走
        v.x = match air.vx {
            Some(vx) if (air.to - me.x) * vx > 0.0 => vx,
            Some(_) => 0.0,
            None => toward(air.to),
        };
        return;
    }
    if !grounded {
        return;
    }
    let Some(goal) = goal else {
        v.x = 0.0;
        return;
    };
    let clock = agent.clock;
    let Some(step) = plan else {
        v.x = toward(goal.x);
        // 没有找好的路：前面脚下没有地面就不走过去
        if let Some((m, g)) = map {
            let ahead = feet + Vec2::new(v.x.signum() * (agent.size.x + 2.0), 0.0);
            if v.x != 0.0 && g.spot_below(m.local(ahead), 2).is_none() {
                v.x = 0.0;
            }
        }
        return;
    };
    let tile = map.map_or(32.0, |(m, _)| m.tile);
    match step.kind {
        Move::Walk => v.x = toward(step.to.x + (step.to.x - step.from.x).signum() * tile * 0.5),
        Move::Fall => {
            v.x = toward(step.to.x);
            agent.air = Some(Air { vx: None, to: step.to.x, seen: false, since: clock });
        }
        Move::Drop => {
            v.x = 0.0;
            if (me.x - step.from.x).abs() > tile * 0.25 {
                v.x = toward(step.from.x);
            } else if physics.drop_through(entity) {
                agent.air = Some(Air { vx: None, to: step.to.x, seen: false, since: clock });
                agent.attempt = Some(step.link);
            } else {
                // 穿不下去（脚下还踩着实心的）：也算失败一次
                agent.settle(step.link, false);
            }
        }
        Move::Jump => {
            if (me.x - step.from.x).abs() > tile * TAKEOFF {
                v.x = toward(step.from.x);
                return;
            }
            let g = gravity.max(1e-3);
            let vy = (2.0 * g * step.height).sqrt().min(agent.jump);
            let dy = step.to.y - step.from.y;
            let t = (vy + (vy * vy - 2.0 * g * dy).max(0.0).sqrt()) / g;
            let vx = ((step.to.x - me.x) / t.max(1e-3)).clamp(-speed, speed);
            *v = Vec2::new(vx, vy);
            agent.air = Some(Air { vx: Some(vx), to: step.to.x, seen: false, since: clock });
            agent.attempt = Some(step.link);
        }
    }
}

#[cfg(all(test, feature = "ai"))]
#[path = "behavior_tests.rs"]
mod tests;
