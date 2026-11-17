// 关卡文件（docs/editor.md）：代码描述的关卡导出成文件再转回来一样，第二关从文件读，坏文件不崩。
import { expect, test } from "nsh/test";
import { checkTypes, formatLevel, parseLevel, type Entity, type LevelFile } from "../../ts/level";
import { TILESET, TYPES } from "./catalog";
import { build, fileLevel, fromFile, LEVELS, MEADOW, TILE, toFile, type Built } from "./levels";

const near = (a: number, b: number) => Math.abs(a - b) < 0.01;
const round = (v: number) => Math.round(v * 100) / 100;

/** 按位置比两个关卡（id 不比：文件里的关卡按类型重新编号）。 */
function sameBuilt(a: Built, b: Built) {
  const xy = (list: { x: number; y: number }[]) => list.map((c) => [round(c.x), round(c.y)]).sort();
  expect(a.tilemaps).toEqual(b.tilemaps);
  expect(xy(a.coins)).toEqual(xy(b.coins));
  expect(a.enemies.map((e) => [e.x, e.y, e.to.x])).toEqual(b.enemies.map((e) => [e.x, e.y, e.to.x]));
  expect(xy(a.ghosts)).toEqual(xy(b.ghosts));
  expect(a.lifts.map((l) => [l.node.x, l.node.y, l.node.w, l.to, l.duration])).toEqual(b.lifts.map((l) => [l.node.x, l.node.y, l.node.w, l.to, l.duration]));
  expect(a.ramps.length).toBe(b.ramps.length);
  a.ramps.forEach((r, i) => {
    const o = b.ramps[i];
    for (const k of ["x", "y", "w", "h", "rotation"] as const) expect(near(r[k] as number, o[k] as number)).toBe(true);
  });
  expect([a.tower, a.jelly, a.right, a.top, a.spawn]).toEqual([b.tower, b.jelly, b.right, b.top, b.spawn]);
  expect([a.crate.body.x, a.crate.body.y, a.goal.x, a.goal.y]).toEqual([b.crate.body.x, b.crate.body.y, b.goal.x, b.goal.y]);
  expect(xy(a.swing.sprites as { x: number; y: number }[])).toEqual(xy(b.swing.sprites as { x: number; y: number }[]));
  expect(a.swing.joints.map((j) => j.anchorA)).toEqual(b.swing.joints.map((j) => j.anchorA));
}

const fromOk = (level: LevelFile) => {
  const r = fromFile(level);
  if (!r.ok) throw new Error(r.errors.join("\n"));
  return r.built;
};

test("草原导出成关卡文件：格式和类型都合规，写成文本再读回来一样", () => {
  const file = toFile(MEADOW);
  expect(checkTypes(file, TYPES)).toEqual([]);
  expect(parseLevel(formatLevel(file))).toEqual({ ok: true, level: file });
  // 各种东西都在
  const types = new Set(file.entities.map((e) => e.type));
  for (const t of ["spawn", "coin", "enemy", "ghost", "lift", "crate", "swing", "goal", "ramp", "tower", "jelly"]) expect(types.has(t)).toBe(true);
});

test("草原导出再转回来，和代码生成的关卡逐项一样", () => {
  sameBuilt(fromOk(toFile(MEADOW)), build(MEADOW));
});

test("第二关石阶从文件读：72 列、16 个金币、3 个敌人、2 个幽灵、2 块移动平台、铁链和箱子", () => {
  const stairs = LEVELS[1];
  expect(stairs.name).toBe("石阶");
  expect(stairs.night).toBeFalsy();
  const b = stairs.built;
  expect(b.right).toBe(72 * TILE);
  expect([b.coins.length, b.enemies.length, b.ghosts.length, b.lifts.length]).toEqual([16, 3, 2, 2]);
  expect(b.swing.sprites.length).toBeGreaterThan(0);
  expect([b.crate.body.x, b.goal.x]).toEqual([66.5 * TILE, 68 * TILE]);
  // 敌人的 id 还是 enemy- 开头（Rust 按这个认）
  for (const e of [...b.enemies, ...b.ghosts]) expect(e.id.startsWith("enemy-")).toBe(true);
  expect(b.tilemaps[0].friction).toBe(0);
  // 相机的上边和以前一样
  expect(LEVELS.map((l) => l.built.top)).toEqual([448, 448, 448]);
});

const minimal = (entities: Entity[]): LevelFile => ({
  format: "nge-level",
  version: 1,
  name: "小",
  night: false,
  tiles: [{ id: "level", texture: TILESET.texture, grid: TILESET.grid, tile: 32, cols: 10, rows: 2, x: 0, y: 32, solid: TILESET.solid, data: [Array(10).fill(-1), Array(10).fill(1)] }],
  entities: [{ id: "c", type: "crate", x: 100, y: 20 }, { id: "g", type: "goal", x: 280, y: 40 }, ...entities],
});

test("实体按类型转：出生点、移动平台（宽、往上走）、斜坡、灯、模型；没写的瓦片摩擦按 0", () => {
  const b = fromOk(
    minimal([
      { id: "s", type: "spawn", x: 50, y: 30 },
      { id: "l", type: "lift", x: 64, y: 0, w: 96, props: { dx: 0, dy: 100, duration: 3 } },
      { id: "r", type: "ramp", x: 100, y: 16, w: 64, h: 32, props: { down: true } },
      { id: "p", type: "light", x: 10, y: 20, props: { kind: "spot", z: 50 } },
      { id: "m", type: "model", x: 30, y: 40, props: { model: "flag.glb", scale: 20 } },
      { id: "e", type: "enemy", x: 200, y: 12, props: { patrol: 2 } },
    ]),
  );
  expect(b.spawn).toEqual([50, 30]);
  expect(b.lifts[0]).toEqual({ node: { id: "lift-0", x: 64, y: 0, w: 96, h: 16 }, to: { y: 100 }, duration: 3 });
  // 往右下降：左边高
  expect(b.ramps[0].rotation! < 0).toBe(true);
  expect(b.lights).toEqual([{ id: "light-0", kind: "spot", x: 10, y: 20, z: 50, color: "#ffffff", intensity: 20000, range: 400, angle: 30, direction: [0, -0.5, -1] }]);
  expect(b.models).toEqual([{ id: "model-0", model: "asset:/@platformer/flag.glb", x: 30, y: 40, z: 0, scale: 20, spin: 0 }]);
  expect(b.enemies[0]).toEqual({ id: "enemy-0", x: 200, y: 12, to: { x: 200 + 2 * TILE } });
  expect(b.tilemaps[0].friction).toBe(0);
  expect(b.right).toBe(320);
});

test("没有出生点时在老地方；铁链的支点是包围盒的顶边", () => {
  const b = fromOk(minimal([{ id: "w", type: "swing", x: 100, y: 104 }]));
  expect(b.spawn).toEqual([3 * TILE, 16]);
  expect(b.swing.joints[0].anchorA).toEqual([100, 150]);
});

test("不合规的关卡不转：缺终点、类型不认识、移动平台两个方向都给", () => {
  const noGoal = minimal([]);
  noGoal.entities = noGoal.entities.filter((e) => e.type !== "goal");
  const r1 = fromFile(noGoal);
  expect(r1.ok).toBe(false);
  if (!r1.ok) expect(r1.errors.join()).toContain("终点");
  const r2 = fromFile(minimal([{ id: "x", type: "dragon", x: 0, y: 0 }]));
  expect(r2.ok).toBe(false);
  const r3 = fromFile(minimal([{ id: "l", type: "lift", x: 0, y: 0, props: { dx: 10, dy: 10 } }]));
  expect(r3.ok).toBe(false);
  if (!r3.ok) expect(r3.errors.join()).toContain("只能往一个方向");
});

test("游戏读坏了的关卡文件不崩：换成一段平地加终点，报错", () => {
  const errors: string[] = [];
  const entry = fileLevel("{坏的", "备用", (m) => errors.push(m));
  expect(entry.name).toBe("备用");
  expect(entry.built.goal.id).toBe("goal");
  expect(entry.built.tilemaps[0].data.some((f) => f >= 0)).toBe(true);
  expect(errors.length).toBe(1);
  const ok = fileLevel(formatLevel(minimal([])), "备用", (m) => errors.push(m));
  expect(ok.name).toBe("小");
  expect(errors.length).toBe(1);
});
