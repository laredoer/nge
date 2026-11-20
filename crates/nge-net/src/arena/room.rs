//! 服务端的一个权威房间（纯逻辑，服务端按 `TICK_HZ` 调 `step`）：
//!
//! - 每个人一个输入队列，按 tick 编号放；算到某个 tick 时用那个 tick 的输入，没到的沿用上一个（客户端回滚时会对上）。
//!   已经算过的 tick 的输入、超前太多的输入丢掉。
//! - 每个 tick 算完记进历史（最近 `HISTORY` 个）：给每个人发快照时拿他确认过的那个 tick 当基准做增量；
//!   判拍人时回到拍人的人当时看到的 tick（延迟补偿），最多回溯 `MAX_REWIND` 个 tick。

use std::collections::{BTreeMap, VecDeque};

use super::delta::{InputMsg, WorldSnapshot, diff};
use super::{World, spawn, step, tag_target, wants_tag};
use crate::proto::PeerId;

/// 输入最多超前服务端几个 tick（2 秒）。
pub const MAX_LEAD: u32 = 60;
/// 每个人最多排几个 tick 的输入。
pub const MAX_QUEUE: usize = 64;
/// 留几个 tick 的历史。
pub const HISTORY: usize = 32;
/// 拍人最多回溯几个 tick（300 ms）。
pub const MAX_REWIND: u32 = 9;
/// 一个房间最多几个人。
pub const MAX_PLAYERS: usize = 16;

/// 一个人的输入队列。
#[derive(Debug, Default, Clone)]
pub struct InputQueue {
    inputs: BTreeMap<u32, u8>,
    last: u8,
    ack: Option<u32>,
}

impl InputQueue {
    /// 收到从 `first` 起的输入；服务端已经算到 `processed`。
    pub fn receive(&mut self, first: u32, inputs: &[u8], processed: u32) {
        for (i, &b) in inputs.iter().enumerate() {
            let Some(t) = first.checked_add(i as u32) else { break };
            if t <= processed || t > processed.saturating_add(MAX_LEAD) {
                continue;
            }
            // 同一个 tick 重发的是同一个输入：先到的为准
            self.inputs.entry(t).or_insert(b);
        }
        while self.inputs.len() > MAX_QUEUE {
            self.inputs.pop_last();
        }
    }

    /// 算 tick `tick` 时用的输入：有就用（记下确认到这里），没有就沿用上一个。
    pub fn take(&mut self, tick: u32) -> u8 {
        self.inputs = self.inputs.split_off(&tick);
        if let Some(b) = self.inputs.remove(&tick) {
            self.last = b;
            self.ack = Some(tick);
        }
        self.last
    }

    /// 用到的最新一个真输入的 tick。
    pub fn ack(&self) -> Option<u32> {
        self.ack
    }

    pub fn len(&self) -> usize {
        self.inputs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }
}

#[derive(Debug, Default)]
struct Player {
    queue: InputQueue,
    /// 他确认收到的最新快照：下一个快照的基准。
    base: Option<u32>,
    /// 他看到的远端是哪个 tick。
    view: u32,
}

/// 一个权威房间。
#[derive(Debug, Default)]
pub struct ArenaRoom {
    tick: u32,
    world: World,
    players: BTreeMap<PeerId, Player>,
    history: VecDeque<(u32, World)>,
    /// 拍中了几次（统计）。
    pub tags: u64,
}

impl ArenaRoom {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tick(&self) -> u32 {
        self.tick
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn len(&self) -> usize {
        self.players.len()
    }

    pub fn is_empty(&self) -> bool {
        self.players.is_empty()
    }

    /// 有人进来：在出生点放一个角色。满了返回 false。
    pub fn join(&mut self, id: PeerId) -> bool {
        if self.players.len() >= MAX_PLAYERS && !self.players.contains_key(&id) {
            return false;
        }
        self.players.entry(id).or_default();
        self.world.entry(id).or_insert_with(|| spawn(id));
        true
    }

    pub fn leave(&mut self, id: PeerId) {
        self.players.remove(&id);
        self.world.remove(&id);
    }

    /// 收到 `id` 的输入。
    pub fn input(&mut self, id: PeerId, msg: &InputMsg) {
        let tick = self.tick;
        let Some(p) = self.players.get_mut(&id) else { return };
        p.queue.receive(msg.first, &msg.inputs, tick);
        if let Some(a) = msg.ack
            && a <= tick
            && p.base.is_none_or(|b| a > b)
        {
            p.base = Some(a);
        }
        p.view = msg.view.min(tick);
    }

    fn at(&self, tick: u32) -> Option<&World> {
        self.history.iter().rev().find(|(t, _)| *t == tick).map(|(_, w)| w)
    }

    /// 走一个 tick，返回给每个人的快照。
    pub fn step(&mut self) -> Vec<(PeerId, WorldSnapshot)> {
        let prev = self.world.clone();
        self.tick += 1;
        let tick = self.tick;
        let mut tags = Vec::new();
        for (&id, p) in self.players.iter_mut() {
            let input = p.queue.take(tick);
            let Some(hero) = self.world.get_mut(&id) else { continue };
            if wants_tag(hero, input) {
                tags.push((id, p.view, [hero.x, hero.y]));
            }
            *hero = step(hero, input);
        }
        // 拍人：回到拍的人看到的那个 tick 找人（够不着的回溯按上限截）
        for (id, view, at) in tags {
            let view = view.clamp(tick.saturating_sub(1 + MAX_REWIND), tick - 1);
            let past = self.at(view).unwrap_or(&prev);
            if let Some(target) = tag_target(past, id, at)
                && let Some(h) = self.world.get_mut(&target)
            {
                h.tagged = h.tagged.saturating_add(1);
                self.tags += 1;
            }
        }
        self.history.push_back((tick, self.world.clone()));
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        let mut out = Vec::with_capacity(self.players.len());
        for (&id, p) in &self.players {
            let base = p.base.and_then(|b| self.at(b).map(|w| (b, w)));
            let (entities, removed) = diff(base.map(|(_, w)| w), &self.world);
            out.push((id, WorldSnapshot { tick, base: base.map(|(b, _)| b), ack: p.queue.ack(), entities, removed }));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::delta::apply;
    use crate::arena::{HALF_H, Hero, TAG_RADIUS, button};

    fn input(first: u32, inputs: &[u8]) -> InputMsg {
        InputMsg { first, inputs: inputs.to_vec(), ack: None, view: 0 }
    }

    #[test]
    fn inputs_are_used_by_tick_and_the_last_one_repeats() {
        let mut q = InputQueue::default();
        q.receive(3, &[1, 2], 0);
        q.receive(1, &[9], 0);
        assert_eq!(q.take(1), 9);
        assert_eq!(q.take(2), 9, "没到的沿用上一个");
        assert_eq!(q.ack(), Some(1));
        assert_eq!(q.take(3), 1);
        q.receive(3, &[7, 7], 3);
        assert_eq!(q.take(4), 2, "算过的 tick 的输入不要，没算过的先到的为准");
        assert_eq!(q.ack(), Some(4));
        q.receive(5 + MAX_LEAD, &[1], 4);
        assert!(q.is_empty(), "超前太多的丢掉");
        q.receive(5, &vec![1; MAX_LEAD as usize], 4);
        q.receive(5, &[0; 32], 4);
        assert!(q.len() <= MAX_QUEUE);
    }

    #[test]
    fn inputs_arriving_out_of_order_are_sorted_by_tick() {
        let mut q = InputQueue::default();
        q.receive(4, &[4], 0);
        q.receive(2, &[2], 0);
        q.receive(3, &[3], 0);
        q.receive(1, &[1], 0);
        assert_eq!((1..=4).map(|t| q.take(t)).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn snapshots_are_deltas_against_what_you_acknowledged() {
        let mut room = ArenaRoom::new();
        room.join(1);
        room.join(2);
        let first = room.step();
        assert!(first.iter().all(|(_, s)| s.base.is_none() && s.entities.len() == 2), "还没确认过：全量");
        room.input(1, &InputMsg { ack: Some(1), ..input(2, &[0]) });
        let second = room.step();
        let mine = &second.iter().find(|(id, _)| *id == 1).unwrap().1;
        assert_eq!(mine.base, Some(1));
        assert!(mine.entities.is_empty(), "都站着不动：什么都不发 {mine:?}");
        assert_eq!(mine.ack, Some(2));
        // 有人动了：只发他、只发变了的字段
        room.input(2, &input(3, &[button::RIGHT]));
        let third = room.step();
        let mine = &third.iter().find(|(id, _)| *id == 1).unwrap().1;
        assert_eq!(mine.entities.len(), 1);
        assert_eq!(mine.entities[0].id, 2);
        assert!(mine.entities[0].fields.len() <= 2, "{:?}", mine.entities[0]);
        let mut book = World::new();
        book.extend(apply(None, &first[0].1.entities, &[]).unwrap());
        assert_eq!(apply(Some(&book), &mine.entities, &mine.removed).unwrap(), *room.world());
        // 有人走了：列在 removed 里
        room.leave(2);
        room.input(1, &InputMsg { ack: Some(3), ..input(4, &[0]) });
        let fourth = room.step();
        assert_eq!(fourth[0].1.removed, vec![2]);
    }

    #[test]
    fn a_base_that_fell_out_of_history_gets_a_full_snapshot() {
        let mut room = ArenaRoom::new();
        room.join(1);
        room.step();
        room.input(1, &InputMsg { ack: Some(1), ..input(2, &[0]) });
        for _ in 0..HISTORY + 1 {
            room.step();
        }
        let s = room.step();
        assert_eq!(s[0].1.base, None);
        assert_eq!(s[0].1.entities.len(), 1);
        // 没收到过的、还没发生的 tick 不当基准
        room.input(1, &InputMsg { ack: Some(room.tick() + 5), ..input(room.tick() + 1, &[0]) });
        assert_eq!(room.step()[0].1.base, None);
    }

    /// 1 站在 2 旁边 10 个 tick，然后 2 往右跑 `run` 个 tick 跑开；1 这时按拍，他看到的是 `view_lag` 个 tick 以前的 2。
    fn chase(run: u32, view_lag: u32) -> u16 {
        let mut room = ArenaRoom::new();
        room.join(1);
        room.join(2);
        room.world.insert(1, Hero { x: 100.0, ..crate::arena::spawn(1) });
        room.world.insert(2, Hero { x: 130.0, ..crate::arena::spawn(2) });
        for _ in 1..=10 {
            room.step();
        }
        for t in 11..=10 + run {
            room.input(2, &input(t, &[button::RIGHT]));
            room.step();
        }
        assert!(room.world()[&2].x - room.world()[&1].x > TAG_RADIUS, "2 已经跑开了");
        let t = room.tick() + 1;
        room.input(1, &InputMsg { view: (t - 1).saturating_sub(view_lag), ..input(t, &[button::TAG]) });
        room.input(2, &input(t, &[button::RIGHT]));
        room.step();
        room.world()[&2].tagged
    }

    #[test]
    fn tags_are_judged_where_the_attacker_saw_the_target() {
        assert_eq!(chase(10, 0), 0, "按现在的位置：够不着");
        assert_eq!(chase(10, 9), 1, "回溯到他看到的时候：拍中了");
        assert_eq!(chase(20, 25), 0, "看到的太旧：只回溯到上限，那时已经够不着了");
    }

    #[test]
    fn the_same_inputs_give_the_same_world() {
        let run = || {
            let mut room = ArenaRoom::new();
            for id in 1..=3 {
                room.join(id);
            }
            let mut seed = 99u32;
            for t in 1..=300u32 {
                for id in 1..=3 {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    room.input(id, &InputMsg { view: t.saturating_sub(3), ..input(t, &[((seed >> 16) as u8) & button::ALL]) });
                }
                room.step();
            }
            room.world().iter().map(|(id, h)| (*id, h.x.to_bits(), h.y.to_bits(), h.vx.to_bits(), h.vy.to_bits(), h.tagged)).collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
        assert!(run().iter().all(|(_, _, y, ..)| f32::from_bits(*y) >= HALF_H));
    }

    #[test]
    fn rooms_have_a_size_limit() {
        let mut room = ArenaRoom::new();
        assert!((0..MAX_PLAYERS as u32).all(|id| room.join(id)));
        assert!(!room.join(999));
        assert!(room.join(0), "已经在里面的再进一次不算");
    }
}
