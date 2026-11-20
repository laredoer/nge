//! 客户端的权威模式（纯逻辑）：解增量快照（留着最近收到的几个当基准）、预测自己的角色、收到快照对账回滚、
//! 对时（超前服务端一个往返加一点余量），每走一个 tick 就把还没确认的输入发出去。

use std::collections::VecDeque;

use super::delta::{InputMsg, WorldSnapshot, apply};
use super::{DT, Hero, World, step};
use crate::proto::PeerId;
use crate::rollback::{MAX_STEPS, Model, Pacer, Predictor, Stats, target_lead};

/// 留几个解出来的快照（服务端可能拿其中任何一个当基准）。
pub const BOOK: usize = 64;
/// 一条输入消息里重发几个 tick 的输入（丢了的靠后面的补上）。
pub const RESEND: usize = 16;
/// 远端显示落后几个 tick（和插值的默认延迟 100 ms 对上）：告诉服务端，拍人时回溯到这里。
pub const VIEW_LAG: u32 = 3;
/// 超前得比想要的少这么多：一下子补上（刚连上、被重置以后）。
const CATCH_UP: i64 = 3;
/// 超前得比想要的多这么多：先停着等服务端追上来。
const TOO_FAR: i64 = 15;

/// 角色的预测模型：只比自己预测得了的字段（被拍的次数是服务端改的，不算误差）。
#[derive(Debug, Clone, Copy)]
pub struct HeroModel;

impl Model for HeroModel {
    type State = Hero;
    type Input = u8;
    fn step(&self, s: &Hero, i: u8) -> Hero {
        step(s, i)
    }
    fn pos(&self, s: &Hero) -> [f32; 2] {
        [s.x, s.y]
    }
    fn same(&self, a: &Hero, b: &Hero) -> bool {
        a.x.to_bits() == b.x.to_bits() && a.y.to_bits() == b.y.to_bits() && a.vx.to_bits() == b.vx.to_bits() && a.vy.to_bits() == b.vy.to_bits() && a.ground == b.ground && a.cool == b.cool
    }
}

pub struct ArenaClient {
    me: PeerId,
    predictor: Option<Predictor<HeroModel>>,
    book: VecDeque<(u32, World)>,
    pacer: Pacer,
    latest: Option<u32>,
    /// 解不出来（基准没了、数据不一致）丢掉的快照。
    pub dropped: u32,
}

impl ArenaClient {
    pub fn new(me: PeerId) -> Self {
        Self { me, predictor: None, book: VecDeque::new(), pacer: Pacer::default(), latest: None, dropped: 0 }
    }

    pub fn me(&self) -> PeerId {
        self.me
    }

    /// 收到一个快照：是新的就解出整个世界、对账，返回解出来的世界（远端角色拿去插值）。
    pub fn receive(&mut self, s: &WorldSnapshot) -> Option<&World> {
        if self.latest.is_some_and(|l| s.tick <= l) {
            return None;
        }
        let base = match s.base {
            None => None,
            Some(b) => match self.book.iter().find(|(t, _)| *t == b) {
                Some((_, w)) => Some(w),
                None => {
                    self.dropped += 1;
                    return None;
                }
            },
        };
        let Ok(world) = apply(base, &s.entities, &s.removed) else {
            self.dropped += 1;
            return None;
        };
        if let Some(hero) = world.get(&self.me) {
            match &mut self.predictor {
                Some(p) => {
                    p.reconcile(s.tick, hero);
                }
                None => self.predictor = Some(Predictor::new(HeroModel, s.tick, *hero)),
            }
        }
        self.latest = Some(s.tick);
        self.book.push_back((s.tick, world));
        while self.book.len() > BOOK {
            self.book.pop_front();
        }
        self.book.back().map(|(_, w)| w)
    }

    /// 过了 `dt` 秒，现在按着 `input`，往返时间 `rtt` 秒：该走的 tick 都走掉，要发的输入返回。
    pub fn frame(&mut self, dt: f64, input: u8, rtt: f64) -> Option<InputMsg> {
        let latest = self.latest?;
        let p = self.predictor.as_mut()?;
        let lead = p.tick() as i64 - latest as i64;
        let target = target_lead(rtt, DT as f64);
        let mut steps = self.pacer.advance(dt, DT as f64, lead, target);
        if lead + CATCH_UP < target {
            steps = (steps + (target - lead) as u32).min(2 * MAX_STEPS);
        } else if lead > target + TOO_FAR {
            steps = 0;
        }
        for _ in 0..steps {
            p.predict(input);
        }
        p.smooth(dt as f32);
        if steps == 0 {
            return None;
        }
        let (first, inputs) = p.pending(RESEND)?;
        Some(InputMsg { first, inputs, ack: Some(latest), view: latest.saturating_sub(VIEW_LAG) })
    }

    /// 自己的角色该画在哪（两个 tick 之间插值，加上修正偏移）；还没收到自己时是 `None`。
    pub fn display(&self) -> Option<[f32; 2]> {
        self.predictor.as_ref().map(|p| p.display(self.pacer.alpha(DT as f64)))
    }

    /// 自己预测到的状态。
    pub fn predicted(&self) -> Option<&Hero> {
        self.predictor.as_ref().map(|p| p.state())
    }

    pub fn stats(&self) -> Stats {
        self.predictor.as_ref().map(|p| p.stats).unwrap_or_default()
    }

    /// 超前服务端（最新快照）几个 tick。
    pub fn lead(&self) -> Option<i64> {
        Some(self.predictor.as_ref()?.tick() as i64 - self.latest? as i64)
    }

    pub fn tick(&self) -> Option<u32> {
        self.predictor.as_ref().map(|p| p.tick())
    }

    /// 最新的服务端世界。
    pub fn world(&self) -> Option<(u32, &World)> {
        self.book.back().map(|(t, w)| (*t, w))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{button, spawn};
    use crate::arena::room::ArenaRoom;
    use crate::simnet::SimNet;

    /// 一个服务端、两个客户端，中间是模拟网络；客户端 60 帧每秒，服务端 30 tick 每秒。
    struct Rig {
        room: ArenaRoom,
        clients: Vec<ArenaClient>,
        up: Vec<SimNet<InputMsg>>,
        down: Vec<SimNet<WorldSnapshot>>,
        now: f64,
        next_tick: f64,
        rtt: f64,
    }

    impl Rig {
        fn new(latency: f64, jitter: f64, loss: f64) -> Self {
            let mut room = ArenaRoom::new();
            room.join(1);
            room.join(2);
            Self {
                room,
                clients: vec![ArenaClient::new(1), ArenaClient::new(2)],
                up: (0..2).map(|i| SimNet::new(latency, jitter, loss, 11 + i)).collect(),
                down: (0..2).map(|i| SimNet::new(latency, jitter, loss, 23 + i)).collect(),
                now: 0.0,
                next_tick: 0.0,
                // 心跳量到的往返：基本延迟，抖动大的时候有的输入会晚到
                rtt: 2.0 * latency,
            }
        }

        /// 走 `seconds` 秒，`input(客户端, 时间)` 给按键。
        fn run(&mut self, seconds: f64, input: impl Fn(usize, f64) -> u8) {
            let frame = 1.0 / 60.0;
            let end = self.now + seconds;
            while self.now < end {
                self.now += frame;
                while self.next_tick <= self.now {
                    for (i, net) in self.up.iter_mut().enumerate() {
                        for m in net.recv(self.now) {
                            self.room.input(i as u32 + 1, &m);
                        }
                    }
                    for (id, s) in self.room.step() {
                        self.down[id as usize - 1].send(self.now, s);
                    }
                    self.next_tick += DT as f64;
                }
                for (i, c) in self.clients.iter_mut().enumerate() {
                    for s in self.down[i].recv(self.now) {
                        c.receive(&s);
                    }
                    if let Some(m) = c.frame(frame, input(i, self.now), self.rtt) {
                        self.up[i].send(self.now, m);
                    }
                }
            }
        }

        fn wander(i: usize, t: f64) -> u8 {
            // 每个人按自己的节奏左右跑、跳、拍
            let phase = (t * (1.3 + i as f64 * 0.7)) as u64;
            [button::RIGHT, button::RIGHT | button::JUMP, button::LEFT, 0, button::LEFT | button::JUMP, button::TAG][(phase % 6) as usize]
        }

        /// 停手以后：预测的、显示的和服务端的一样。
        fn assert_converged(&self) {
            for c in &self.clients {
                let server = self.room.world()[&c.me()];
                let mine = c.predicted().unwrap();
                assert_eq!((mine.x.to_bits(), mine.y.to_bits(), mine.vx.to_bits(), mine.vy.to_bits()), (server.x.to_bits(), server.y.to_bits(), server.vx.to_bits(), server.vy.to_bits()), "客户端 {} 预测的 {mine:?}，服务端 {server:?}", c.me());
                let shown = c.display().unwrap();
                assert!((shown[0] - server.x).abs() < 0.01 && (shown[1] - server.y).abs() < 0.01, "显示的 {shown:?} 和服务端 {server:?} 对上");
            }
        }
    }

    #[test]
    fn any_differing_field_counts_as_a_misprediction() {
        let m = HeroModel;
        let base = spawn(1);
        assert!(m.same(&base, &base));
        let changes: [fn(&mut Hero); 6] = [|h| h.x += 0.5, |h| h.y += 0.5, |h| h.vx = 1.0, |h| h.vy = 1.0, |h| h.ground = !h.ground, |h| h.cool = 3];
        for (i, change) in changes.iter().enumerate() {
            let mut other = base;
            change(&mut other);
            assert!(!m.same(&base, &other), "第 {i} 个字段不同也得算预测错了");
        }
    }

    #[test]
    fn a_clean_steady_link_predicts_without_rollbacks() {
        let mut rig = Rig::new(0.05, 0.0, 0.0);
        rig.run(1.0, |_, _| 0);
        rig.run(6.0, Rig::wander);
        rig.run(2.0, |_, _| 0);
        for c in &rig.clients {
            let lead = c.lead().unwrap();
            let target = target_lead(rig.rtt, DT as f64);
            assert!((target - 1..=target + 3).contains(&lead), "超前 {lead} 个 tick，想要 {target}");
            assert_eq!(c.stats().rollbacks, 0, "输入总是按时到：预测一次都没错 {:?}", c.stats());
        }
        rig.assert_converged();
    }

    #[test]
    fn prediction_converges_under_delay_loss_and_reordering() {
        let mut rig = Rig::new(0.06, 0.12, 0.15);
        rig.run(1.0, |_, _| 0);
        rig.run(10.0, Rig::wander);
        rig.run(3.0, |_, _| 0);
        assert!(rig.up.iter().all(|n| n.lost > 0 && n.reordered > 0), "真的丢了、乱了");
        let rolled: u32 = rig.clients.iter().map(|c| c.stats().rollbacks).sum();
        assert!(rolled > 0, "丢包时服务端沿用旧输入，客户端要回滚");
        rig.assert_converged();
        assert!(rig.room.world().values().all(|h| h.tagged == 0) || rig.room.tags > 0);
    }

    #[test]
    fn a_long_stall_recovers() {
        let mut rig = Rig::new(0.05, 0.0, 0.0);
        rig.run(1.0, Rig::wander);
        // 客户端 1 的线路断了 1 秒：服务端那边沿用旧输入，客户端预测的偏了
        rig.up[0].loss = 1.0;
        rig.down[0].loss = 1.0;
        rig.run(1.0, Rig::wander);
        rig.up[0].loss = 0.0;
        rig.down[0].loss = 0.0;
        rig.run(3.0, |_, _| 0);
        rig.assert_converged();
    }

    #[test]
    fn deltas_without_their_base_are_dropped_and_old_ones_ignored() {
        let mut room = ArenaRoom::new();
        room.join(1);
        let mut c = ArenaClient::new(1);
        let first = room.step().remove(0).1;
        let second = room.step().remove(0).1;
        assert!(c.receive(&second).is_some());
        assert!(c.receive(&first).is_none(), "比已经有的旧");
        let orphan = WorldSnapshot { tick: 10, base: Some(9), ack: None, entities: vec![], removed: vec![] };
        assert!(c.receive(&orphan).is_none());
        assert_eq!(c.dropped, 1);
        assert_eq!(c.world().unwrap().0, 2);
    }
}
