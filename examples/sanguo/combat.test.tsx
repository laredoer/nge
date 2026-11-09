// 谁打到了谁：玩家的判定框打敌人、打掉箭；敌人的刀、骑将的冲锋、箭打玩家。
import { expect, test } from "nsh/test";
import { hitstopFor, RAGE_PER_HIT, resolveEnemyAttacks, resolvePlayerAttack } from "./combat";
import { newEnemy, type Arrow, type Enemy } from "./enemies";
import { ATTACKS, newPlayer, PLAYER_HP, type Player } from "./player";

/** 正在出第 step 段、在出手时间里的玩家。 */
function swinging(step: number, x = 0, d = 50, swing = 1): Player {
  return { ...newPlayer(x, d), act: "attack", step, t: ATTACKS[step - 1].hit[0] + 0.01, swing };
}

test("一招打中前面的敌人，同一招只算一次，每打中一个攒怒气", () => {
  const enemies = [newEnemy("a", "soldier", 60, 52, 0), newEnemy("b", "soldier", 70, 40, 1), newEnemy("c", "soldier", -60, 50, 2), newEnemy("d", "soldier", 60, 100, 3)];
  const r = resolvePlayerAttack(swinging(1), enemies, []);
  expect(r.hits.map((h) => h.id)).toEqual(["a", "b"]);
  expect(r.enemies.map((e) => e.hp)).toEqual([30, 30, 40, 40]);
  expect(r.player.rage).toBe(2 * RAGE_PER_HIT);
  const again = resolvePlayerAttack(r.player, r.enemies, []);
  expect(again.hits).toEqual([]);
  // 下一招又能打
  const next = resolvePlayerAttack({ ...swinging(2), swing: 2, rage: r.player.rage }, r.enemies, []);
  expect(next.hits.map((h) => h.id)).toEqual(["a", "b"]);
});

test("第三段挑飞；打死的标出来", () => {
  const r = resolvePlayerAttack(swinging(3), [newEnemy("a", "soldier", 60, 50, 0), { ...newEnemy("b", "soldier", 50, 50, 1), hp: 5 }], []);
  expect(r.enemies.map((e) => e.act)).toEqual(["launched", "launched"]);
  expect(r.hits.map((h) => [h.id, h.launch, h.killed])).toEqual([["a", true, false], ["b", true, true]]);
});

test("箭能被打掉", () => {
  const arrows: Arrow[] = [{ id: "x", x: 50, d: 50, h: 50, vx: -380 }, { id: "y", x: 50, d: 120, h: 50, vx: -380 }];
  const r = resolvePlayerAttack(swinging(1), [], arrows);
  expect(r.arrows.map((a) => a.id)).toEqual(["y"]);
  expect(r.parried.map((a) => a.id)).toEqual(["x"]);
});

test("刀兵出刀砍中玩家；玩家无敌时不算", () => {
  const e: Enemy = { ...newEnemy("s", "soldier", 50, 50, 0), act: "attack", face: -1 };
  const r = resolveEnemyAttacks(newPlayer(0, 50), [e], []);
  expect(r.player.hp).toBe(PLAYER_HP - 8);
  expect(r.player.act).toBe("hurt");
  expect(r.hurt?.heavy).toBe(false);
  const again = resolveEnemyAttacks(r.player, [e], []);
  expect(again.player.hp).toBe(PLAYER_HP - 8);
  expect(again.hurt).toBe(null);
  // 纵深差得远砍不到
  expect(resolveEnemyAttacks(newPlayer(0, 90), [e], []).hurt).toBe(null);
});

test("骑将冲锋撞到是重击，玩家倒地", () => {
  const e: Enemy = { ...newEnemy("b", "boss", 30, 50, 0), act: "charge", face: -1 };
  const r = resolveEnemyAttacks(newPlayer(0, 50), [e], []);
  expect([r.player.act, r.hurt?.heavy]).toEqual(["down", true]);
});

test("箭射中玩家就没了；跳得够高躲过去，箭接着飞", () => {
  const arrow: Arrow = { id: "x", x: 5, d: 52, h: 50, vx: -380 };
  const hit = resolveEnemyAttacks(newPlayer(0, 50), [], [arrow]);
  expect(hit.player.hp).toBeLessThan(PLAYER_HP);
  expect(hit.arrows).toEqual([]);
  const jumped = resolveEnemyAttacks({ ...newPlayer(0, 50), act: "jump", h: 90 }, [], [arrow]);
  expect(jumped.player.hp).toBe(PLAYER_HP);
  expect(jumped.arrows.length).toBe(1);
});

test("顿帧：挑飞、下劈比普通一下顿得久，突刺穿过人群顿得短", () => {
  expect(hitstopFor("combo", false)).toBeGreaterThan(0);
  expect(hitstopFor("combo", true)).toBeGreaterThan(hitstopFor("combo", false));
  expect(hitstopFor("plunge", true)).toBeGreaterThan(hitstopFor("combo", false));
  expect(hitstopFor("thrust", false)).toBeLessThan(hitstopFor("combo", false));
});
