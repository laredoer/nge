//! 服务端的一个权威房间（纯逻辑，服务端按 `TICK_HZ` 调 `step`）：
//!
//! - 每个人一个输入队列，按 tick 编号放；算到某个 tick 时用那个 tick 的输入，没到的沿用上一个（客户端回滚时会对上）。
//!   已经算过的 tick 的输入、超前太多的输入丢掉。每个输入记着按下它时客户端看到的远端是哪个 tick（延迟补偿用）。
//! - tick 是 `u32`，30 Hz 下 4.5 年才到头；到头了房间停住（不回绕、不 panic），人走光以后房间重建。
//! - 每个 tick 算完记进历史（最近 `HISTORY` 个）：给每个人发快照时拿他确认过的那个 tick 当基准做增量；
//!   判拍人时回到拍人的人当时看到的 tick（延迟补偿），最多回溯 `MAX_REWIND` 个 tick。

use std::collections::{BTreeMap, VecDeque};

use super::delta::{InputMsg, WorldSnapshot, diff};
use super::client::VIEW_LAG;
use super::{TICK_HZ, World, spawn, step, tag_target, wants_tag};
use crate::proto::PeerId;
use crate::rollback::LEAD_MARGIN;

/// 输入最多超前服务端几个 tick（2 秒）。
pub const MAX_LEAD: u32 = 60;
/// 每个人最多排几个 tick 的输入。
pub const MAX_QUEUE: usize = 64;
/// 留几个 tick 的历史。
pub const HISTORY: usize = 32;
/// 延迟补偿覆盖到多大的往返（毫秒）。
pub const COMPENSATED_RTT_MS: u32 = 300;
/// 拍人最多回溯几个 tick：按下拍的那个 tick 比客户端看到的服务端超前 `ceil(往返 / DT) + LEAD_MARGIN` 个，
/// 看到的远端又比最新快照落后 `VIEW_LAG` 个；多留 1 个给往返换成 tick 时的取整。300 ms 是 9 + 1 + 2 + 3 = 15。
pub const MAX_REWIND: u32 = COMPENSATED_RTT_MS * TICK_HZ / 1000 + 1 + LEAD_MARGIN as u32 + VIEW_LAG;
/// 一个房间最多几个人。
pub const MAX_PLAYERS: usize = 16;

// 只收 (processed, processed + MAX_LEAD] 里的输入，队列不会超过 MAX_QUEUE
const _: () = assert!(MAX_LEAD as usize <= MAX_QUEUE);

/// 一个人的输入队列。
#[derive(Debug, Default, Clone)]
pub struct InputQueue {
    /// tick → (按键, 那时看到的远端 tick)。
    inputs: BTreeMap<u32, (u8, u32)>,
    last: (u8, u32),
    ack: Option<u32>,
}

impl InputQueue {
    /// 收到从 `first` 起的输入，最后一个输入时客户端看到的远端是 `view`；服务端已经算到 `processed`。
    /// 前面的输入看到的远端按每个 tick 早一个算（第一次收到时一般就是最后一个，重发的才往前推）。
    pub fn receive(&mut self, first: u32, inputs: &[u8], view: u32, processed: u32) {
        let newest = first.saturating_add(inputs.len().saturating_sub(1) as u32);
        for (i, &b) in inputs.iter().enumerate() {
            let Some(t) = first.checked_add(i as u32) else { break };
            if t <= processed || t > processed.saturating_add(MAX_LEAD) {
                continue;
            }
            // 同一个 tick 重发的是同一个输入：先到的为准
            self.inputs.entry(t).or_insert((b, view.saturating_sub(newest - t)));
        }
    }

    /// 算 tick `tick` 时用的输入和那时看到的远端 tick：有就用（记下确认到这里），没有就沿用上一个
    /// （看到的远端跟着往后推）。
    pub fn take(&mut self, tick: u32) -> (u8, u32) {
        self.inputs = self.inputs.split_off(&tick);
        if let Some(entry) = self.inputs.remove(&tick) {
            self.last = entry;
            self.ack = Some(tick);
        }
        let (input, view) = self.last;
        (input, view.saturating_add(self.ack.map_or(0, |a| tick - a)))
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
        p.queue.receive(msg.first, &msg.inputs, msg.view, tick);
        if let Some(a) = msg.ack
            && a <= tick
            && p.base.is_none_or(|b| a > b)
        {
            p.base = Some(a);
        }
    }

    fn at(&self, tick: u32) -> Option<&World> {
        self.history.iter().rev().find(|(t, _)| *t == tick).map(|(_, w)| w)
    }

    /// 走一个 tick，返回给每个人的快照；tick 到头了不走，返回空的。
    pub fn step(&mut self) -> Vec<(PeerId, WorldSnapshot)> {
        let Some(tick) = self.tick.checked_add(1) else { return Vec::new() };
        let prev = self.world.clone();
        self.tick = tick;
        let mut tags = Vec::new();
        for (&id, p) in self.players.iter_mut() {
            let (input, view) = p.queue.take(tick);
            let Some(hero) = self.world.get_mut(&id) else { continue };
            if wants_tag(hero, input) {
                tags.push((id, view, [hero.x, hero.y]));
            }
            *hero = step(hero, input);
        }
        // 拍人：回到拍的人看到的那个 tick 找人（够不着的回溯按上限截）
        for (id, view, at) in tags {
            let view = view.clamp(tick.saturating_sub(MAX_REWIND), tick - 1);
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
            out.push((id, WorldSnapshot { tick, base: base.map(|(b, _)| b), entities, removed }));
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
        q.receive(3, &[1, 2], 0, 0);
        q.receive(1, &[9], 0, 0);
        assert_eq!(q.take(1).0, 9);
        assert_eq!(q.take(2).0, 9, "没到的沿用上一个");
        assert_eq!(q.ack(), Some(1));
        assert_eq!(q.take(3).0, 1);
        q.receive(3, &[7, 7], 0, 3);
        assert_eq!(q.take(4).0, 2, "算过的 tick 的输入不要，没算过的先到的为准");
        assert_eq!(q.ack(), Some(4));
        q.receive(5 + MAX_LEAD, &[1], 0, 4);
        assert!(q.is_empty(), "超前太多的丢掉");
        q.receive(5, &vec![1; MAX_LEAD as usize + 8], 0, 4);
        assert_eq!(q.len(), MAX_LEAD as usize, "最多排 MAX_LEAD 个");
    }

    #[test]
    fn each_input_remembers_what_the_player_saw_when_pressing_it() {
        let mut q = InputQueue::default();
        // 最后一个（tick 7）按下时看到的是 10：前面的按一个 tick 早一个算
        q.receive(5, &[1, 2, 3], 10, 0);
        assert_eq!((5..=7).map(|t| q.take(t)).collect::<Vec<_>>(), vec![(1, 8), (2, 9), (3, 10)]);
        assert_eq!(q.take(8), (3, 11), "没到的沿用上一个，看到的跟着往后推");
        // 重发的不改第一次记下的
        q.receive(9, &[4], 20, 8);
        q.receive(9, &[4, 5], 30, 8);
        assert_eq!(q.take(9), (4, 20));
        assert_eq!(q.take(10), (5, 30));
    }

    #[test]
    fn inputs_arriving_out_of_order_are_sorted_by_tick() {
        let mut q = InputQueue::default();
        q.receive(4, &[4], 0, 0);
        q.receive(2, &[2], 0, 0);
        q.receive(3, &[3], 0, 0);
        q.receive(1, &[1], 0, 0);
        assert_eq!((1..=4).map(|t| q.take(t).0).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
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
        // 2 跑到第 3 个 tick 出了范围（tick 13）：跑 16 个 tick 以后按，要回溯 15 个，刚好上限
        assert_eq!(chase(16, MAX_REWIND - 1), 1, "刚好回溯上限：拍中");
        assert_eq!(chase(17, MAX_REWIND), 0, "要回溯 16 个：只回溯到上限，那时已经够不着了");
        assert_eq!(MAX_REWIND, 15, "300 ms 往返：9 + 1 + 2 + 3");
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
    fn the_tick_stops_at_the_end_instead_of_overflowing() {
        let mut room = ArenaRoom::new();
        room.join(1);
        room.tick = u32::MAX - 1;
        assert_eq!(room.step().len(), 1);
        assert_eq!(room.tick(), u32::MAX);
        assert!(room.step().is_empty(), "到头了：不再走（30 Hz 下要 4.5 年）");
        assert_eq!(room.tick(), u32::MAX);
    }

    #[test]
    fn rooms_have_a_size_limit() {
        let mut room = ArenaRoom::new();
        assert!((0..MAX_PLAYERS as u32).all(|id| room.join(id)));
        assert!(!room.join(999));
        assert!(room.join(0), "已经在里面的再进一次不算");
    }
}
