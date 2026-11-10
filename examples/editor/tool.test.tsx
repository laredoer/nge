// 画布上的指针（tool.ts）：按下 / 移动 / 松开 → 选择、移动、框选、平移、放置、瓦片刷；文件名和存取（files.ts）。
import { expect, test } from "nsh/test";
import { emptyLevel, formatLevel, type Entity, type LevelFile } from "../../ts/level";
import { fileName, memoryStore, openFrom, saveTo } from "./files";
import { execute, newEditor } from "./history";
import { IDLE, pointerDown, pointerMove, pointerUp, previewOf, type Ctx, type Drag } from "./tool";

const box = (e: Entity) => ({ left: e.x - 10, right: e.x + 10, bottom: e.y - 10, top: e.y + 10 });
const level = (): LevelFile => ({
  ...emptyLevel({ name: "", cols: 8, rows: 4, tile: 32, texture: "t", grid: [3, 1], solid: [] }),
  entities: [
    { id: "e1", type: "coin", x: 0, y: 0 },
    { id: "e2", type: "coin", x: 100, y: 0 },
    { id: "e3", type: "tower", x: 200, y: 22 },
  ],
});
// 视图：中心在世界原点，缩放 1，视口 400 × 300：屏幕 (200 + x, 150 - y) 是世界 (x, y)
const at = (x: number, y: number): [number, number] => [200 + x, 150 - y];
const ctx = (over: Partial<Ctx> = {}): Ctx => ({
  level: level(),
  selection: [],
  view: { x: 0, y: 0, zoom: 1 },
  size: [400, 300],
  grid: 0,
  tool: "select",
  placeType: null,
  frame: 1,
  layer: 0,
  boundsOf: box,
  sizeOf: () => [20, 20],
  // 箱子堆站在地上：y 固定
  place: (e, x, y) => ({ ...e, x, y: e.type === "tower" ? 22 : y }),
  make: (type, id, x, y) => ({ id, type, x, y }),
  ...over,
});

/** 按下、依次移动、在最后一个点松开。 */
function gesture(c: Ctx, points: [number, number][]) {
  let out = pointerDown(c, points[0]);
  let cur: Ctx = { ...c, selection: out.selection ?? c.selection, view: out.view ?? c.view };
  let drag: Drag = out.drag;
  const previews: LevelFile[] = [];
  for (const p of points.slice(1)) {
    out = pointerMove(cur, drag, p);
    drag = out.drag;
    cur = { ...cur, selection: out.selection ?? cur.selection, view: out.view ?? cur.view };
    previews.push(previewOf(cur, drag));
  }
  const end = pointerUp(cur, drag, points.at(-1)!);
  return { end, cur, previews };
}

test("点一下选中，点空白取消选择，点多选里的一个只留它", () => {
  expect(gesture(ctx(), [at(2, 3)]).end.selection).toEqual(["e1"]);
  expect(gesture(ctx({ selection: ["e1"] }), [at(50, 50)]).end.selection).toEqual([]);
  expect(gesture(ctx({ selection: ["e1", "e2"] }), [at(100, 0)]).end.selection).toEqual(["e2"]);
});

test("拖动选中的：一起移动，松开时一个移动命令，撤销回到原处；挪不到 3 point 算点击", () => {
  const c = ctx({ selection: ["e1", "e2"] });
  const { end, previews } = gesture(c, [at(0, 0), at(10, 0), at(30, -20)]);
  expect(previews.at(-1)!.entities.map((e) => [e.x, e.y])).toEqual([[30, -20], [130, -20], [200, 22]]);
  const ed = execute(newEditor(c.level), end.command!);
  expect(ed.level.entities.map((e) => [e.x, e.y])).toEqual([[30, -20], [130, -20], [200, 22]]);
  expect(end.command!.revert(ed.level)).toEqual(c.level);
  const tiny = gesture(c, [at(0, 0), at(2, 1)]);
  expect(tiny.end.command).toBeUndefined();
  expect(tiny.end.selection).toEqual(["e1"]);
});

test("拖没选中的：先选中它再拖；站在地上的只改 x", () => {
  const { end } = gesture(ctx({ selection: ["e1"] }), [at(200, 22), at(240, 80)]);
  const moved = end.command!.apply(level());
  expect(moved.entities.find((e) => e.id === "e3")).toEqual({ id: "e3", type: "tower", x: 240, y: 22 });
  expect(moved.entities.find((e) => e.id === "e1")).toEqual({ id: "e1", type: "coin", x: 0, y: 0 });
});

test("移动时按网格吸附：按抓着的那个对齐，别的跟着挪一样多", () => {
  const c = ctx({ selection: ["e1", "e2"], grid: 32 });
  const { end } = gesture(c, [at(0, 0), at(37, 5)]);
  // 抓着 e1（20 宽，比格子小）：中心落在格子中间 (48, 16)，e2 也挪 (48, 16)
  expect(end.command!.apply(c.level).entities.slice(0, 2).map((e) => [e.x, e.y])).toEqual([[48, 16], [148, 16]]);
});

test("在空白处拖是框选，框住的都选上", () => {
  const { end, cur } = gesture(ctx(), [at(-20, 30), at(150, -30)]);
  expect(end.selection).toEqual(["e1", "e2"]);
  expect(end.command).toBeUndefined();
  expect(cur.selection).toEqual([]);
});

test("平移工具：拖动挪视图，不选东西", () => {
  const { end } = gesture(ctx({ tool: "pan", view: { x: 0, y: 0, zoom: 2 } }), [[100, 100], [140, 80]]);
  expect(end.view).toEqual({ x: -20, y: -10, zoom: 2 });
  expect(end.selection).toBeUndefined();
});

test("放置工具：点一下在吸附后的位置放一个物件库选中的类型，选中它", () => {
  const { end } = gesture(ctx({ tool: "place", placeType: "enemy", grid: 32 }), [at(37, 5)]);
  const added = end.command!.apply(level());
  expect(added.entities.at(-1)).toEqual({ id: "e4", type: "enemy", x: 48, y: 16 });
  expect(end.selection).toEqual(["e4"]);
  // 没选类型时什么也不放
  expect(gesture(ctx({ tool: "place" }), [at(37, 5)]).end.command).toBeUndefined();
});

test("瓦片刷：拖过的格子都画上（快速拖动也不漏），一笔一个命令；橡皮擦掉", () => {
  // 瓦片层左上角在 (0, 96)：第 2 行（从 0 数）是 y ∈ [0, 32)
  const c = ctx({ tool: "brush", frame: 2 });
  const { end, previews } = gesture(c, [at(5, 10), at(100, 10)]);
  expect(previews.at(-1)!.tiles[0].data[2]).toEqual([2, 2, 2, 2, -1, -1, -1, -1]);
  const painted = end.command!.apply(c.level);
  expect(painted.tiles[0].data[2]).toEqual([2, 2, 2, 2, -1, -1, -1, -1]);
  const erase = gesture({ ...c, tool: "erase", level: painted }, [at(40, 10)]);
  expect(erase.end.command!.apply(painted).tiles[0].data[2]).toEqual([2, -1, 2, 2, -1, -1, -1, -1]);
  // 在地图外画什么也不发生
  expect(gesture(c, [at(-50, 10)]).end.command).toBeUndefined();
});

test("松开后回到空闲", () => {
  expect(gesture(ctx(), [at(0, 0)]).end.drag).toEqual(IDLE);
});

test("文件名：去掉空白、补 .json，不让带路径和怪字符", () => {
  expect(fileName(" 第一关 ")).toBe("第一关.json");
  expect(fileName("my-level_2.json")).toBe("my-level_2.json");
  expect(fileName("")).toBeNull();
  expect(fileName("../x")).toBeNull();
  expect(fileName("a/b")).toBeNull();
  expect(fileName(".json")).toBeNull();
  expect(fileName("x".repeat(80))).toBeNull();
});

test("存取：保存是格式化的文本，打开时校验，坏文件报错不崩", async () => {
  const store = memoryStore({ "bad.json": "{坏的", "notes.txt": "x" });
  await saveTo(store, "a.json", level());
  expect(store.files["a.json"]).toBe(formatLevel(level()));
  expect(await store.list()).toEqual(["a.json", "bad.json"]);
  expect(await openFrom(store, "a.json")).toEqual({ ok: true, level: level() });
  const bad = await openFrom(store, "bad.json");
  expect(bad.ok).toBe(false);
  const missing = await openFrom(store, "nope.json");
  expect(missing.ok).toBe(false);
  if (!missing.ok) expect(missing.errors[0]).toContain("nope.json");
});

test("拖动中被拖的实体没了：拖动作废，不抛错", () => {
  const c = ctx();
  const down = pointerDown(c, at(0, 0));
  const moving = pointerMove({ ...c, selection: ["e1"] }, down.drag, at(30, 0)).drag;
  expect(moving.kind).toBe("move");
  const gone = { ...c, level: { ...c.level, entities: c.level.entities.filter((e) => e.id !== "e1") } };
  expect(pointerMove(gone, moving, at(40, 0)).drag.kind).toBe("idle");
  expect(pointerUp(gone, moving, at(40, 0)).command).toBe(undefined);
});

test("按下以后换了工具：松手按按下时的工具算", () => {
  const c = ctx({ tool: "place", placeType: "coin" });
  const down = pointerDown(c, at(50, 50));
  const up = pointerUp({ ...c, tool: "select" }, down.drag, at(50, 50));
  expect(up.command).not.toBe(undefined);
  const sel = ctx();
  const press = pointerDown(sel, at(300, 300));
  expect(pointerUp({ ...sel, tool: "place", placeType: "coin" }, press.drag, at(300, 300)).command).toBe(undefined);
});
