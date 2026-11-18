//! 行为树各节点的语义、冷却、黑板，和状态机一起用（见 docs/gameplay2d.md）。

use super::*;
use crate::Fsm;

/// 测试用的叶子：按名字从脚本里取这一帧的结果，记下谁被跑了。
#[derive(Debug, Clone, PartialEq)]
enum Leaf {
    Do(&'static str),
    Set(&'static str, f32),
    Has(&'static str),
}

struct World {
    status: Vec<(&'static str, Status)>,
    ran: Vec<&'static str>,
}

impl World {
    fn new() -> Self {
        Self { status: Vec::new(), ran: Vec::new() }
    }

    fn set(&mut self, name: &'static str, s: Status) {
        self.status.retain(|(n, _)| *n != name);
        self.status.push((name, s));
    }
}

fn tick(tree: &mut Tree<Leaf>, w: &mut World, dt: f32) -> Status {
    tree.tick(dt, &mut |leaf: &Leaf, bb: &mut Blackboard| match leaf {
        Leaf::Do(name) => {
            w.ran.push(name);
            w.status.iter().find(|(n, _)| n == name).map(|(_, s)| *s).unwrap_or(Status::Success)
        }
        Leaf::Set(key, v) => {
            bb.set(key, Value::Num(*v));
            Status::Success
        }
        Leaf::Has(key) => Status::from(bb.get(key).is_some()),
    })
}

fn act(name: &'static str) -> Node<Leaf> {
    Node::leaf(Leaf::Do(name))
}

use Status::{Failure, Running, Success};

#[test]
fn sequences_stop_at_the_first_failure_and_resume_the_running_child() {
    let mut tree = Tree::new(Node::sequence(vec![act("a"), act("b"), act("c")]));
    let mut w = World::new();
    w.set("b", Failure);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Failure);
    assert_eq!(w.ran, ["a", "b"]);
    w.set("b", Running);
    w.ran.clear();
    assert_eq!(tick(&mut tree, &mut w, 0.1), Running);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Running);
    assert_eq!(w.ran, ["a", "b", "b"], "接着跑还在跑的那个，不从头来");
    w.set("b", Success);
    w.ran.clear();
    assert_eq!(tick(&mut tree, &mut w, 0.1), Success);
    assert_eq!(w.ran, ["b", "c"]);
}

#[test]
fn reactive_sequences_recheck_conditions_every_tick() {
    let mut tree = Tree::new(Node::sequence(vec![act("see"), act("chase")]).reactive());
    let mut w = World::new();
    w.set("chase", Running);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Running);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Running);
    assert_eq!(w.ran, ["see", "chase", "see", "chase"]);
    w.set("see", Failure);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Failure, "看不见了就停");
}

#[test]
fn selectors_take_the_first_success_and_higher_priorities_interrupt() {
    let mut tree = Tree::new(Node::selector(vec![act("a"), act("b"), act("c")]));
    let mut w = World::new();
    w.set("a", Failure);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Success);
    assert_eq!(w.ran, ["a", "b"]);
    for s in ["a", "b", "c"] {
        w.set(s, Failure);
    }
    assert_eq!(tick(&mut tree, &mut w, 0.1), Failure);

    // 反应式：低优先级的在跑，高优先级的条件成了就打断它
    let wait_then = Node::sequence(vec![Node::wait(1.0), act("patrolled")]);
    let mut tree = Tree::new(Node::selector(vec![Node::sequence(vec![act("see"), act("chase")]).reactive(), wait_then]).reactive());
    let mut w = World::new();
    w.set("see", Failure);
    w.set("chase", Running);
    assert_eq!(tick(&mut tree, &mut w, 0.6), Running, "在等");
    w.set("see", Success);
    assert_eq!(tick(&mut tree, &mut w, 0.6), Running);
    assert_eq!(w.ran.last(), Some(&"chase"), "打断了等待去追");
    w.set("see", Failure);
    w.ran.clear();
    assert_eq!(tick(&mut tree, &mut w, 0.6), Running, "被打断的等待从头等");
    assert_eq!(tick(&mut tree, &mut w, 0.6), Success);
    assert_eq!(w.ran.last(), Some(&"patrolled"));
}

#[test]
fn parallels_succeed_when_enough_children_do() {
    let mut tree = Tree::new(Node::parallel(2, vec![act("a"), act("b"), act("c")]));
    let mut w = World::new();
    w.set("a", Running);
    w.set("b", Running);
    w.set("c", Success);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Running);
    assert_eq!(w.ran, ["a", "b", "c"], "每个都跑");
    w.ran.clear();
    assert_eq!(tick(&mut tree, &mut w, 0.1), Running);
    assert_eq!(w.ran, ["a", "b"], "成了的不再跑");
    w.set("a", Success);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Success);
    // 剩下的不可能凑够了：失败
    let mut tree = Tree::new(Node::parallel(2, vec![act("x"), act("y"), act("z")]));
    let mut w = World::new();
    w.set("x", Failure);
    w.set("y", Failure);
    w.set("z", Running);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Failure);
}

#[test]
fn invert_swaps_success_and_failure() {
    let mut w = World::new();
    let mut tree = Tree::new(Node::invert(act("a")));
    assert_eq!(tick(&mut tree, &mut w, 0.1), Failure);
    w.set("a", Failure);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Success);
    w.set("a", Running);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Running);
}

#[test]
fn repeat_runs_the_child_again_until_done_or_it_fails() {
    let mut w = World::new();
    let mut tree = Tree::new(Node::repeat(Some(3), act("a")));
    assert_eq!(tick(&mut tree, &mut w, 0.1), Running);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Running);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Success, "第三次成了");
    assert_eq!(w.ran.len(), 3, "一帧跑一次");
    assert_eq!(tick(&mut tree, &mut w, 0.1), Running, "成了以后重新数");
    let mut tree = Tree::new(Node::repeat(None, act("a")));
    for _ in 0..10 {
        assert_eq!(tick(&mut tree, &mut w, 0.1), Running, "一直重复");
    }
    w.set("a", Failure);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Failure, "孩子失败就失败");
}

#[test]
fn cooldown_blocks_the_child_for_a_while_after_it_finishes() {
    let mut w = World::new();
    let mut tree = Tree::new(Node::cooldown(1.0, act("shoot")));
    assert_eq!(tick(&mut tree, &mut w, 0.25), Success);
    for _ in 0..3 {
        assert_eq!(tick(&mut tree, &mut w, 0.25), Failure, "冷却中");
    }
    assert_eq!(w.ran.len(), 1);
    assert_eq!(tick(&mut tree, &mut w, 0.25), Success, "1 秒后又能用");
    // 孩子在跑的时候不算冷却；跑完（成或败）才开始冷却
    w.set("shoot", Running);
    let mut tree = Tree::new(Node::cooldown(1.0, act("shoot")));
    for _ in 0..8 {
        assert_eq!(tick(&mut tree, &mut w, 0.25), Running);
    }
    w.set("shoot", Failure);
    assert_eq!(tick(&mut tree, &mut w, 0.25), Failure);
    w.set("shoot", Success);
    assert_eq!(tick(&mut tree, &mut w, 0.25), Failure, "失败了也冷却");
    // 重复里的冷却：成了以后重复从头跑孩子，冷却照样算着
    w.set("shoot", Success);
    let mut tree = Tree::new(Node::repeat(None, Node::cooldown(1.0, act("shoot"))));
    assert_eq!(tick(&mut tree, &mut w, 0.25), Running);
    assert_eq!(tick(&mut tree, &mut w, 0.25), Failure, "冷却中");
    // 并行结束时打断孩子：冷却照样算着
    let mut tree = Tree::new(Node::parallel(1, vec![Node::cooldown(1.0, act("shoot")), act("aim")]));
    w.set("aim", Running);
    assert_eq!(tick(&mut tree, &mut w, 0.25), Success);
    assert_eq!(tick(&mut tree, &mut w, 0.25), Running, "冷却中，只剩瞄准在跑");
    // 被打断（halt）不清冷却
    let mut tree = Tree::new(Node::selector(vec![act("gate"), Node::cooldown(1.0, act("shoot"))]).reactive());
    w.set("gate", Failure);
    assert_eq!(tick(&mut tree, &mut w, 0.25), Success);
    w.set("gate", Success);
    tick(&mut tree, &mut w, 0.25);
    w.set("gate", Failure);
    assert_eq!(tick(&mut tree, &mut w, 0.25), Failure, "还在冷却");
}

#[test]
fn wait_succeeds_after_its_time() {
    let mut w = World::new();
    let mut tree = Tree::new(Node::wait(0.5));
    assert_eq!(tick(&mut tree, &mut w, 0.2), Running);
    assert_eq!(tick(&mut tree, &mut w, 0.2), Running);
    assert_eq!(tick(&mut tree, &mut w, 0.2), Success);
    assert_eq!(tick(&mut tree, &mut w, 0.2), Running, "再等一轮");
}

#[test]
fn the_blackboard_is_shared_by_leaves() {
    let mut w = World::new();
    let mut tree = Tree::new(Node::sequence(vec![Node::leaf(Leaf::Set("seen", 1.0)), Node::leaf(Leaf::Has("seen"))]));
    assert_eq!(tree.blackboard.get("seen"), None);
    assert_eq!(tick(&mut tree, &mut w, 0.1), Success);
    assert_eq!(tree.blackboard.get("seen"), Some(&Value::Num(1.0)));
    tree.blackboard.set("pos", Value::Pos(Vec2::new(1.0, 2.0)));
    assert_eq!(tree.blackboard.pos("pos"), Some(Vec2::new(1.0, 2.0)));
    assert_eq!(tree.blackboard.num("pos"), None);
    assert!(tree.blackboard.remove("pos").is_some());
    assert_eq!(Value::Bool(true).truthy(), true);
    assert_eq!(Value::Num(0.0).truthy(), false);
}

#[test]
fn trees_drive_a_state_machine_and_read_it_back() {
    // 状态机管动画（走 / 攻击），行为树决定什么时候换：两者一起用
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Anim {
        Walk,
        Attack,
    }
    let mut fsm = Fsm::new(Anim::Walk);
    #[derive(Clone)]
    enum L {
        Close,
        Attack,
        Attacking,
    }
    let mut tree = Tree::new(Node::selector(vec![
        Node::leaf(L::Attacking),
        Node::sequence(vec![Node::leaf(L::Close), Node::cooldown(1.0, Node::leaf(L::Attack))]),
    ]));
    let mut close = true;
    assert_eq!(fsm.state(), Anim::Walk);
    let run = |tree: &mut Tree<L>, fsm: &mut Fsm<Anim>, close: bool| {
        fsm.tick(0.25);
        tree.tick(0.25, &mut |l: &L, _: &mut Blackboard| match l {
            L::Close => Status::from(close),
            L::Attack => Status::from(fsm.go(Anim::Attack)),
            L::Attacking => if fsm.state() == Anim::Attack && fsm.time() < 0.5 { Running } else { Failure },
        })
    };
    assert_eq!(run(&mut tree, &mut fsm, close), Success);
    assert_eq!(fsm.state(), Anim::Attack);
    fsm.go(Anim::Walk);
    close = false;
    run(&mut tree, &mut fsm, close);
    assert_eq!(fsm.state(), Anim::Walk, "离远了不攻击");
}

#[test]
fn halted_leaves_get_a_callback() {
    // 反应式选择：前面的条件成了，打断后面在跑的 walk；整棵树打断时在跑的也收到
    let mut tree = Tree::new(Node::selector(vec![Node::leaf(Leaf::Has("seen")), Node::leaf(Leaf::Do("walk"))]).reactive());
    let mut w = World::new();
    w.set("walk", Status::Running);
    let mut halted = Vec::new();
    let mut step = |tree: &mut Tree<Leaf>, halted: &mut Vec<Leaf>| {
        tree.tick_halting(0.1, &mut |leaf: &Leaf, bb: &mut Blackboard| match leaf {
            Leaf::Do(_) => Status::Running,
            Leaf::Has(key) => Status::from(bb.get(key).is_some()),
            Leaf::Set(..) => Status::Success,
        }, &mut |leaf: &Leaf, _: &mut Blackboard| halted.push(leaf.clone()))
    };
    assert_eq!(step(&mut tree, &mut halted), Status::Running);
    assert!(halted.is_empty());
    tree.blackboard.set("seen", Value::Bool(true));
    assert_eq!(step(&mut tree, &mut halted), Status::Success);
    assert_eq!(halted, [Leaf::Do("walk")], "被打断的叶子收到一次");
    tree.blackboard.remove("seen");
    step(&mut tree, &mut halted);
    tree.halt_with(&mut |leaf: &Leaf, _: &mut Blackboard| halted.push(leaf.clone()));
    assert_eq!(halted, [Leaf::Do("walk"), Leaf::Do("walk")]);
    // 没在跑的不收
    tree.halt_with(&mut |leaf: &Leaf, _: &mut Blackboard| halted.push(leaf.clone()));
    assert_eq!(halted.len(), 2);
    let _ = w;
}

#[test]
fn the_clock_keeps_precision_after_running_for_days() {
    // 跑了好几天（f32 的秒数只剩厘秒级精度）：等 0.05 秒还是 3 帧
    let mut tree = Tree::new(Node::sequence(vec![Node::wait(0.05), Node::leaf(Leaf::Do("done"))]));
    let mut w = World::new();
    for _ in 0..(3 * 24 * 3600 / 60) {
        tree.time += 60.0;
    }
    let frames = (1..10).find(|_| tick(&mut tree, &mut w, 1.0 / 60.0) == Status::Success).unwrap();
    assert_eq!(frames, 3);
}
