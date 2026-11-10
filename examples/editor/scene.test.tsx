// 编辑时的场景（scene.ts）：所见即所得，但不动、没有刚体；选中框、网格、框选框。
import { expect, test } from "nsh/test";
import { parseLevel, type LevelFile } from "../../ts/level";
import { boundsOf, TYPE_ORDER } from "../platformer/catalog";
import { MEADOW, toFile } from "../platformer/levels";
import stairs from "./levels/stairs.json";
import { editorScene, gridLines, MAX_GRID_LINES, type SceneInput } from "./scene";

const input = (level: LevelFile, over: Partial<SceneInput> = {}): SceneInput => ({ level, selection: [], view: { x: 400, y: 200, zoom: 1 }, size: [800, 600], grid: 32, box: null, ...over });

const everyType = (): LevelFile => {
  const level = toFile(MEADOW);
  return { ...level, entities: [...level.entities, ...TYPE_ORDER.filter((t) => t === "light" || t === "model").map((type, i) => ({ id: `x${i}`, type, x: 100, y: 100 }))] };
};

test("编辑时什么都不动：时间缩放 0，没有刚体、关节、角色，瓦片不建碰撞体", () => {
  const scene = editorScene(input(everyType()));
  expect(scene.timeScale).toBe(0);
  expect(scene.joints ?? []).toEqual([]);
  expect((scene.sprites ?? []).filter((s) => s.body !== undefined || s.chase !== undefined)).toEqual([]);
  expect((scene.models ?? []).filter((m) => m.body !== undefined || m.attach !== undefined)).toEqual([]);
  expect((scene.sprites ?? []).some((s) => s.id === "player")).toBe(false);
  expect(scene.tilemaps!.every((t) => (t.solid ?? []).length === 0)).toBe(true);
});

test("每种实体都画得出来，节点 id 不重复", () => {
  const level = everyType();
  const scene = editorScene(input(level, { selection: level.entities.map((e) => e.id) }));
  const ids = [...scene.sprites!, ...scene.models!].map((n) => n.id);
  expect(new Set(ids).size).toBe(ids.length);
  for (const e of level.entities) expect(ids.some((id) => id === `ed:${e.id}` || id.startsWith(`ed:${e.id}:`))).toBe(true);
  for (const type of TYPE_ORDER) expect(level.entities.some((e) => e.type === type)).toBe(true);
  // 灯也进了 3D 灯
  expect(scene.lights!.some((l) => l.id === "ed:x0")).toBe(true);
});

test("选中的画蓝色描边，贴着包围盒，线宽跟着缩放", () => {
  const level = everyType();
  const coin = level.entities.find((e) => e.type === "coin")!;
  const scene = editorScene(input(level, { selection: [coin.id], view: { x: 0, y: 0, zoom: 2 } }));
  const lines = scene.sprites!.filter((s) => s.id.startsWith(`sel:${coin.id}:`));
  expect(lines.length).toBe(4);
  const b = boundsOf(coin);
  const top = lines.find((s) => s.id.endsWith(":t"))!;
  expect([top.y, top.h, top.w]).toEqual([b.top, 1, b.right - b.left + 1]);
  expect(scene.sprites!.filter((s) => s.id.startsWith("sel:")).length).toBe(4);
});

test("网格：只画视野里的，关掉或太密时不画", () => {
  const lines = gridLines({ x: 0, y: 0, zoom: 1 }, [320, 64], 32);
  // x 从 -160 到 160：11 条竖线；y 从 -32 到 32：3 条横线
  expect(lines.filter((l) => l.id.startsWith("grid:v")).length).toBe(11);
  expect(lines.filter((l) => l.id.startsWith("grid:h")).length).toBe(3);
  expect(gridLines({ x: 0, y: 0, zoom: 1 }, [320, 64], 0)).toEqual([]);
  expect(gridLines({ x: 0, y: 0, zoom: 0.25 }, [4000, 300], 8).length).toBe(0);
  expect(MAX_GRID_LINES).toBeGreaterThan(50);
});

test("框选时画框", () => {
  const scene = editorScene(input(everyType(), { box: [[10, 20], [-30, 60]] }));
  const box = scene.sprites!.find((s) => s.id === "box")!;
  expect([box.x, box.y, box.w, box.h]).toEqual([-10, 40, 40, 40]);
});

test("夜里的关卡：开环境光，跟着角色的灯跟着出生点", () => {
  const level = { ...everyType(), night: true };
  const scene = editorScene(input(level));
  const spawn = level.entities.find((e) => e.type === "spawn")!;
  expect(scene.lighting!.lights!.some((l) => l.follow === `ed:${spawn.id}`)).toBe(true);
  expect(scene.lighting!.lights!.some((l) => l.follow === "player")).toBe(false);
  expect(editorScene(input(everyType())).lighting).toBeUndefined();
});

test("第二关石阶的文件在编辑器里画得出来：每个实体都有节点", () => {
  const r = parseLevel(stairs);
  if (!r.ok) throw new Error(r.errors.join());
  const scene = editorScene(input(r.level));
  expect(scene.tilemaps![0].data.length).toBe(72 * 15);
  for (const e of r.level.entities) expect([...scene.sprites!, ...scene.models!].some((n) => n.id === `ed:${e.id}` || n.id.startsWith(`ed:${e.id}:`))).toBe(true);
});
