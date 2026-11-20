//! 权威模式线上的输入和快照。快照是增量的：和客户端确认收到过的某个 tick（基准）比，只发变了的字段；
//! 没变的角色整个不发，基准里有、现在没有的列进 `removed`。没有基准（刚进来、基准太旧）时发全量。

use serde::{Deserialize, Serialize};

use super::{Hero, World, button};
use crate::proto::{MAX_COORD, PeerId};

/// 一条输入消息最多带几个 tick 的输入（新的在后，丢包时靠后面几条补上）。
pub const MAX_INPUTS: usize = 32;
/// 一个快照最多几个角色、几个删除。
pub const MAX_ENTITIES: usize = 64;
/// 速度的绝对值上限。
pub const MAX_SPEED: f32 = 1.0e4;

/// 客户端发给服务端：从 tick `first` 起连续几个 tick 的按键位。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputMsg {
    pub first: u32,
    pub inputs: Vec<u8>,
    /// 收到并解出来的最新快照的 tick：服务端拿它当下一个快照的基准。
    pub ack: Option<u32>,
    /// 远端角色正显示到哪个 tick（插值落后一点）：服务端判拍人时回溯到这里。
    pub view: u32,
}

/// 一个字段的新值。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Field {
    X(f32),
    Y(f32),
    Vx(f32),
    Vy(f32),
    Ground(bool),
    Cool(u8),
    Tagged(u16),
}

/// 字段的种类数：一个角色最多这么多条。
const FIELDS: usize = 7;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityDelta {
    pub id: PeerId,
    pub fields: Vec<Field>,
}

/// 服务端发给一个客户端的快照。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldSnapshot {
    pub tick: u32,
    /// 和哪个 tick 比的；`None` 是全量。
    pub base: Option<u32>,
    pub entities: Vec<EntityDelta>,
    pub removed: Vec<PeerId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaError {
    /// 基准里没有、又不是全量的角色。
    MissingBase(PeerId),
    /// 同一个角色出现了两次。
    Duplicate(PeerId),
}

fn fields(h: &Hero) -> [Field; FIELDS] {
    [Field::X(h.x), Field::Y(h.y), Field::Vx(h.vx), Field::Vy(h.vy), Field::Ground(h.ground), Field::Cool(h.cool), Field::Tagged(h.tagged)]
}

fn set(h: &mut Hero, f: Field) {
    match f {
        Field::X(v) => h.x = v,
        Field::Y(v) => h.y = v,
        Field::Vx(v) => h.vx = v,
        Field::Vy(v) => h.vy = v,
        Field::Ground(v) => h.ground = v,
        Field::Cool(v) => h.cool = v,
        Field::Tagged(v) => h.tagged = v,
    }
}

/// 第几种字段（判重用）。
fn kind(f: &Field) -> usize {
    match f {
        Field::X(_) => 0,
        Field::Y(_) => 1,
        Field::Vx(_) => 2,
        Field::Vy(_) => 3,
        Field::Ground(_) => 4,
        Field::Cool(_) => 5,
        Field::Tagged(_) => 6,
    }
}

/// 现在的 `world` 和 `base` 比：变了的角色只带变了的字段（按位比，-0.0 和 0.0 算不一样）。
pub fn diff(base: Option<&World>, world: &World) -> (Vec<EntityDelta>, Vec<PeerId>) {
    let mut entities = Vec::new();
    for (&id, hero) in world {
        let now = fields(hero);
        let changed: Vec<Field> = match base.and_then(|b| b.get(&id)) {
            Some(old) => now.iter().zip(fields(old)).filter(|(a, b)| !same(a, b)).map(|(a, _)| *a).collect(),
            None => now.to_vec(),
        };
        if !changed.is_empty() {
            entities.push(EntityDelta { id, fields: changed });
        }
    }
    let removed = base.map(|b| b.keys().filter(|id| !world.contains_key(id)).copied().collect()).unwrap_or_default();
    (entities, removed)
}

fn same(a: &Field, b: &Field) -> bool {
    match (a, b) {
        (Field::X(a), Field::X(b)) | (Field::Y(a), Field::Y(b)) | (Field::Vx(a), Field::Vx(b)) | (Field::Vy(a), Field::Vy(b)) => a.to_bits() == b.to_bits(),
        _ => a == b,
    }
}

/// 在 `base` 上套用增量，得到完整的世界。新出现的角色（基准里没有）必须带全所有字段。
pub fn apply(base: Option<&World>, entities: &[EntityDelta], removed: &[PeerId]) -> Result<World, DeltaError> {
    let mut world = base.cloned().unwrap_or_default();
    for id in removed {
        world.remove(id);
    }
    let mut seen = std::collections::BTreeSet::new();
    for e in entities {
        if !seen.insert(e.id) {
            return Err(DeltaError::Duplicate(e.id));
        }
        let mut hero = match world.get(&e.id) {
            Some(h) => *h,
            None if e.fields.len() == FIELDS => Hero { x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, ground: false, cool: 0, tagged: 0 },
            None => return Err(DeltaError::MissingBase(e.id)),
        };
        for f in &e.fields {
            set(&mut hero, *f);
        }
        world.insert(e.id, hero);
    }
    Ok(world)
}

/// 解出来的快照能不能用：数是有限的、在范围里，条目不超过上限，一个角色的字段不重复。
pub fn validate_snapshot(s: &WorldSnapshot) -> Result<(), &'static str> {
    if s.entities.len() > MAX_ENTITIES || s.removed.len() > MAX_ENTITIES {
        return Err("快照的条目太多");
    }
    if s.base.is_some_and(|b| b >= s.tick) {
        return Err("快照的基准不比它旧");
    }
    for e in &s.entities {
        if e.fields.is_empty() || e.fields.len() > FIELDS {
            return Err("角色的字段数不对");
        }
        let mut kinds = [false; FIELDS];
        for f in &e.fields {
            let k = kind(f);
            if std::mem::replace(&mut kinds[k], true) {
                return Err("角色的字段重复");
            }
            let ok = match *f {
                Field::X(v) | Field::Y(v) => v.is_finite() && v.abs() <= MAX_COORD,
                Field::Vx(v) | Field::Vy(v) => v.is_finite() && v.abs() <= MAX_SPEED,
                _ => true,
            };
            if !ok {
                return Err("坐标或速度不是有限数、或者超出范围");
            }
        }
    }
    Ok(())
}

pub fn validate_input(m: &InputMsg) -> Result<(), &'static str> {
    if m.inputs.is_empty() || m.inputs.len() > MAX_INPUTS {
        return Err("输入条数不对");
    }
    if m.inputs.iter().any(|i| i & !button::ALL != 0) {
        return Err("输入里有不认得的位");
    }
    if m.first.checked_add(m.inputs.len() as u32).is_none() {
        return Err("tick 溢出");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::spawn;
    use crate::proto::{RelayMsg, decode_relay, encode};

    fn world(heroes: &[(PeerId, Hero)]) -> World {
        heroes.iter().copied().collect()
    }

    #[test]
    fn unchanged_heroes_are_not_sent_and_changed_ones_carry_only_what_changed() {
        let base = world(&[(1, spawn(1)), (2, spawn(2)), (3, spawn(3))]);
        let mut now = base.clone();
        now.get_mut(&1).unwrap().x += 5.0;
        now.get_mut(&2).unwrap().tagged = 1;
        now.remove(&3);
        now.insert(4, spawn(4));
        let (entities, removed) = diff(Some(&base), &now);
        assert_eq!(entities, vec![EntityDelta { id: 1, fields: vec![Field::X(now[&1].x)] }, EntityDelta { id: 2, fields: vec![Field::Tagged(1)] }, EntityDelta { id: 4, fields: fields(&spawn(4)).to_vec() }]);
        assert_eq!(removed, vec![3]);
        assert_eq!(apply(Some(&base), &entities, &removed), Ok(now.clone()));
        // 全量
        let (full, gone) = diff(None, &now);
        assert!(gone.is_empty() && full.iter().all(|e| e.fields.len() == FIELDS));
        assert_eq!(apply(None, &full, &[]), Ok(now));
    }

    #[test]
    fn a_still_world_is_a_tiny_snapshot() {
        let w: World = (1..=8).map(|id| (id, spawn(id))).collect();
        let (entities, removed) = diff(Some(&w), &w);
        assert!(entities.is_empty() && removed.is_empty());
        let still = encode(&RelayMsg::World(WorldSnapshot { tick: 100, base: Some(99), entities, removed }));
        let (full, _) = diff(None, &w);
        let whole = encode(&RelayMsg::World(WorldSnapshot { tick: 100, base: None, entities: full, removed: vec![] }));
        assert!(still.len() < 12 && whole.len() > 8 * 20, "增量 {} 字节，全量 {} 字节", still.len(), whole.len());
    }

    #[test]
    fn deltas_need_their_base() {
        let partial = [EntityDelta { id: 9, fields: vec![Field::X(1.0)] }];
        assert_eq!(apply(None, &partial, &[]), Err(DeltaError::MissingBase(9)));
        let twice = [EntityDelta { id: 1, fields: fields(&spawn(1)).to_vec() }, EntityDelta { id: 1, fields: fields(&spawn(1)).to_vec() }];
        assert_eq!(apply(None, &twice, &[]), Err(DeltaError::Duplicate(1)));
    }

    fn snap(entities: Vec<EntityDelta>) -> WorldSnapshot {
        WorldSnapshot { tick: 5, base: None, entities, removed: vec![] }
    }

    #[test]
    fn bad_snapshots_are_rejected() {
        for bad in [f32::NAN, f32::INFINITY, -f32::INFINITY, MAX_COORD * 2.0] {
            let s = snap(vec![EntityDelta { id: 1, fields: vec![Field::X(bad)] }]);
            assert!(validate_snapshot(&s).is_err(), "{bad}");
            assert!(decode_relay(&encode(&RelayMsg::World(s)), 1 << 16).is_err(), "解码时就拦下 {bad}");
        }
        assert!(validate_snapshot(&snap(vec![EntityDelta { id: 1, fields: vec![Field::Vy(f32::NAN)] }])).is_err());
        assert!(validate_snapshot(&snap(vec![EntityDelta { id: 1, fields: vec![Field::X(1.0), Field::X(2.0)] }])).is_err(), "字段重复");
        assert!(validate_snapshot(&snap(vec![EntityDelta { id: 1, fields: vec![] }])).is_err(), "空的");
        assert!(validate_snapshot(&snap(vec![EntityDelta { id: 1, fields: vec![Field::Cool(1); FIELDS + 1] }])).is_err(), "太长");
        let many = (0..=MAX_ENTITIES as u32).map(|id| EntityDelta { id, fields: vec![Field::Cool(0)] }).collect();
        assert!(validate_snapshot(&snap(many)).is_err(), "条目太多");
        let removed = WorldSnapshot { removed: vec![1; MAX_ENTITIES + 1], ..snap(vec![]) };
        assert!(validate_snapshot(&removed).is_err());
        assert!(validate_snapshot(&WorldSnapshot { base: Some(5), ..snap(vec![]) }).is_err(), "基准不比自己旧");
        assert!(validate_snapshot(&snap(vec![EntityDelta { id: 1, fields: fields(&spawn(1)).to_vec() }])).is_ok());
        // 超长的帧不解
        assert!(decode_relay(&encode(&RelayMsg::World(snap(vec![]))), 3).is_err());
    }

    #[test]
    fn bad_inputs_are_rejected() {
        let ok = InputMsg { first: 1, inputs: vec![button::ALL], ack: None, view: 0 };
        assert!(validate_input(&ok).is_ok());
        assert!(validate_input(&InputMsg { inputs: vec![], ..ok.clone() }).is_err());
        assert!(validate_input(&InputMsg { inputs: vec![0; MAX_INPUTS + 1], ..ok.clone() }).is_err());
        assert!(validate_input(&InputMsg { inputs: vec![16], ..ok.clone() }).is_err(), "不认得的位");
        assert!(validate_input(&InputMsg { first: u32::MAX, inputs: vec![0, 0], ..ok }).is_err(), "溢出");
    }
}
