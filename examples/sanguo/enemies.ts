// 曹军：刀兵、弓兵、骑将（首领）的状态机，和弓兵放的箭。纯函数，按固定步长推进。
import { clamp, DEPTH } from "./belt";
import type { EnemyKind } from "./levels";

export const SOLDIER = { hp: 40, speed: 95, depthSpeed: 60, engage: 55, wait: 140, windup: 0.4, strike: 0.12, recover: 0.45, cooldown: 1.0, damage: 8 };
export const ARCHER = { hp: 30, speed: 85, depthSpeed: 55, near: 180, far: 340, prefer: 260, draw: 0.65, recover: 0.4, cooldown: 2.4 };
export const BOSS = {
  hp: 360, speed: 110, depthSpeed: 60, engage: 150, close: 100, chargeWindup: 0.9, chargeSpeed: 560, overshoot: 220, chargeMax: 1.8,
  chargeRecover: 0.9, windup: 0.5, strike: 0.15, recover: 0.6, cooldown: 1.6, damage: 12, chargeDamage: 18,
};
export const ARROW = { speed: 380, h: 50, damage: 7 };
/** 同时在前摇、出刀的刀兵最多几个，多的在外圈等。 */
export const MAX_ATTACKERS = 2;
/** 受击硬直、被打退多快、挑飞的初速度、倒地多久、爬起来多久、死了躺多久再化烟、闪白多久。 */
export const STAGGER = { hurt: 0.3, knock: 140, launchUp: 420, launchBack: 150, gravity: 1500, down: 0.7, getup: 0.35, dead: 0.7, flash: 0.12 };

export type EnemyAct =
  | "idle" | "walk" | "windup" | "attack" | "recover" | "draw" | "chargeWindup" | "charge"
  | "hurt" | "launched" | "down" | "getup" | "dead";

export interface Enemy {
  id: string;
  kind: EnemyKind;
  x: number;
  d: number;
  h: number;
  vh: number;
  vx: number;
  face: 1 | -1;
  act: EnemyAct;
  t: number;
  hp: number;
  maxHp: number;
  cooldown: number;
  /** 还要闪白多久。 */
  flash: number;
  /** 最后一次被玩家哪一招打中（`Player.swing`）。 */
  hitBy: number;
  dead: boolean;
  /** 化成烟了，可以删了。 */
  gone: boolean;
  /** 第几个出场的：错开等待的位置和冷却。 */
  slot: number;
  /** 冲锋冲到哪。 */
  goal: number;
}

/** 敌人眼里的玩家。 */
export interface Target {
  x: number;
  d: number;
  h: number;
  /** 倒地或倒下了：不上去砍。 */
  down: boolean;
}

export interface Ctx {
  player: Target;
  /** 正在前摇、出刀的刀兵有几个（不算自己）。 */
  attackers: number;
  /** 镜头里能待的 x 范围（弓兵退到边上就不退了，冲锋冲到边上停下）。 */
  bounds: { minX: number; maxX: number };
  blocked: (x: number, d: number) => boolean;
}

export interface Arrow {
  id: string;
  x: number;
  d: number;
  h: number;
  vx: number;
}

export type EnemyEvent = { kind: "arrow"; arrow: Arrow } | { kind: "warn" } | { kind: "hooves" } | { kind: "swing" } | { kind: "gone" };

/** 打到敌人的一下。 */
export interface Hit {
  damage: number;
  launch: boolean;
  fromX: number;
  swing: number;
}

export interface EnemyHitbox {
  x0: number;
  x1: number;
  d0: number;
  d1: number;
  h0: number;
  h1: number;
  damage: number;
  /** 重击：玩家直接倒地。 */
  heavy: boolean;
}

const STATS = { soldier: SOLDIER, archer: ARCHER, boss: BOSS };

export function newEnemy(id: string, kind: EnemyKind, x: number, d: number, slot: number): Enemy {
  const hp = STATS[kind].hp;
  return { id, kind, x, d, h: 0, vh: 0, vx: 0, face: -1, act: "walk", t: 0, hp, maxHp: hp, cooldown: 0, flash: 0, hitBy: 0, dead: false, gone: false, slot, goal: 0 };
}

/** 刀兵在前摇或出刀（占一个出刀的名额）。 */
export function isAttacking(e: Enemy): boolean {
  return e.kind === "soldier" && (e.act === "windup" || e.act === "attack");
}

export function canBeHit(e: Enemy): boolean {
  return !e.dead && e.act !== "down" && e.act !== "getup";
}

/** 霸体：骑将出招、冲锋时挨打不硬直。 */
function armored(e: Enemy): boolean {
  return e.kind === "boss" && (e.act === "windup" || e.act === "attack" || e.act === "chargeWindup" || e.act === "charge");
}

export function hitEnemy(e: Enemy, hit: Hit): Enemy {
  if (!canBeHit(e) || e.hitBy === hit.swing) return e;
  const hp = Math.max(0, e.hp - hit.damage);
  const away = hit.fromX > e.x ? -1 : 1;
  const base: Enemy = { ...e, hp, flash: STAGGER.flash, hitBy: hit.swing, t: 0 };
  if (e.kind === "boss") {
    if (hp === 0) return { ...base, dead: true, act: "dead", vx: 0 };
    return armored(e) ? { ...base, t: e.t } : { ...base, act: "hurt", vx: 0, t: STAGGER.hurt - 0.15 };
  }
  if (hp === 0) return { ...base, dead: true, act: "launched", vh: STAGGER.launchUp * 0.8, vx: away * STAGGER.launchBack };
  if (hit.launch || e.act === "launched") return { ...base, act: "launched", vh: e.act === "launched" ? Math.max(e.vh, 220) : STAGGER.launchUp, vx: away * STAGGER.launchBack };
  return { ...base, act: "hurt", vx: away * STAGGER.knock };
}

/** 往 nx 走被物件挡住时，离 d 最近的能走过去的纵深（绕路用）；附近都不通是 null。 */
export function freeDepth(nx: number, d: number, blocked: Ctx["blocked"]): number | null {
  for (let k = 1; k <= 24; k++) {
    for (const s of [1, -1]) {
      const nd = d + s * k * 6;
      if (nd >= 0 && nd <= DEPTH && !blocked(nx, nd)) return nd;
    }
  }
  return null;
}

function moveToward(e: Enemy, tx: number, wantD: number, speed: number, depthSpeed: number, dt: number, blocked: Ctx["blocked"]): Enemy {
  const dx = tx - e.x;
  let td = wantD;
  // 正前方被物件挡住：先往旁边绕
  if (Math.abs(dx) >= 2 && blocked(e.x + Math.sign(dx) * 4, e.d) && !blocked(e.x, e.d)) td = freeDepth(e.x + Math.sign(dx) * 4, e.d, blocked) ?? wantD;
  const dd = td - e.d;
  const sx = Math.abs(dx) < 2 ? 0 : Math.sign(dx) * Math.min(Math.abs(dx), speed * dt);
  const sd = Math.abs(dd) < 2 ? 0 : Math.sign(dd) * Math.min(Math.abs(dd), depthSpeed * dt);
  const x = e.x + sx;
  const d = clamp(e.d + sd, 0, DEPTH);
  let next = e;
  // 出场时落在物件里的：先不管挡不挡，走出来再说
  if (!blocked(x, d) || blocked(e.x, e.d)) next = { ...e, x, d };
  else if (!blocked(x, e.d)) next = { ...e, x };
  else if (!blocked(e.x, d)) next = { ...e, d };
  const moving = sx !== 0 || sd !== 0;
  const act = moving ? "walk" : "idle";
  return act === next.act ? next : { ...next, act, t: 0 };
}

function faceTo(e: Enemy, x: number): Enemy {
  return { ...e, face: x >= e.x ? 1 : -1 };
}

/** 受击、挑飞、倒地、死了的那几个状态：各种敌人一样。返回 null 时不在这些状态里。 */
function stagger(e: Enemy, dt: number, events: EnemyEvent[]): Enemy | null {
  switch (e.act) {
    case "hurt": {
      const x = e.x + e.vx * dt;
      const vx = e.vx * Math.exp(-dt * 8);
      return e.t >= STAGGER.hurt ? { ...e, x, vx: 0, act: "walk", t: 0 } : { ...e, x, vx };
    }
    case "launched": {
      const vh = e.vh - STAGGER.gravity * dt;
      const h = e.h + vh * dt;
      const x = e.x + e.vx * dt;
      if (h > 0) return { ...e, x, h, vh };
      return { ...e, x, h: 0, vh: 0, vx: 0, act: e.dead ? "dead" : "down", t: 0 };
    }
    case "down":
      return e.t >= STAGGER.down ? { ...e, act: "getup", t: 0 } : e;
    case "getup":
      return e.t >= STAGGER.getup ? { ...e, act: "walk", t: 0, cooldown: Math.max(e.cooldown, 0.4) } : e;
    case "dead":
      if (!e.gone && e.t >= (e.kind === "boss" ? 1.2 : STAGGER.dead)) {
        events.push({ kind: "gone" });
        return { ...e, gone: true };
      }
      return e;
    default:
      return null;
  }
}

function soldier(e: Enemy, c: Ctx, dt: number): Enemy {
  const p = c.player;
  const S = SOLDIER;
  switch (e.act) {
    case "windup":
      return e.t >= S.windup ? { ...e, act: "attack", t: 0 } : e;
    case "attack":
      return e.t >= S.strike ? { ...e, act: "recover", t: 0 } : e;
    case "recover":
      return e.t >= S.recover ? { ...e, act: "walk", t: 0, cooldown: S.cooldown + (e.slot % 3) * 0.3 } : e;
  }
  const side = e.x >= p.x ? 1 : -1;
  const engaged = c.attackers < MAX_ATTACKERS && !p.down;
  // 站位不出镜头：玩家贴着镜头边时绕到另一边去
  const tx = clamp(p.x + side * (engaged ? S.engage : S.wait + (e.slot % 2) * 30), c.bounds.minX + 20, c.bounds.maxX - 20);
  const td = clamp(p.d + (engaged ? 0 : ((e.slot % 3) - 1) * 30), 0, DEPTH);
  const ready = engaged && e.cooldown <= 0 && Math.abs(tx - e.x) < 8 && Math.abs(td - e.d) < 8;
  if (ready) return faceTo({ ...e, act: "windup", t: 0 }, p.x);
  return faceTo(moveToward(e, tx, td, S.speed, S.depthSpeed, dt, c.blocked), p.x);
}

function archer(e: Enemy, c: Ctx, dt: number, events: EnemyEvent[]): Enemy {
  const p = c.player;
  const A = ARCHER;
  switch (e.act) {
    case "draw":
      if (e.t < A.draw) return e;
      // id 由 game.ts 换成不重复的
      events.push({ kind: "arrow", arrow: { id: e.id, x: e.x + e.face * 30, d: e.d, h: ARROW.h, vx: e.face * ARROW.speed } });
      return { ...e, act: "recover", t: 0 };
    case "recover":
      return e.t >= A.recover ? { ...e, act: "walk", t: 0, cooldown: A.cooldown + (e.slot % 2) * 0.5 } : e;
  }
  const side = e.x >= p.x ? 1 : -1;
  const gap = Math.abs(e.x - p.x);
  const want = gap < A.near || gap > A.far ? p.x + side * A.prefer : e.x;
  const tx = clamp(want, c.bounds.minX + 30, c.bounds.maxX - 30);
  // 已经在镜头外面的（刚出场）先走进来
  const target = e.x < c.bounds.minX || e.x > c.bounds.maxX ? clamp(e.x, c.bounds.minX + 40, c.bounds.maxX - 40) : tx;
  const inRange = gap >= A.near - 20 && gap <= A.far + 20 && Math.abs(e.d - p.d) < 10;
  if (inRange && e.cooldown <= 0 && !p.down && e.x >= c.bounds.minX && e.x <= c.bounds.maxX) return faceTo({ ...e, act: "draw", t: 0 }, p.x);
  return faceTo(moveToward(e, target, p.d, A.speed, A.depthSpeed, dt, c.blocked), p.x);
}

function boss(e: Enemy, c: Ctx, dt: number, events: EnemyEvent[]): Enemy {
  const p = c.player;
  const B = BOSS;
  switch (e.act) {
    case "chargeWindup":
      if (e.t < B.chargeWindup) return e;
      events.push({ kind: "hooves" });
      return { ...e, act: "charge", t: 0, goal: p.x + e.face * B.overshoot };
    case "charge": {
      const x = e.x + e.face * B.chargeSpeed * dt;
      const done = (e.face > 0 ? x >= e.goal : x <= e.goal) || x < c.bounds.minX - 60 || x > c.bounds.maxX + 60 || e.t >= B.chargeMax;
      // 冲完收势比横扫久：从负的时间开始数
      return done ? { ...e, x, act: "recover", t: B.recover - B.chargeRecover, cooldown: B.cooldown } : { ...e, x };
    }
    case "windup":
      return e.t >= B.windup ? { ...e, act: "attack", t: 0 } : e;
    case "attack":
      return e.t >= B.strike ? { ...e, act: "recover", t: 0, cooldown: B.cooldown * 0.75 } : e;
    case "recover":
      return e.t >= B.recover ? { ...e, act: "walk", t: 0 } : e;
  }
  const side = e.x >= p.x ? 1 : -1;
  const gap = Math.abs(e.x - p.x);
  const aligned = Math.abs(e.d - p.d) < 14;
  if (aligned && e.cooldown <= 0 && !p.down) {
    if (gap >= B.close + 20) {
      events.push({ kind: "warn" });
      return faceTo({ ...e, act: "chargeWindup", t: 0 }, p.x);
    }
    if (gap < B.close) return faceTo({ ...e, act: "windup", t: 0 }, p.x);
  }
  const tx = clamp(p.x + side * B.engage, c.bounds.minX + 40, c.bounds.maxX - 40);
  return faceTo(moveToward(e, tx, p.d, B.speed, B.depthSpeed, dt, c.blocked), p.x);
}

export function stepEnemy(prev: Enemy, c: Ctx, dt: number): { enemy: Enemy; events: EnemyEvent[] } {
  const events: EnemyEvent[] = [];
  const e: Enemy = { ...prev, t: prev.t + dt, cooldown: Math.max(0, prev.cooldown - dt), flash: Math.max(0, prev.flash - dt) };
  const staggered = stagger(e, dt, events);
  if (staggered) return { enemy: staggered, events };
  const next = e.kind === "soldier" ? soldier(e, c, dt) : e.kind === "archer" ? archer(e, c, dt, events) : boss(e, c, dt, events);
  return { enemy: next, events };
}

/** 这一帧敌人打出去的判定框；没在出手时是 null。 */
export function enemyHitbox(e: Enemy): EnemyHitbox | null {
  const front = (near: number, far: number) => (e.face > 0 ? { x0: e.x + near, x1: e.x + far } : { x0: e.x - far, x1: e.x - near });
  if (e.kind === "soldier" && e.act === "attack") return { ...front(10, 64), d0: e.d - 12, d1: e.d + 12, h0: 0, h1: 40, damage: SOLDIER.damage, heavy: false };
  if (e.kind === "boss" && e.act === "attack") return { ...front(10, 100), d0: e.d - 16, d1: e.d + 16, h0: 0, h1: 50, damage: BOSS.damage, heavy: false };
  if (e.kind === "boss" && e.act === "charge") return { ...front(-40, 70), d0: e.d - 18, d1: e.d + 18, h0: 0, h1: 60, damage: BOSS.chargeDamage, heavy: true };
  return null;
}

export function stepArrow(a: Arrow, dt: number): Arrow {
  return { ...a, x: a.x + a.vx * dt };
}
