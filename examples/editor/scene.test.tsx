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
  return { ...level, entities: [...level.entities, ...TYPE_ORDER.filter((t) => ["light", "model", "walker", "group", "deco", "prefab"].includes(t)).map((type) => ({ id: `x-${type}`, type, x: 100, y: 100 }))] };
};

test("编辑时什么都不动：时间缩放 0，没有刚体、关节、角色，瓦片不建碰撞体", () => {
  const scene = editorScene(input(everyType()));
  expect(scene.timeScale).toBe(0);
  expect(scene.joints ?? []).toEqual([]);
  expect((scene.sprites ?? []).filter((s) => s.body !== undefined || s.chase !== undefined || s.behavior !== undefined)).toEqual([]);
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
  expect(scene.lights!.some((l) => l.id === "ed:x-light")).toBe(true);
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

test("同一种东西，场景树里靠后的画在上面；预览只用合规的属性", () => {
  const base = toFile(MEADOW);
  const level: LevelFile = { ...base, entities: [{ id: "a", type: "coin", x: 0, y: 0 }, { id: "b", type: "coin", x: 0, y: 0 }, { id: "m", type: "model", x: 0, y: 0, props: { model: "../secret.glb", scale: -3 } }] };
  const scene = editorScene(input(level));
  const z = (id: string) => scene.sprites!.find((s) => s.id === id)!.z!;
  expect(z("ed:b")).toBeGreaterThan(z("ed:a"));
  // 不跨到别的类型的层：金币 2，敌人 4
  expect(z("ed:b")).toBeLessThan(3);
  const model = scene.models!.find((m) => m.id === "ed:m")!;
  expect(model.model).toBe("asset:/@platformer/crate.glb");
  expect(model.scale).toBe(40);
});

test("移动平台只能改宽：文件里写了高也按 16 画、按 16 算包围盒", () => {
  const base = toFile(MEADOW);
  const lift = { id: "l", type: "lift", x: 0, y: 0, w: 96, h: 64 };
  const level: LevelFile = { ...base, entities: [lift] };
  const s = editorScene(input(level)).sprites!.find((n) => n.id === "ed:l")!;
  expect([s.w, s.h]).toEqual([96, 16]);
  const b = boundsOf(lift);
  expect(b.top - b.bottom).toBe(16);
});

test("瓦片层没变时瓦片地图用同一份，不重新摊平", () => {
  const level = toFile(MEADOW);
  const a = editorScene(input(level)).tilemaps![0];
  const moved = { ...level, entities: level.entities.map((e, i) => (i === 0 ? { ...e, x: e.x + 1 } : e)) };
  expect(editorScene(input(moved)).tilemaps![0] === a).toBe(true);
});

// ---- docs/editor-more.md ----

test("藏起来的层不画；没写 z 的层按顺序叠；预制体实体变成实例（和游戏里一样按包围盒中心放）", () => {
  const level = toFile(MEADOW);
  const far = { ...level.tiles[0], id: "far", hidden: true };
  const top = { ...level.tiles[0], id: "top" };
  const withLayers: LevelFile = { ...level, tiles: [level.tiles[0], far, top], entities: [...level.entities, { id: "p", type: "prefab", x: 300, y: 68, props: { prefab: "grove" }, set: { a: { x: 1 } } }] };
  const scene = editorScene(input(withLayers));
  expect(scene.tilemaps!.map((t) => [t.id, t.z])).toEqual([["level", undefined], ["top", 0.002]]);
  expect(scene.instances).toEqual([{ id: "ed:p:inst", prefab: "grove", x: 236, y: 0, set: { a: { x: 1 } } }]);
  expect(Object.keys(scene.prefabs ?? {})).toContain("grove");
});

test("悬停：没选中的画一圈浅色细框，选中的不画；选中的走地怪画出路线点", () => {
  const level = everyType();
  const coin = level.entities.find((e) => e.type === "coin")!;
  const hovered = editorScene(input(level, { hover: coin.id, view: { x: 0, y: 0, zoom: 2 } }));
  const ring = hovered.sprites!.filter((s) => s.id.startsWith(`hov:${coin.id}:`));
  expect(ring.length).toBe(4);
  expect(ring.find((s) => s.id.endsWith(":t"))!.h).toBe(0.5);
  expect(editorScene(input(level, { hover: coin.id, selection: [coin.id] })).sprites!.some((s) => s.id.startsWith("hov:"))).toBe(false);
  const walker = { id: "w", type: "walker", x: 100, y: 12, points: [[200, 12], [260, 76]] as [number, number][] };
  const routed = editorScene(input({ ...level, entities: [...level.entities, walker] }, { selection: ["w"] }));
  expect(routed.sprites!.filter((s) => s.id.startsWith("pt:w:")).map((s) => [s.x, s.y])).toEqual([[200, 12], [260, 76]]);
  // 路线：出生点 → 每个点连起来
  expect(routed.sprites!.filter((s) => s.id.startsWith("ed:w:path")).length).toBe(2);
});

test("装饰按旋转和 z 画；转过的包围盒是外接矩形", () => {
  const deco = { id: "d", type: "deco", x: 0, y: 0, w: 20, h: 10, rotation: 90, z: -5 };
  const scene = editorScene(input({ ...toFile(MEADOW), entities: [deco] }));
  const s = scene.sprites!.find((x) => x.id === "ed:d")!;
  expect([s.rotation, s.z]).toEqual([90, -5]);
  const b = boundsOf(deco);
  expect([Math.round(b.right - b.left), Math.round(b.top - b.bottom)]).toEqual([10, 20]);
  // 不能转的类型写了 rotation 也不算
  const coin = boundsOf({ id: "c", type: "coin", x: 0, y: 0, rotation: 45 });
  expect(coin.right - coin.left).toBe(14);
});
