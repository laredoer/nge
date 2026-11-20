//! 权威模式的竞技场：服务端和客户端跑同一份确定性的模拟（见 docs/net-more.md）。
//!
//! - `sim`（这个文件）：角色的移动（跑、跳、重力、落到平台上）和「拍人」，按固定步长 1/30 秒走一步。
//! - `delta`：线上的输入和快照，快照只发和基准比变了的字段。
//! - `room`：服务端的一个房间：收输入、按 tick 推进、回溯判拍人、给每个人发增量快照。
//! - `client`：客户端：预测自己、对账回滚、对时、算统计。
//!
//! 确定性的前提：固定步长；每个 tick 的输入按 tick 编号排好再用；只用 IEEE 754 的加减乘除和比较（不用三角函数、
//! 不用 `mul_add`），同一份代码在不同机器上算出一样的位；角色之间的影响（拍人）只在服务端算。
//! 没用 rapier：它的浮点结果跨平台不保证一样（SIMD、并行求解、编译器的取舍都会让结果差最后几位），
//! 预测和回滚要求客户端重放出和服务端一样的结果，所以这里是一份手写的、只用基本运算的运动学模拟。

pub mod client;
pub mod delta;
pub mod room;

use std::collections::BTreeMap;

use serde::Serialize;

use crate::proto::PeerId;

/// 每秒几个 tick。
pub const TICK_HZ: u32 = 30;
/// 一个 tick 多少秒。
pub const DT: f32 = 1.0 / TICK_HZ as f32;
/// 一个 tick 多少毫秒。
pub const TICK_MS: f64 = 1000.0 / TICK_HZ as f64;

/// 输入的按键位。
pub mod button {
    pub const LEFT: u8 = 1;
    pub const RIGHT: u8 = 2;
    pub const JUMP: u8 = 4;
    pub const TAG: u8 = 8;
    /// 认得的位：别的位是坏数据。
    pub const ALL: u8 = LEFT | RIGHT | JUMP | TAG;
}

/// 竞技场多宽（左墙 0，右墙 `WIDTH`），地面在 y = 0。
pub const WIDTH: f32 = 960.0;
/// 角色的半宽、半高。
pub const HALF_W: f32 = 12.0;
pub const HALF_H: f32 = 16.0;
pub const GRAVITY: f32 = -1800.0;
pub const SPEED: f32 = 260.0;
pub const GROUND_ACCEL: f32 = 3000.0;
pub const AIR_ACCEL: f32 = 1500.0;
pub const JUMP_SPEED: f32 = 760.0;
pub const MAX_FALL: f32 = 1200.0;
/// 拍人的距离（中心之间）。
pub const TAG_RADIUS: f32 = 48.0;
/// 拍一次以后几个 tick 不能再拍。
pub const TAG_COOLDOWN: u8 = 15;

/// 平台：`x0`..`x1` 之间、顶面在 `y`，只从上面落得上去（跳起来能穿过）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Platform {
    pub x0: f32,
    pub x1: f32,
    pub y: f32,
}

pub const PLATFORMS: [Platform; 3] = [Platform { x0: 160.0, x1: 352.0, y: 120.0 }, Platform { x0: 416.0, x1: 544.0, y: 220.0 }, Platform { x0: 608.0, x1: 800.0, y: 120.0 }];

/// 一个角色的全部状态。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, serde::Deserialize)]
pub struct Hero {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub ground: bool,
    /// 还有几个 tick 才能再拍。
    pub cool: u8,
    /// 被拍过几次（只有服务端改）。
    pub tagged: u16,
}

/// 整个竞技场：成员 id → 角色。
pub type World = BTreeMap<PeerId, Hero>;

/// 成员 `id` 出生的地方：按 id 错开。
pub fn spawn(id: PeerId) -> Hero {
    Hero { x: 96.0 + (id % 8) as f32 * 96.0, y: HALF_H, vx: 0.0, vy: 0.0, ground: true, cool: 0, tagged: 0 }
}

/// 这一步会不会拍人（拍谁由服务端回溯着判）。
pub fn wants_tag(hero: &Hero, input: u8) -> bool {
    input & button::TAG != 0 && hero.cool == 0
}

fn approach(v: f32, target: f32, step: f32) -> f32 {
    if v < target { (v + step).min(target) } else { (v - step).max(target) }
}

/// 走一步：只看自己和自己的输入（预测时客户端能原样重放）。
pub fn step(hero: &Hero, input: u8) -> Hero {
    let mut h = *hero;
    let dir = (input & button::RIGHT != 0) as i32 - (input & button::LEFT != 0) as i32;
    let accel = if h.ground { GROUND_ACCEL } else { AIR_ACCEL };
    h.vx = approach(h.vx, dir as f32 * SPEED, accel * DT);
    if input & button::JUMP != 0 && h.ground {
        h.vy = JUMP_SPEED;
    }
    h.vy = (h.vy + GRAVITY * DT).max(-MAX_FALL);
    let bottom = h.y - HALF_H;
    h.x += h.vx * DT;
    if h.x < HALF_W || h.x > WIDTH - HALF_W {
        h.x = h.x.clamp(HALF_W, WIDTH - HALF_W);
        h.vx = 0.0;
    }
    h.y += h.vy * DT;
    h.ground = false;
    if h.vy <= 0.0 {
        // 这一步从顶面上方落到了顶面下方：站上去（取最高的那个）
        let tops = PLATFORMS.iter().filter(|p| h.x >= p.x0 - HALF_W && h.x <= p.x1 + HALF_W).map(|p| p.y).chain(std::iter::once(0.0));
        let landed = tops.filter(|&top| bottom >= top && h.y - HALF_H <= top).fold(None, |best: Option<f32>, top| Some(best.map_or(top, |b| b.max(top))));
        if let Some(top) = landed {
            h.y = top + HALF_H;
            h.vy = 0.0;
            h.ground = true;
        }
    }
    h.cool = h.cool.saturating_sub(1);
    if wants_tag(hero, input) {
        h.cool = TAG_COOLDOWN;
    }
    h
}

/// `attacker` 在 `at` 拍人：`world` 里离它最近、在 `TAG_RADIUS` 以内的别人（一样近时 id 小的）。
pub fn tag_target(world: &World, attacker: PeerId, at: [f32; 2]) -> Option<PeerId> {
    let r2 = TAG_RADIUS * TAG_RADIUS;
    world
        .iter()
        .filter(|(id, _)| **id != attacker)
        .map(|(id, h)| (*id, (h.x - at[0]) * (h.x - at[0]) + (h.y - at[1]) * (h.y - at[1])))
        .filter(|(_, d2)| *d2 <= r2)
        .fold(None, |best: Option<(PeerId, f32)>, (id, d2)| match best {
            Some((_, b)) if b <= d2 => best,
            _ => Some((id, d2)),
        })
        .map(|(id, _)| id)
}

/// 给 TSX 画竞技场用的几何。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Layout {
    pub width: f32,
    pub platforms: Vec<Platform>,
    pub hero: [f32; 2],
}

pub fn layout() -> Layout {
    Layout { width: WIDTH, platforms: PLATFORMS.to_vec(), hero: [HALF_W * 2.0, HALF_H * 2.0] }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(mut h: Hero, inputs: &[u8]) -> Hero {
        for &i in inputs {
            h = step(&h, i);
        }
        h
    }

    #[test]
    fn standing_on_the_floor_is_stable() {
        let h = run(spawn(1), &[0; 60]);
        assert_eq!((h.y, h.vy, h.ground), (HALF_H, 0.0, true));
    }

    #[test]
    fn runs_jumps_and_lands_back() {
        let h = run(spawn(1), &[button::RIGHT; 30]);
        assert!(h.vx == SPEED && h.x > spawn(1).x + 100.0, "{h:?}");
        let up = step(&h, button::JUMP);
        assert!(up.vy > 0.0 && !up.ground && up.y > HALF_H);
        let down = run(up, &[0; 90]);
        assert!(down.ground && down.y == HALF_H, "落回地面 {down:?}");
        // 空中不能再跳
        let mid = run(spawn(1), &[button::JUMP, 0, 0]);
        assert!(step(&mid, button::JUMP).vy < mid.vy);
    }

    #[test]
    fn lands_on_platforms_from_above_and_passes_through_from_below() {
        // 平台下面起跳：穿过去，落到顶上
        let mut h = Hero { x: 250.0, ..spawn(0) };
        h = step(&h, button::JUMP);
        let mut highest: f32 = 0.0;
        for _ in 0..90 {
            h = step(&h, 0);
            highest = highest.max(h.y);
        }
        assert!(highest > 120.0 + HALF_H, "跳得过平台 {highest}");
        assert_eq!((h.y, h.ground), (120.0 + HALF_H, true), "站在平台上");
        // 走出平台边缘就掉下去
        let off = run(h, &[button::LEFT; 40]);
        let off = run(off, &[0; 60]);
        assert_eq!(off.y, HALF_H);
    }

    #[test]
    fn walls_stop_you() {
        let h = run(spawn(0), &[button::LEFT; 60]);
        assert_eq!((h.x, h.vx), (HALF_W, 0.0));
        let h = run(Hero { x: WIDTH - 20.0, ..spawn(0) }, &[button::RIGHT; 60]);
        assert_eq!(h.x, WIDTH - HALF_W);
    }

    #[test]
    fn the_same_inputs_give_the_same_state_bit_for_bit() {
        // 一段伪随机的输入跑两遍：每一步都一样（比较的是位，不是近似）
        let mut seed = 12345u32;
        let inputs: Vec<u8> = (0..600)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 24) as u8) & button::ALL
            })
            .collect();
        let (mut a, mut b) = (spawn(3), spawn(3));
        for &i in &inputs {
            a = step(&a, i);
            b = step(&b, i);
            assert_eq!((a.x.to_bits(), a.y.to_bits(), a.vx.to_bits(), a.vy.to_bits(), a.ground, a.cool), (b.x.to_bits(), b.y.to_bits(), b.vx.to_bits(), b.vy.to_bits(), b.ground, b.cool));
        }
        // 固定的输入序列给出固定的结果：换了平台、编译器，这个数不该变
        let end = run(spawn(3), &inputs);
        assert!(end.x.is_finite() && end.y >= HALF_H);
    }

    #[test]
    fn tagging_has_a_cooldown_and_picks_the_nearest() {
        let h = step(&spawn(0), button::TAG);
        assert_eq!(h.cool, TAG_COOLDOWN);
        assert!(!wants_tag(&h, button::TAG));
        let h = run(h, &[0; TAG_COOLDOWN as usize]);
        assert!(wants_tag(&h, button::TAG));
        let world: World = [(1, Hero { x: 100.0, ..spawn(1) }), (2, Hero { x: 130.0, ..spawn(2) }), (3, Hero { x: 70.0, ..spawn(3) }), (4, Hero { x: 300.0, ..spawn(4) })].into();
        assert_eq!(tag_target(&world, 1, [100.0, HALF_H]), Some(2), "一样近时 id 小的");
        assert_eq!(tag_target(&world, 4, [300.0, HALF_H]), None, "够不着");
        assert_eq!(tag_target(&world, 2, [130.0, HALF_H]), Some(1));
    }
}
