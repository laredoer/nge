// 波次推进和镜头：走到触发点开打、镜头锁住、按时间放兵、清完解锁、全部打完；镜头跟着玩家只往右走。
import { expect, test } from "nsh/test";
import { VIEW_HALF } from "./belt";
import { BRIDGE_X, blocked, FOOTPRINTS, PROPS, stageAt, STAGES, WAVES, WORLD } from "./levels";
import { DT } from "./player";
import { cameraTarget, followCamera, lockOf, newWaves, playerRange, spawnX, stepWaves, type WaveState } from "./waves";

function run(w: WaveState, x: number, alive: number, seconds: number) {
  const spawned = [];
  for (let i = 0; i < Math.ceil(seconds / DT - 1e-6); i++) {
    const r = stepWaves(w, x, alive, DT);
    w = r.waves;
    spawned.push(...r.spawns);
  }
  return { w, spawned };
}

test("三段战场，每段两三波，最后一波有骑将，在长坂桥头", () => {
  expect(STAGES.length).toBe(3);
  for (const st of STAGES) {
    expect(st.waves.length).toBeGreaterThanOrEqual(2);
    expect(st.waves.length).toBeLessThanOrEqual(3);
  }
  const last = WAVES[WAVES.length - 1];
  expect(last.spawns.filter((s) => s.kind === "boss").length).toBe(1);
  expect(WAVES.slice(0, -1).every((w) => w.spawns.every((s) => s.kind !== "boss"))).toBe(true);
  expect(Math.abs(last.at - BRIDGE_X)).toBeLessThan(200);
  // 触发点从左往右排，都在世界里
  WAVES.forEach((w, i) => i > 0 && expect(w.at).toBeGreaterThan(WAVES[i - 1].at));
  expect(stageAt(0)).toBe(0);
  expect(stageAt(STAGES[1].from + 1)).toBe(1);
  expect(stageAt(WORLD.right)).toBe(2);
});

test("走到触发点开打：镜头锁住，按 delay 放兵，兵从锁定区外面进来", () => {
  let { w, spawned } = run(newWaves(), WAVES[0].at - 10, 0, 1);
  expect([w.active, w.lock, spawned.length]).toEqual([null, null, 0]);
  ({ w, spawned } = run(w, WAVES[0].at, 0, DT));
  expect(w.active).toBe(0);
  expect(w.lock).toEqual(lockOf(WAVES[0].at));
  const all = WAVES[0].spawns.length;
  const first = spawned.length;
  expect(first).toBeGreaterThan(0);
  expect(first).toBeLessThan(all);
  const later = run(w, WAVES[0].at, first, 3);
  expect(first + later.spawned.length).toBe(all);
  const lock = w.lock!;
  for (const s of [...spawned, ...later.spawned]) {
    const x = spawnX(s, lock);
    expect(x < lock.left || x > lock.right).toBe(true);
  }
});

test("敌人清完才解锁，接着等下一波；最后一波打完算全部打完", () => {
  let w = run(newWaves(), WAVES[0].at, 0, DT).w;
  w = run(w, WAVES[0].at, 3, 5).w;
  expect(w.active).toBe(0);
  w = run(w, WAVES[0].at, 0, DT).w;
  expect([w.active, w.lock, w.next, w.done]).toEqual([null, null, 1, false]);
  expect(w.cleared).toBe(1);
  for (let i = 1; i < WAVES.length; i++) {
    w = run(w, WAVES[i].at, 0, 4).w;
  }
  expect(w.done).toBe(true);
  expect(w.cleared).toBe(WAVES.length);
});

test("没出完的兵也算：兵没放完就算场上没人，也不解锁", () => {
  let w = run(newWaves(), WAVES[0].at, 0, DT).w;
  w = run(w, WAVES[0].at, 0, DT).w;
  expect(w.active).toBe(0);
});

test("镜头：跟着玩家往右，不往回走；锁住时对准锁定区中间；不出世界", () => {
  expect(cameraTarget(500, 300, null)).toBe(500 + 60);
  expect(cameraTarget(100, 800, null)).toBe(800);
  expect(cameraTarget(-500, 0, null)).toBe(WORLD.left + VIEW_HALF);
  expect(cameraTarget(99999, 0, null)).toBe(WORLD.right - VIEW_HALF);
  const lock = lockOf(1000);
  expect(cameraTarget(1500, 500, lock)).toBe((lock.left + lock.right) / 2);
  expect(lock.right - lock.left).toBe(2 * VIEW_HALF);
  let cam = 0;
  for (let i = 0; i < 120; i++) cam = followCamera(cam, 1000, DT);
  expect(Math.abs(cam - 1000)).toBeLessThan(5);
  expect(followCamera(0, 1000, DT)).toBeLessThan(1000);
});

test("玩家走不出镜头，锁住时走不出锁定区", () => {
  const free = playerRange(1000, null);
  expect(free.minX).toBeGreaterThan(1000 - VIEW_HALF);
  expect(free.maxX).toBeLessThanOrEqual(WORLD.right);
  const lock = lockOf(1000);
  const locked = playerRange(lock.left + VIEW_HALF, lock);
  expect(locked.minX).toBeGreaterThanOrEqual(lock.left);
  expect(locked.maxX).toBeLessThanOrEqual(lock.right);
});

test("粮车、拒马挡路，城墙和旌旗不挡；物件不挡住触发点", () => {
  const cart = PROPS.find((p) => p.kind === "cart")!;
  expect(blocked(cart.x, cart.d)).toBe(true);
  const wall = PROPS.find((p) => p.kind === "wall")!;
  expect(blocked(wall.x + 50, Math.min(wall.d, 130))).toBe(false);
  expect(FOOTPRINTS.length).toBe(PROPS.filter((p) => ["cart", "fire", "barricade"].includes(p.kind)).length);
  // 每个触发点那一列都有能走的纵深
  for (const w of WAVES) expect([0, 30, 65, 100, 130].some((d) => !blocked(w.at, d))).toBe(true);
});
