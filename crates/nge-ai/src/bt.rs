//! 行为树：顺序、选择、并行、取反、重复、冷却、等待，条件和动作是叶子，叶子之间用黑板传值（见 docs/gameplay2d.md）。
//!
//! 叶子是什么由用的人定（`Node<L>` 的 `L`），每帧 [`Tree::tick`] 时把叶子交给回调去跑。
//! 和 [`crate::Fsm`] 不冲突：状态机管「现在是什么状态」，行为树管「这一帧该做什么」，叶子里可以读写状态机。

use std::collections::HashMap;

use glam::Vec2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Success,
    Failure,
    Running,
}

impl From<bool> for Status {
    fn from(ok: bool) -> Self {
        if ok { Status::Success } else { Status::Failure }
    }
}

/// 黑板上的值。
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Num(f32),
    Text(String),
    Pos(Vec2),
}

impl Value {
    /// 当条件用：false、0、空字符串是假。
    pub fn truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0.0,
            Value::Text(t) => !t.is_empty(),
            Value::Pos(_) => true,
        }
    }
}

/// 黑板：一棵树的叶子共用的键值。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Blackboard(HashMap<String, Value>);

impl Blackboard {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.0.insert(key.to_owned(), value);
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.0.remove(key)
    }

    pub fn num(&self, key: &str) -> Option<f32> {
        match self.get(key) {
            Some(Value::Num(n)) => Some(*n),
            _ => None,
        }
    }

    pub fn pos(&self, key: &str) -> Option<Vec2> {
        match self.get(key) {
            Some(Value::Pos(p)) => Some(*p),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Kind<L> {
    Sequence(Vec<Node<L>>),
    Selector(Vec<Node<L>>),
    Parallel(usize, Vec<Node<L>>),
    Invert(Box<Node<L>>),
    Repeat(Option<u32>, Box<Node<L>>),
    Cooldown(f32, Box<Node<L>>),
    Wait(f32),
    Leaf(L),
}

/// 树上的一个节点，带着自己跑到哪了。
#[derive(Debug, Clone, PartialEq)]
pub struct Node<L> {
    kind: Kind<L>,
    /// 顺序 / 选择：每帧从第一个重新查（不记跑到第几个）。
    reactive: bool,
    running: bool,
    /// 顺序 / 选择跑到第几个；重复跑了几次。
    at: usize,
    /// 等待从什么时候开始；冷却到什么时候结束。
    time: f32,
    /// 并行里每个孩子的结果（还在跑的是 None）。
    done: Vec<Option<Status>>,
}

impl<L> Node<L> {
    fn new(kind: Kind<L>) -> Self {
        Self { kind, reactive: false, running: false, at: 0, time: f32::NEG_INFINITY, done: Vec::new() }
    }

    /// 一个一个跑，有一个失败就失败，全成了才成；还在跑的下一帧接着跑。
    pub fn sequence(children: Vec<Node<L>>) -> Self {
        Self::new(Kind::Sequence(children))
    }

    /// 一个一个试，有一个成了就成，全失败才失败。
    pub fn selector(children: Vec<Node<L>>) -> Self {
        Self::new(Kind::Selector(children))
    }

    /// 每帧都跑所有没跑完的孩子，成了 `succeed` 个就成，凑不够了就失败。
    pub fn parallel(succeed: usize, children: Vec<Node<L>>) -> Self {
        Self::new(Kind::Parallel(succeed, children))
    }

    pub fn invert(child: Node<L>) -> Self {
        Self::new(Kind::Invert(Box::new(child)))
    }

    /// 孩子成了就再跑（下一帧），成了 `times` 次才成（`None` 一直重复）；孩子失败就失败。
    pub fn repeat(times: Option<u32>, child: Node<L>) -> Self {
        Self::new(Kind::Repeat(times, Box::new(child)))
    }

    /// 孩子跑完（成或败）以后 `seconds` 秒内直接失败，不跑孩子。
    pub fn cooldown(seconds: f32, child: Node<L>) -> Self {
        Self::new(Kind::Cooldown(seconds, Box::new(child)))
    }

    pub fn wait(seconds: f32) -> Self {
        Self::new(Kind::Wait(seconds))
    }

    pub fn leaf(leaf: L) -> Self {
        Self::new(Kind::Leaf(leaf))
    }

    /// 顺序 / 选择改成每帧从第一个孩子重新查：条件变了能打断后面在跑的（比如看见玩家就不巡逻了）。
    pub fn reactive(mut self) -> Self {
        self.reactive = true;
        self
    }

    /// 打断：这个节点和下面在跑的都回到没开始的样子（冷却的时间留着）。
    fn halt(&mut self) {
        self.running = false;
        self.at = 0;
        self.done.clear();
        if matches!(self.kind, Kind::Wait(_)) {
            self.time = f32::NEG_INFINITY;
        }
        match &mut self.kind {
            Kind::Sequence(c) | Kind::Selector(c) | Kind::Parallel(_, c) => c.iter_mut().for_each(Node::halt),
            Kind::Invert(c) | Kind::Repeat(_, c) | Kind::Cooldown(_, c) => c.halt(),
            Kind::Wait(_) | Kind::Leaf(_) => {}
        }
    }

    fn tick(&mut self, now: f32, dt: f32, run: &mut dyn FnMut(&L, &mut Blackboard) -> Status, bb: &mut Blackboard) -> Status {
        let status = match &mut self.kind {
            Kind::Sequence(children) => composite(children, self.reactive, &mut self.at, Status::Success, now, dt, run, bb),
            Kind::Selector(children) => composite(children, self.reactive, &mut self.at, Status::Failure, now, dt, run, bb),
            Kind::Parallel(succeed, children) => {
                self.done.resize(children.len(), None);
                for (c, d) in children.iter_mut().zip(&mut self.done) {
                    if d.is_none() {
                        let s = c.tick(now, dt, run, bb);
                        *d = (s != Status::Running).then_some(s);
                    }
                }
                let ok = self.done.iter().filter(|d| **d == Some(Status::Success)).count();
                let failed = self.done.iter().filter(|d| **d == Some(Status::Failure)).count();
                let s = if ok >= *succeed {
                    Status::Success
                } else if children.len() - failed < *succeed {
                    Status::Failure
                } else {
                    Status::Running
                };
                if s != Status::Running {
                    children.iter_mut().for_each(Node::halt);
                    self.done.clear();
                }
                s
            }
            Kind::Invert(child) => match child.tick(now, dt, run, bb) {
                Status::Success => Status::Failure,
                Status::Failure => Status::Success,
                Status::Running => Status::Running,
            },
            Kind::Repeat(times, child) => match child.tick(now, dt, run, bb) {
                Status::Success => {
                    self.at += 1;
                    if times.is_some_and(|t| self.at as u32 >= t) {
                        self.at = 0;
                        Status::Success
                    } else {
                        // 孩子跑完自己就回到没开始的样子，下一帧从头跑
                        Status::Running
                    }
                }
                Status::Failure => {
                    self.at = 0;
                    Status::Failure
                }
                Status::Running => Status::Running,
            },
            Kind::Cooldown(seconds, child) => {
                if now < self.time {
                    Status::Failure
                } else {
                    let s = child.tick(now, dt, run, bb);
                    if s != Status::Running {
                        self.time = now + *seconds;
                    }
                    s
                }
            }
            Kind::Wait(seconds) => {
                if !self.running {
                    // 从这一帧开头算起
                    self.time = now - dt;
                }
                if now - self.time >= *seconds - 1e-6 { Status::Success } else { Status::Running }
            }
            Kind::Leaf(leaf) => run(leaf, bb),
        };
        self.running = status == Status::Running;
        if !self.running && matches!(self.kind, Kind::Wait(_)) {
            self.time = f32::NEG_INFINITY;
        }
        status
    }
}

/// 顺序（`stop` 是 Failure）和选择（`stop` 是 Success）：碰到 `stop` 或在跑的就停在那。`pass` 是孩子的什么结果算接着往下。
#[allow(clippy::too_many_arguments)]
fn composite<L>(
    children: &mut [Node<L>],
    reactive: bool,
    at: &mut usize,
    pass: Status,
    now: f32,
    dt: f32,
    run: &mut dyn FnMut(&L, &mut Blackboard) -> Status,
    bb: &mut Blackboard,
) -> Status {
    let start = if reactive { 0 } else { *at };
    for i in start..children.len() {
        let s = children[i].tick(now, dt, run, bb);
        if s == pass {
            continue;
        }
        // 停在这：后面之前在跑的（反应式时被前面的打断了）回到没开始
        for later in &mut children[i + 1..] {
            if later.running {
                later.halt();
            }
        }
        *at = if s == Status::Running { i } else { 0 };
        return s;
    }
    *at = 0;
    pass
}

/// 一棵树：根节点、黑板、树自己的时钟（冷却和等待按它算）。
#[derive(Debug, Clone, PartialEq)]
pub struct Tree<L> {
    pub root: Node<L>,
    pub blackboard: Blackboard,
    time: f32,
}

impl<L> Tree<L> {
    pub fn new(root: Node<L>) -> Self {
        Self { root, blackboard: Blackboard::default(), time: 0.0 }
    }

    /// 过了 `dt` 秒，从根跑一遍；叶子交给 `run`。
    pub fn tick(&mut self, dt: f32, run: &mut dyn FnMut(&L, &mut Blackboard) -> Status) -> Status {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.time += dt;
        self.root.tick(self.time, dt, run, &mut self.blackboard)
    }

    /// 打断整棵树（在跑的都回到没开始，冷却留着）。
    pub fn halt(&mut self) {
        self.root.halt();
    }

    pub fn time(&self) -> f32 {
        self.time
    }
}

#[cfg(test)]
#[path = "bt_tests.rs"]
mod tests;
