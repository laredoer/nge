// 敌人的状态机：刀兵追上来砍、弓兵保持距离放箭、骑将冲锋前停一下；受击、挑飞、倒地、化烟。
import { expect, test } from "nsh/test";
import {
  ARCHER, BOSS, canBeHit, enemyHitbox, hitEnemy, MAX_ATTACKERS, newEnemy, SOLDIER, stepArrow, stepEnemy, type Ctx, type Enemy, type EnemyEvent, type Target,
} from "./enemies";
import { DT } from "./player";

const player = (x: number, d: number, extra: Partial<Target> = {}): Target => ({ x, d, h: 0, down: false, ...extra });
const ctx = (p: Target, attackers = 0): Ctx => ({ player: p, attackers, bounds: { minX: -1000, maxX: 1000 }, blocked: () => false });

function run(e: Enemy, seconds: number, c: Ctx): { e: Enemy; events: EnemyEvent[]; acts: string[] } {
  const events: EnemyEvent[] = [];
  const acts: string[] = [];
  for (let i = 0; i < Math.ceil(seconds / DT - 1e-6); i++) {
    const r = stepEnemy(e, c, DT);
    e = r.enemy;
    events.push(...r.events);
    if (acts[acts.length - 1] !== e.act) acts.push(e.act);
  }
  return { e, events, acts };
}

test("刀兵追过来：走到玩家旁边、对齐纵深，面朝玩家", () => {
  const { e } = run(newEnemy("s", "soldier", 300, 20, 0), 3, ctx(player(0, 90)));
  expect(Math.abs(e.x - SOLDIER.engage)).toBeLessThan(10);
  expect(Math.abs(e.d - 90)).toBeLessThan(8);
  expect(e.face).toBe(-1);
});

test("刀兵出刀：前摇、出手（只有这时有判定框）、收招，然后冷却", () => {
  const c = ctx(player(0, 50));
  let { e, acts } = run(newEnemy("s", "soldier", SOLDIER.engage, 50, 0), SOLDIER.windup + SOLDIER.strike / 2, c);
  expect(acts).toEqual(["windup", "attack"]);
  const box = enemyHitbox(e)!;
  expect(box.x0).toBeLessThan(0);
  expect(box.heavy).toBe(false);
  e = run(e, SOLDIER.strike, c).e;
  expect([e.act, enemyHitbox(e)]).toEqual(["recover", null]);
  e = run(e, SOLDIER.recover, c).e;
  expect(e.cooldown).toBeGreaterThan(0);
  expect(run(e, 0.2, c).acts).not.toContain("windup");
});

test("同时出刀的刀兵有上限，多出来的在外圈等着", () => {
  const { e, acts } = run(newEnemy("s", "soldier", 300, 50, 1), 3, ctx(player(0, 50), MAX_ATTACKERS));
  expect(acts).not.toContain("windup");
  expect(Math.abs(e.x)).toBeGreaterThan(SOLDIER.engage + 40);
});

test("打一下：掉血、闪白、硬直一下再追；同一招只算一次", () => {
  let e = hitEnemy(newEnemy("s", "soldier", 50, 50, 0), { damage: 10, launch: false, fromX: 0, swing: 1 });
  expect([e.hp, e.act]).toEqual([SOLDIER.hp - 10, "hurt"]);
  expect(e.flash).toBeGreaterThan(0);
  expect(hitEnemy(e, { damage: 10, launch: false, fromX: 0, swing: 1 }).hp).toBe(SOLDIER.hp - 10);
  const r = run(e, 1, ctx(player(0, 50)));
  expect(r.acts[0]).toBe("hurt");
  expect(r.acts).not.toContain("down");
  expect(r.e.x).toBeGreaterThan(50);
  expect(r.e.flash).toBe(0);
});

test("挑飞：飞起来、落地倒着、爬起来；倒着时打不到", () => {
  const e = hitEnemy(newEnemy("s", "soldier", 50, 50, 0), { damage: 10, launch: true, fromX: 0, swing: 1 });
  expect(e.act).toBe("launched");
  const r = run(e, 3, ctx(player(0, 50)));
  expect(r.acts.slice(0, 3)).toEqual(["launched", "down", "getup"]);
  const down = run(e, 0.9, ctx(player(0, 50))).e;
  expect(down.act).toBe("down");
  expect(canBeHit(down)).toBe(false);
});

test("打死：倒地、过一会儿化成烟（gone），只发一次", () => {
  const e = hitEnemy(newEnemy("s", "soldier", 50, 50, 0), { damage: 999, launch: false, fromX: 0, swing: 1 });
  expect([e.dead, e.hp]).toEqual([true, 0]);
  expect(canBeHit(e)).toBe(false);
  const r = run(e, 3, ctx(player(0, 50)));
  expect(r.acts).toContain("dead");
  expect(r.e.gone).toBe(true);
  expect(r.events.filter((x) => x.kind === "gone").length).toBe(1);
});

test("弓兵保持距离：贴近了往后退，远了往前凑，在射程里对齐纵深就拉弓放箭", () => {
  const near = run(newEnemy("a", "archer", 80, 50, 0), 1, ctx(player(0, 50))).e;
  expect(near.x).toBeGreaterThan(80);
  const far = run(newEnemy("a", "archer", 700, 50, 0), 1, ctx(player(0, 50))).e;
  expect(far.x).toBeLessThan(700);
  const r = run(newEnemy("a", "archer", 600, 100, 0), 6, ctx(player(0, 50)));
  expect(r.e.x).toBeGreaterThanOrEqual(ARCHER.near);
  expect(r.e.x).toBeLessThanOrEqual(ARCHER.far);
  expect(r.acts).toContain("draw");
  const shots = r.events.filter((x) => x.kind === "arrow");
  expect(shots.length).toBeGreaterThan(0);
  const arrow = shots[0].kind === "arrow" ? shots[0].arrow : null;
  expect(arrow!.vx).toBeLessThan(0);
  expect(Math.abs(arrow!.d - 50)).toBeLessThan(12);
  expect(stepArrow(arrow!, 0.5).x).toBeLessThan(arrow!.x - 100);
});

test("骑将冲锋前停一下：前摇时不动、发出预警和马蹄声，然后快速冲过玩家", () => {
  const c = ctx(player(0, 60));
  let r = run(newEnemy("b", "boss", 400, 60, 0), DT * 3, c);
  expect(r.e.act).toBe("chargeWindup");
  expect(r.events.map((x) => x.kind)).toEqual(["warn"]);
  const still = r.e.x;
  r = run(r.e, BOSS.chargeWindup - DT * 4, c);
  expect([r.e.act, r.e.x]).toEqual(["chargeWindup", still]);
  expect(enemyHitbox(r.e)).toBe(null);
  r = run(r.e, DT * 3, c);
  expect(r.e.act).toBe("charge");
  expect(r.events.map((x) => x.kind)).toContain("hooves");
  expect(enemyHitbox(r.e)?.heavy).toBe(true);
  r = run(r.e, 1.5, c);
  expect(r.e.x).toBeLessThan(-BOSS.overshoot + 40);
  expect(r.acts).toContain("recover");
});

test("骑将近身时横扫；冲锋中挨打不停（霸体），只掉血闪白", () => {
  const c = ctx(player(0, 60));
  const r = run(newEnemy("b", "boss", 80, 60, 0), 0.1, c);
  expect(r.e.act).toBe("windup");
  let e = run(newEnemy("b", "boss", 400, 60, 0), BOSS.chargeWindup + 0.1, c).e;
  expect(e.act).toBe("charge");
  e = hitEnemy(e, { damage: 30, launch: true, fromX: 0, swing: 1 });
  expect([e.act, e.hp]).toEqual(["charge", BOSS.hp - 30]);
  expect(e.flash).toBeGreaterThan(0);
  const calm = hitEnemy(newEnemy("b", "boss", 400, 60, 0), { damage: 10, launch: true, fromX: 0, swing: 1 });
  expect(calm.act).toBe("hurt");
});

test("玩家倒地时刀兵不上去砍", () => {
  const { acts } = run(newEnemy("s", "soldier", 300, 50, 0), 3, ctx(player(0, 50, { down: true })));
  expect(acts).not.toContain("windup");
});
