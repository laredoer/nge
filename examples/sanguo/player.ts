// 玩家（白袍枪将）的状态机：纯函数，按固定步长推进。
import { clamp, DEPTH } from "./belt";

/** 固定步长（秒）。 */
export const DT = 1 / 60;
export const PLAYER_HP = 100;
export const RAGE_MAX = 100;
/** 走路：左右、纵深每秒多少。 */
export const SPEED = 220;
export const DEPTH_SPEED = 130;
export const GRAVITY = 1700;
export const JUMP_V = 560;
export const PLUNGE_V = 900;

/**
 * 三段连击。dur 一段多长；hit 是出手（有判定框）的时间段；cancel 之后才能接下一段（之前按的先记着）；
 * reach 往前打多远。第三段把人挑飞。
 */
export interface AttackStep {
  dur: number;
  hit: [number, number];
  cancel: number;
  reach: number;
  damage: number;
  launch: boolean;
}

export const ATTACKS: AttackStep[] = [
  { dur: 0.3, hit: [0.07, 0.15], cancel: 0.18, reach: 82, damage: 10, launch: false },
  { dur: 0.32, hit: [0.08, 0.17], cancel: 0.2, reach: 86, damage: 10, launch: false },
  { dur: 0.5, hit: [0.12, 0.24], cancel: 0.5, reach: 92, damage: 18, launch: true },
];
/** 一段打完以后多久之内按 J 还能接下一段。 */
export const COMBO_WINDOW = 0.35;

/** 突刺：花多少怒气、冷却几秒、持续多久（前 dash 秒在冲）、冲多快、伤害。 */
export const THRUST = { cost: 35, cooldown: 1.5, dur: 0.32, dash: 0.24, speed: 720, damage: 24 };
/** 下劈落地：判定持续多久、落地硬直多久、伤害、前后打多远。 */
export const PLUNGE = { active: 0.08, recover: 0.25, damage: 20, radius: 70 };
/** 受击硬直、倒地多久（重击）、之后还有多久无敌、被打退多快。 */
export const HURT = { stun: 0.35, down: 1.2, grace: 0.5, knock: 180 };

export type PlayerAct = "idle" | "walk" | "attack" | "thrust" | "jump" | "plunge" | "land" | "hurt" | "down" | "dead";

export interface Player {
  x: number;
  d: number;
  h: number;
  /** 竖直速度（跳）。 */
  vh: number;
  /** 被打退、突刺的水平速度。 */
  vx: number;
  face: 1 | -1;
  act: PlayerAct;
  /** 进这个动作多久了。 */
  t: number;
  /** 连击第几段（1–3）。 */
  step: number;
  /** 攻击中又按了 J。 */
  buffered: boolean;
  /** 上一段打完的是第几段、还剩多久能接。 */
  chain: number;
  chainLeft: number;
  hp: number;
  rage: number;
  /** 突刺还要冷却多久。 */
  cooldown: number;
  /** 还有多久无敌。 */
  invuln: number;
  /** 每出一招加一：同一招对同一个敌人只算一次。 */
  swing: number;
}

export interface Input {
  /** -1 左、1 右。 */
  x: number;
  /** -1 往外（下）、1 往里（上）。 */
  y: number;
  attack: boolean;
  thrust: boolean;
  jump: boolean;
}

export const NO_INPUT: Input = { x: 0, y: 0, attack: false, thrust: false, jump: false };

/** 能走到哪：x 的范围、(x, d) 有没有被物件挡住。 */
export interface Area {
  minX: number;
  maxX: number;
  blocked: (x: number, d: number) => boolean;
}

export type PlayerEvent = "swing" | "thrust" | "jump" | "plunge" | "land";

/** 判定框（带子坐标）。 */
export interface Hitbox {
  x0: number;
  x1: number;
  d0: number;
  d1: number;
  h0: number;
  h1: number;
  damage: number;
  launch: boolean;
  kind: "combo" | "thrust" | "plunge";
  swing: number;
}

export function newPlayer(x: number, d: number): Player {
  return { x, d, h: 0, vh: 0, vx: 0, face: 1, act: "idle", t: 0, step: 0, buffered: false, chain: 0, chainLeft: 0, hp: PLAYER_HP, rage: 0, cooldown: 0, invuln: 0, swing: 0 };
}

/** 刚打完第 chain 段、还剩 chainLeft 秒时按 J：在窗口里接下一段，第三段之后或过了窗口回到第一段。 */
export function nextStep(chain: number, chainLeft: number): number {
  return chainLeft > 0 && chain >= 1 && chain < 3 ? chain + 1 : 1;
}

export function isInvulnerable(p: Player): boolean {
  return p.invuln > 0 || p.act === "thrust" || p.act === "down" || p.act === "dead";
}

export function gainRage(p: Player, amount: number): Player {
  return { ...p, rage: Math.min(RAGE_MAX, p.rage + amount) };
}

/** 挨了一下：无敌时不算。heavy（骑将冲锋）直接倒地。血没了倒下不起。fromX 是打过来的那一边。 */
export function hurtPlayer(p: Player, damage: number, fromX: number, heavy: boolean): Player {
  if (isInvulnerable(p)) return p;
  const hp = Math.max(0, p.hp - damage);
  const away = fromX > p.x ? -1 : 1;
  const base = { ...gainRage(p, 4), hp, t: 0, vx: away * HURT.knock, buffered: false, step: 0 };
  if (hp === 0) return { ...base, act: "dead", h: 0, vh: 0 };
  if (heavy) return { ...base, act: "down", vx: away * HURT.knock * 1.5, invuln: HURT.down + HURT.grace };
  return { ...base, act: "hurt", invuln: HURT.stun + HURT.grace };
}

function startAttack(p: Player, step: number): Player {
  return { ...p, act: "attack", t: 0, step, buffered: false, swing: p.swing + 1, chainLeft: 0 };
}

/** 左右、纵深挪一步：出了范围截住，被物件挡住时分开试两个方向（贴着滑过去）。 */
function moveBy(p: Player, dx: number, dd: number, area: Area): Player {
  const x = clamp(p.x + dx, area.minX, area.maxX);
  const d = clamp(p.d + dd, 0, DEPTH);
  if (!area.blocked(x, d)) return { ...p, x, d };
  if (!area.blocked(x, p.d)) return { ...p, x };
  if (!area.blocked(p.x, d)) return { ...p, d };
  return p;
}

/** 推进一步。返回新的玩家和这一步发生的事（出招、起跳、落地，给音效用）。 */
export function stepPlayer(prev: Player, input: Input, dt: number, area: Area): { player: Player; events: PlayerEvent[] } {
  const events: PlayerEvent[] = [];
  let p: Player = { ...prev, t: prev.t + dt, cooldown: Math.max(0, prev.cooldown - dt), invuln: Math.max(0, prev.invuln - dt) };
  // 被打退、突刺冲出去的那一下
  if (p.vx !== 0 && p.act !== "thrust") {
    p = moveBy(p, p.vx * dt, 0, area);
    p.vx = Math.abs(p.vx) < 10 ? 0 : p.vx * Math.exp(-dt * 10);
  }

  const free = p.act === "idle" || p.act === "walk";
  if (free) {
    p.chainLeft = Math.max(0, p.chainLeft - dt);
    if (input.thrust && p.cooldown <= 0 && p.rage >= THRUST.cost) {
      events.push("thrust");
      return { player: { ...p, act: "thrust", t: 0, rage: p.rage - THRUST.cost, cooldown: THRUST.cooldown, swing: p.swing + 1, vx: 0 }, events };
    }
    if (input.jump) {
      events.push("jump");
      return { player: { ...p, act: "jump", t: 0, vh: JUMP_V, h: JUMP_V * dt }, events };
    }
    if (input.attack) {
      events.push("swing");
      return { player: startAttack(p, nextStep(p.chain, p.chainLeft)), events };
    }
    if (input.x !== 0) p.face = input.x > 0 ? 1 : -1;
    p = moveBy(p, input.x * SPEED * dt, input.y * DEPTH_SPEED * dt, area);
    const act = input.x !== 0 || input.y !== 0 ? "walk" : "idle";
    if (act !== p.act) p = { ...p, act, t: 0 };
    return { player: p, events };
  }

  switch (p.act) {
    case "attack": {
      const a = ATTACKS[p.step - 1];
      if (input.attack && p.step < 3) p.buffered = true;
      // 出手时往前带一小步
      if (p.t >= a.hit[0] && p.t <= a.hit[1]) p = moveBy(p, p.face * 60 * dt, 0, area);
      if (input.thrust && p.cooldown <= 0 && p.rage >= THRUST.cost && p.t >= a.hit[1]) {
        events.push("thrust");
        return { player: { ...p, act: "thrust", t: 0, rage: p.rage - THRUST.cost, cooldown: THRUST.cooldown, swing: p.swing + 1 }, events };
      }
      if (p.buffered && p.t >= a.cancel && p.step < 3) {
        events.push("swing");
        return { player: startAttack(p, p.step + 1), events };
      }
      if (p.t >= a.dur) p = { ...p, act: "idle", t: 0, chain: p.step === 3 ? 0 : p.step, chainLeft: p.step === 3 ? 0 : COMBO_WINDOW, buffered: false };
      return { player: p, events };
    }
    case "thrust":
      if (p.t <= THRUST.dash) p = moveBy(p, p.face * THRUST.speed * dt, 0, area);
      if (p.t >= THRUST.dur) p = { ...p, act: "idle", t: 0 };
      return { player: p, events };
    case "jump":
    case "plunge": {
      if (p.act === "jump" && input.attack) {
        events.push("plunge");
        p = { ...p, act: "plunge", t: 0, vh: -PLUNGE_V, swing: p.swing + 1 };
      }
      if (p.act === "jump") {
        if (input.x !== 0) p.face = input.x > 0 ? 1 : -1;
        p = moveBy(p, input.x * SPEED * 0.8 * dt, input.y * DEPTH_SPEED * 0.5 * dt, area);
        p.vh -= GRAVITY * dt;
      }
      p.h += p.vh * dt;
      if (p.h <= 0) {
        events.push("land");
        p = p.act === "plunge" ? { ...p, act: "land", t: 0, h: 0, vh: 0 } : { ...p, act: "idle", t: 0, h: 0, vh: 0 };
      }
      return { player: p, events };
    }
    case "land":
      if (p.t >= PLUNGE.recover) p = { ...p, act: "idle", t: 0 };
      return { player: p, events };
    case "hurt":
      if (p.t >= HURT.stun) p = { ...p, act: "idle", t: 0 };
      return { player: p, events };
    case "down":
      if (p.t >= HURT.down) p = { ...p, act: "idle", t: 0 };
      return { player: p, events };
    default:
      return { player: p, events };
  }
}

/** 这一帧玩家打出去的判定框；不在出手的时间里是 null。 */
export function playerHitbox(p: Player): Hitbox | null {
  const front = (near: number, far: number) =>
    p.face > 0 ? { x0: p.x + near, x1: p.x + far } : { x0: p.x - far, x1: p.x - near };
  if (p.act === "attack") {
    const a = ATTACKS[p.step - 1];
    if (p.t < a.hit[0] || p.t > a.hit[1]) return null;
    return { ...front(-10, a.reach), d0: p.d - 16, d1: p.d + 16, h0: p.h, h1: p.h + 90, damage: a.damage, launch: a.launch, kind: "combo", swing: p.swing };
  }
  if (p.act === "thrust" && p.t <= THRUST.dash + 0.04) {
    return { ...front(-30, 70), d0: p.d - 18, d1: p.d + 18, h0: 0, h1: 90, damage: THRUST.damage, launch: false, kind: "thrust", swing: p.swing };
  }
  if (p.act === "land" && p.t <= PLUNGE.active) {
    return { x0: p.x - PLUNGE.radius, x1: p.x + PLUNGE.radius, d0: p.d - 24, d1: p.d + 24, h0: 0, h1: 60, damage: PLUNGE.damage, launch: true, kind: "plunge", swing: p.swing };
  }
  return null;
}
