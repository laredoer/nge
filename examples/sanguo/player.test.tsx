// 玩家的状态机：走、三段连击的计时、突刺的冷却和怒气、跳和下劈、受击。
import { expect, test } from "nsh/test";
import { DEPTH } from "./belt";
import {
  ATTACKS, COMBO_WINDOW, DT, gainRage, hurtPlayer, isInvulnerable, newPlayer, nextStep, NO_INPUT, PLAYER_HP, playerHitbox, RAGE_MAX,
  stepPlayer, THRUST, type Area, type Input, type Player,
} from "./player";

const OPEN: Area = { minX: -1000, maxX: 1000, blocked: () => false };

/** 按住 input 走 seconds 秒；按键（attack、thrust、jump）只在第一帧按下。 */
function run(p: Player, seconds: number, input: Partial<Input> = {}, area = OPEN): { p: Player; events: string[] } {
  const events: string[] = [];
  const frames = Math.ceil(seconds / DT - 1e-6);
  for (let i = 0; i < frames; i++) {
    const once = i === 0 ? input : { x: input.x ?? 0, y: input.y ?? 0 };
    const r = stepPlayer(p, { ...NO_INPUT, ...once }, DT, area);
    p = r.player;
    events.push(...r.events);
  }
  return { p, events };
}

test("方向键左右走、上下改纵深，纵深不出带子，x 不出范围", () => {
  let { p } = run(newPlayer(0, 50), 0.5, { x: 1, y: 1 });
  expect(p.x).toBeGreaterThan(80);
  expect(p.d).toBeGreaterThan(80);
  expect(p.face).toBe(1);
  p = run(p, 2, { y: 1 }).p;
  expect(p.d).toBe(DEPTH);
  p = run(p, 3, { y: -1 }).p;
  expect(p.d).toBe(0);
  p = run(p, 0.2, { x: -1 }, { ...OPEN, minX: 90 }).p;
  expect(p.x).toBe(90);
  expect(p.face).toBe(-1);
});

test("走不进物件，但能贴着它滑过去", () => {
  const wall: Area = { ...OPEN, blocked: (x) => x > 50 };
  const { p } = run(newPlayer(0, 50), 1, { x: 1, y: 1 }, wall);
  expect(p.x).toBeLessThanOrEqual(50);
  expect(p.d).toBeGreaterThan(100);
});

test("连段：J 出第一段，攻击中再按 J，到能接的时间就接下一段，第三段把人挑飞", () => {
  let { p, events } = run(newPlayer(0, 50), DT, { attack: true });
  expect([p.act, p.step]).toEqual(["attack", 1]);
  expect(events).toEqual(["swing"]);
  // 刚出手就按：先记着，到 cancel 才接
  p = run(p, DT, { attack: true }).p;
  expect(p.step).toBe(1);
  p = run(p, ATTACKS[0].cancel).p;
  expect([p.act, p.step]).toEqual(["attack", 2]);
  p = run(p, DT, { attack: true }).p;
  p = run(p, ATTACKS[1].cancel).p;
  expect(p.step).toBe(3);
  // 打到第三段的出手时间，判定框会挑飞
  p = run(p, ATTACKS[2].hit[0]).p;
  expect(playerHitbox(p)?.launch).toBe(true);
  expect(ATTACKS[0].launch || ATTACKS[1].launch).toBe(false);
});

test("每段只在出手的那一小段时间里有判定框，框在面朝的那边", () => {
  let p = run(newPlayer(0, 50), DT, { attack: true }).p;
  expect(playerHitbox(p)).toBe(null);
  p = run(p, ATTACKS[0].hit[0]).p;
  const box = playerHitbox(p)!;
  expect(box.x0).toBeGreaterThan(p.x - 20);
  expect(box.x1).toBeGreaterThan(p.x + 50);
  expect(box.damage).toBe(ATTACKS[0].damage);
  p = run(p, ATTACKS[0].dur - ATTACKS[0].hit[0]).p;
  expect(playerHitbox(p)).toBe(null);
  expect(p.act).toBe("idle");
});

test("连段的计时：一段打完在窗口里按 J 接下一段，过了窗口从第一段重来", () => {
  expect(nextStep(1, COMBO_WINDOW)).toBe(2);
  expect(nextStep(2, 0.01)).toBe(3);
  expect(nextStep(2, 0)).toBe(1);
  expect(nextStep(3, COMBO_WINDOW)).toBe(1);
  let p = run(newPlayer(0, 50), ATTACKS[0].dur + DT, { attack: true }).p;
  expect(p.act).toBe("idle");
  const inWindow = run(p, COMBO_WINDOW / 2).p;
  expect(run(inWindow, DT, { attack: true }).p.step).toBe(2);
  const late = run(p, COMBO_WINDOW + 0.1).p;
  expect(run(late, DT, { attack: true }).p.step).toBe(1);
});

test("突刺要怒气够、冷却好：用掉怒气、往前冲、冲的时候不挨打，冷却完之前不能再刺", () => {
  let p = run(newPlayer(0, 50), DT, { thrust: true }).p;
  expect(p.act).toBe("idle");
  p = gainRage(p, THRUST.cost);
  const r = run(p, DT, { thrust: true });
  p = r.p;
  expect(r.events).toEqual(["thrust"]);
  expect([p.act, p.rage, p.cooldown]).toEqual(["thrust", 0, THRUST.cooldown]);
  expect(isInvulnerable(p)).toBe(true);
  expect(playerHitbox(p)?.kind).toBe("thrust");
  const start = p.x;
  p = run(p, THRUST.dur).p;
  expect(p.x - start).toBeGreaterThan(120);
  expect(p.act).toBe("idle");
  p = gainRage(p, RAGE_MAX * 2);
  expect(p.rage).toBe(RAGE_MAX);
  expect(run(p, DT, { thrust: true }).p.act).toBe("idle");
  p = run(p, THRUST.cooldown).p;
  expect(run(p, DT, { thrust: true }).p.act).toBe("thrust");
});

test("跳起来落回地面；空中按 J 下劈，落地那一下有一圈判定", () => {
  let { p, events } = run(newPlayer(0, 50), 0.2, { jump: true });
  expect(events).toEqual(["jump"]);
  expect(p.h).toBeGreaterThan(40);
  p = run(p, 1).p;
  expect([p.act, p.h]).toEqual(["idle", 0]);

  p = run(newPlayer(0, 50), 0.2, { jump: true }).p;
  const r = run(p, DT, { attack: true });
  expect([r.p.act, r.events]).toEqual(["plunge", ["plunge"]]);
  expect(playerHitbox(r.p)).toBe(null);
  const landed = run(r.p, 0.3);
  expect(landed.events).toContain("land");
  // 落地后的头几帧有一圈判定（前后都打到）
  let q = r.p;
  let box = null;
  for (let i = 0; i < 40 && box === null; i++) {
    q = stepPlayer(q, NO_INPUT, DT, OPEN).player;
    box = playerHitbox(q);
  }
  expect(box?.kind).toBe("plunge");
  expect(box!.x0).toBeLessThan(q.x - 40);
  expect(box!.x1).toBeGreaterThan(q.x + 40);
});

test("挨打：掉血、硬直、之后一小会儿无敌；重击倒地；血没了倒下不起", () => {
  let p = hurtPlayer(newPlayer(0, 50), 10, 50, false);
  expect([p.hp, p.act]).toEqual([PLAYER_HP - 10, "hurt"]);
  expect(isInvulnerable(p)).toBe(true);
  expect(hurtPlayer(p, 10, 50, false).hp).toBe(PLAYER_HP - 10);
  // 从右边打过来，往左退
  p = run(p, 0.2).p;
  expect(p.x).toBeLessThan(0);
  p = run(p, 1).p;
  expect([p.act, isInvulnerable(p)]).toEqual(["idle", false]);
  p = hurtPlayer(p, 20, -50, true);
  expect(p.act).toBe("down");
  p = run(p, 0.5, { x: 1 }).p;
  expect(p.act).toBe("down");
  p = run(p, 1.5).p;
  expect(p.act).toBe("idle");
  p = hurtPlayer(p, 999, 0, false);
  expect([p.hp, p.act]).toEqual([0, "dead"]);
  p = run(p, 3, { attack: true, x: 1 }).p;
  expect(p.act).toBe("dead");
});

test("挨打会攒一点怒气", () => {
  const p = hurtPlayer(newPlayer(0, 50), 10, 50, false);
  expect(p.rage).toBeGreaterThan(0);
});
