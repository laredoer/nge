// 一局游戏：顿帧、刷怪和锁镜头、打死化烟、胜负判定；一个按键的机器人从开头打到胜利。
import { expect, test } from "nsh/test";
import { VIEW_HALF } from "./belt";
import { freeDepth, newEnemy } from "./enemies";
import { bossOf, END_DELAY, newGame, step, type Game, type GameEvent } from "./game";
import { blocked, WAVES } from "./levels";
import { ATTACKS, DT, NO_INPUT, THRUST, type Input } from "./player";
import { lockOf } from "./waves";

function run(g: Game, seconds: number, input: (g: Game) => Input = () => NO_INPUT) {
  const events: GameEvent[] = [];
  for (let i = 0; i < Math.ceil(seconds / DT - 1e-6); i++) {
    const r = step(g, input(g), DT);
    g = r.game;
    events.push(...r.events);
  }
  return { g, events };
}

test("开局：在左边，满血，镜头在玩家附近，没有敌人", () => {
  const g = newGame();
  expect([g.phase, g.enemies.length, g.kills]).toEqual(["play", 0, 0]);
  expect(Math.abs(g.cam - g.player.x)).toBeLessThan(VIEW_HALF);
});

test("顿帧：整局停住，按键不动；停完接着走", () => {
  const g = { ...newGame(), hitstop: 0.05 };
  const right = () => ({ ...NO_INPUT, x: 1 });
  const frozen = run(g, 0.03, right).g;
  expect([frozen.player.x, frozen.time]).toEqual([g.player.x, 0]);
  const moving = run(frozen, 0.2, right).g;
  expect(moving.player.x).toBeGreaterThan(g.player.x);
});

test("打中敌人：击中声、震屏、顿帧、火星；打死了记斩敌数，倒地后化成烟", () => {
  const g0 = newGame();
  const g: Game = { ...g0, enemies: [{ ...newEnemy("s", "soldier", g0.player.x + 50, g0.player.d, 0), hp: 5, cooldown: 5 }] };
  let r = run(g, ATTACKS[0].hit[0] + 0.05, (s) => ({ ...NO_INPUT, attack: s.player.act === "idle" }));
  const kinds = r.events.map((e) => e.kind === "sound" ? `sound:${e.name}` : e.kind);
  expect(kinds).toContain("sound:swing");
  expect(kinds).toContain("sound:hit");
  expect(kinds).toContain("shake");
  expect(kinds).toContain("kill");
  expect(r.g.kills).toBe(1);
  expect(r.g.hitstop).toBeGreaterThan(0);
  expect(r.g.fx.map((f) => f.name)).toContain("spark");
  r = run(r.g, 3);
  expect(r.g.enemies).toEqual([]);
  expect(r.events.some((e) => e.kind === "burst" && e.smoke)).toBe(true);
});

test("走到第一个触发点：刷兵、镜头锁住，玩家走不出锁定区", () => {
  const right = () => ({ ...NO_INPUT, x: 1 });
  let { g } = run(newGame(), 3, right);
  expect(g.waves.active).toBe(0);
  expect(g.enemies.length).toBeGreaterThan(0);
  const lock = lockOf(WAVES[0].at);
  g = run(g, 5, right).g;
  expect(g.player.x).toBeLessThanOrEqual(lock.right);
  expect(Math.abs(g.cam - (lock.left + lock.right) / 2)).toBeLessThan(5);
});

test("血没了：倒下，过一会儿出失败画面，只放一次失败的声音", () => {
  const g0 = newGame();
  const g: Game = { ...g0, player: { ...g0.player, hp: 1 }, enemies: [{ ...newEnemy("s", "soldier", g0.player.x + 50, g0.player.d, 0), act: "attack", face: -1 }] };
  const r = run(g, END_DELAY.lost + 0.5);
  expect(r.g.player.act).toBe("dead");
  expect(r.g.phase).toBe("lost");
  expect(r.events.filter((e) => e.kind === "sound" && e.name === "lose").length).toBe(1);
  // 结束以后不再变
  const after = run(r.g, 1, () => ({ ...NO_INPUT, x: 1, attack: true }));
  expect(after.g.player.x).toBe(r.g.player.x);
  expect(after.events).toEqual([]);
});

test("最后一波打完、敌人化成烟：过一会儿出胜利画面", () => {
  const g0 = newGame();
  const g: Game = { ...g0, waves: { ...g0.waves, next: WAVES.length, done: true } };
  const r = run(g, END_DELAY.won + 0.1);
  expect(r.g.phase).toBe("won");
  expect(r.events.filter((e) => e.kind === "sound" && e.name === "win").length).toBe(1);
  // 还有敌人没清完时不算赢
  const busy = run({ ...g, enemies: [newEnemy("s", "soldier", 2000, 50, 0)] }, END_DELAY.won + 0.1).g;
  expect(busy.phase).toBe("play");
});

/** 简单的机器人：没敌人就沿着带子中间往右走；有就找最近的，对齐纵深、站到旁边连按 J，怒气够了突刺；骑将前摇时往旁边闪。 */
function bot(g: Game): Input {
  const p = g.player;
  const frame = Math.round(g.time / DT);
  const foes = g.enemies.filter((e) => !e.dead && e.x > g.cam - VIEW_HALF - 120 && e.x < g.cam + VIEW_HALF + 120);
  if (foes.length === 0) return { ...NO_INPUT, x: 1, y: p.d < 60 ? 1 : p.d > 70 ? -1 : 0 };
  const boss = bossOf(g);
  if (boss && !boss.dead && (boss.act === "chargeWindup" || boss.act === "charge") && Math.abs(boss.d - p.d) < 50) {
    return { ...NO_INPUT, y: p.d > 65 ? -1 : 1 };
  }
  const t = foes.reduce((a, b) => (Math.abs(b.x - p.x) + 2 * Math.abs(b.d - p.d) < Math.abs(a.x - p.x) + 2 * Math.abs(a.d - p.d) ? b : a));
  const dx = t.x - p.x;
  const dd = t.d - p.d;
  const side = dx >= 0 ? 1 : -1;
  const reach = t.kind === "boss" ? 85 : 60;
  const close = Math.abs(dd) < 10 && Math.abs(dx) < reach + 25;
  if (close) {
    if (p.face !== side && (p.act === "idle" || p.act === "walk")) return { ...NO_INPUT, x: side };
    const thrust = p.rage >= THRUST.cost && p.cooldown <= 0;
    return { ...NO_INPUT, attack: !thrust && frame % 6 === 0, thrust };
  }
  const wantX = t.x - side * reach;
  const x = Math.abs(wantX - p.x) > 8 ? Math.sign(wantX - p.x) : 0;
  // 前面有粮车、拒马挡着就先绕
  const around = x !== 0 && blocked(p.x + x * 6, p.d) ? freeDepth(p.x + x * 6, p.d, blocked) : null;
  const y = around !== null ? Math.sign(around - p.d) : Math.abs(dd) > 5 ? Math.sign(dd) : 0;
  return { ...NO_INPUT, x, y };
}

// 打倒骑将（首领战的平衡）放到第二轮：现在只验证一路清到首领那一波
test("机器人从开头一路打到长坂桥：前面每一波都刷出来、清掉，骑将出场", () => {
  let g = newGame();
  const events: GameEvent[] = [];
  for (let i = 0; i < 60 * 60 * 4 && g.phase === "play" && bossOf(g) === null; i++) {
    const r = step(g, bot(g), DT);
    g = r.game;
    events.push(...r.events);
  }
  expect(bossOf(g) === null).toBe(false);
  expect(g.waves.cleared).toBe(WAVES.length - 1);
  expect(g.kills).toBe(WAVES.slice(0, -1).reduce((n, w) => n + w.spawns.length, 0));
  expect(events.some((e) => e.kind === "sound" && e.name === "arrow")).toBe(true);
});
