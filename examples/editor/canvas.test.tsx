// 画布上的纯逻辑：坐标换算、视图、点选和框选、吸附、瓦片刷、复制粘贴（docs/editor.md）。
import { expect, test } from "nsh/test";
import { emptyLevel, type Entity, type LevelFile } from "../../ts/level";
import { screenToWorld, worldToScreen } from "../../ts/nge";
import { copy, nextId, pasted } from "./clipboard";
import { boxSelect, hitTest, prune, toggle } from "./select";
import { snapPoint, snapValue } from "./snap";
import { cellAt, cellsBetween, layerWith, strokeAdd, NEW_STROKE } from "./tiles";
import { MAX_ZOOM, MIN_ZOOM, pan, pinchView, zoomAt } from "./view";

const SIZE: [number, number] = [400, 300];

test("屏幕坐标和世界坐标：中心是相机的位置，y 朝上，按缩放换算，来回一样", () => {
  const cam = { x: 100, y: 50, zoom: 2 };
  expect(screenToWorld(cam, SIZE, [200, 150])).toEqual([100, 50]);
  expect(screenToWorld(cam, SIZE, [210, 140])).toEqual([105, 55]);
  expect(worldToScreen(cam, SIZE, [105, 55])).toEqual([210, 140]);
  // 缩放是 0 或负数时按下限算，不出 NaN、无穷
  const [x, y] = screenToWorld({ x: 0, y: 0, zoom: 0 }, SIZE, [0, 0]);
  expect(Number.isFinite(x) && Number.isFinite(y)).toBe(true);
});

test("视图：拖动平移跟手，以指针为中心缩放时指针下的点不动，缩放有上下限", () => {
  const view = { x: 0, y: 0, zoom: 2 };
  expect(pan(view, 20, -10)).toEqual({ x: -10, y: -5, zoom: 2 });
  const at: [number, number] = [300, 100];
  const before = screenToWorld(view, SIZE, at);
  const zoomed = zoomAt(view, SIZE, at, 1.5);
  expect(zoomed.zoom).toBe(3);
  const after = screenToWorld(zoomed, SIZE, at);
  expect(after.map((v) => Math.round(v * 1000) / 1000)).toEqual(before.map((v) => Math.round(v * 1000) / 1000));
  expect(zoomAt(view, SIZE, at, 100).zoom).toBe(MAX_ZOOM);
  expect(zoomAt(view, SIZE, at, 0.0001).zoom).toBe(MIN_ZOOM);
});

test("双指捏合：以两指中点缩放，中点挪了多少画面跟着挪多少", () => {
  const view = { x: 0, y: 0, zoom: 1 };
  const at: [number, number] = [100, 100];
  const before = screenToWorld(view, SIZE, at);
  // 只缩放：中点下的世界坐标不动
  const zoomed = pinchView(view, SIZE, { x: 100, y: 100, factor: 2, dx: 0, dy: 0 });
  expect(zoomed.zoom).toBe(2);
  expect(screenToWorld(zoomed, SIZE, at).map((v) => Math.round(v * 1000) / 1000)).toEqual(before.map((v) => Math.round(v * 1000) / 1000));
  // 缩放加平移：原来中点下的世界坐标跟着手指到了新的中点
  const moved = pinchView(view, SIZE, { x: 130, y: 90, factor: 2, dx: 30, dy: -10 });
  expect(screenToWorld(moved, SIZE, [130, 90]).map((v) => Math.round(v * 1000) / 1000)).toEqual(screenToWorld(view, SIZE, [100, 100]).map((v) => Math.round(v * 1000) / 1000));
  // 只平移
  expect(pinchView(view, SIZE, { x: 0, y: 0, factor: 1, dx: 20, dy: -10 })).toEqual(pan(view, 20, -10));
});

const box = (e: Entity) => ({ left: e.x - (e.w ?? 10) / 2, right: e.x + (e.w ?? 10) / 2, bottom: e.y - (e.h ?? 10) / 2, top: e.y + (e.h ?? 10) / 2 });
const ENTITIES: Entity[] = [
  { id: "a", type: "coin", x: 0, y: 0, w: 20, h: 20 },
  { id: "b", type: "coin", x: 5, y: 0, w: 20, h: 20 },
  { id: "c", type: "coin", x: 100, y: 100 },
];

test("点选：命中多个时取最上面的（列表里靠后的），点空白是 null，边上算命中", () => {
  expect(hitTest(ENTITIES, box, [3, 0])).toBe("b");
  expect(hitTest(ENTITIES, box, [-8, 0])).toBe("a");
  expect(hitTest(ENTITIES, box, [50, 50])).toBeNull();
  expect(hitTest(ENTITIES, box, [105, 105])).toBe("c");
  // 太小的东西按最小可点范围算
  expect(hitTest([{ id: "t", type: "coin", x: 0, y: 0, w: 1, h: 1 }], box, [3, 3], 4)).toBe("t");
});

test("框选：包围盒和框相交的都选上，框反着拖也一样", () => {
  expect(boxSelect(ENTITIES, box, [-100, -100], [8, 8])).toEqual(["a", "b"]);
  expect(boxSelect(ENTITIES, box, [200, 200], [96, 96])).toEqual(["c"]);
  expect(boxSelect(ENTITIES, box, [40, 40], [60, 60])).toEqual([]);
});

test("选择：切换、删掉的去掉", () => {
  expect(toggle(["a"], "b")).toEqual(["a", "b"]);
  expect(toggle(["a", "b"], "a")).toEqual(["b"]);
  const level = { ...emptyLevel({ name: "", cols: 1, rows: 1, tile: 32, texture: "t", grid: [1, 1], solid: [] }), entities: ENTITIES.slice(0, 1) };
  expect(prune(["a", "b", "zz"], level)).toEqual(["a"]);
});

test("吸附：比格子大的边对齐格线，比格子小的放在格子中间，网格 0 不吸附", () => {
  expect(snapValue(37, 32, 64)).toBe(32);
  expect(snapValue(50, 32, 96)).toBe(48);
  expect(snapValue(37, 32, 14)).toBe(48);
  expect(snapValue(-5, 32, 14)).toBe(-16);
  expect(snapValue(37.3, 0, 14)).toBe(37.3);
  expect(snapPoint([37, 70], 32, [64, 16])).toEqual([32, 80]);
});

const layer = () => emptyLevel({ name: "", cols: 4, rows: 3, tile: 32, texture: "t", grid: [3, 1], solid: [] }).tiles[0];

test("瓦片：世界坐标换成格子（左上角在 (0, 64)，y 朝上），地图外是 null", () => {
  const t = layer();
  expect(cellAt(t, [5, 60])).toEqual({ r: 0, c: 0 });
  expect(cellAt(t, [127, -31])).toEqual({ r: 2, c: 3 });
  expect(cellAt(t, [128, 0])).toBeNull();
  expect(cellAt(t, [5, 65])).toBeNull();
});

test("拖得快时两个格子之间按直线补齐，不漏格", () => {
  expect(cellsBetween({ r: 0, c: 0 }, { r: 0, c: 3 })).toEqual([{ r: 0, c: 0 }, { r: 0, c: 1 }, { r: 0, c: 2 }, { r: 0, c: 3 }]);
  expect(cellsBetween({ r: 2, c: 0 }, { r: 0, c: 2 })).toEqual([{ r: 2, c: 0 }, { r: 1, c: 1 }, { r: 0, c: 2 }]);
  expect(cellsBetween({ r: 1, c: 1 }, { r: 1, c: 1 })).toEqual([{ r: 1, c: 1 }]);
});

test("一笔：重复的格子只记一次（旧值是第一次的），和原来一样的不记，预览就是画上去的样子", () => {
  const t = { ...layer(), data: [[-1, -1, -1, -1], [-1, -1, -1, -1], [1, 1, -1, -1]] };
  let s = strokeAdd(NEW_STROKE, t, [{ r: 2, c: 0 }, { r: 2, c: 1 }, { r: 2, c: 2 }], 1);
  s = strokeAdd(s, t, [{ r: 2, c: 2 }, { r: 1, c: 2 }], 1);
  expect(s.changes).toEqual([{ r: 2, c: 2, before: -1, after: 1 }, { r: 1, c: 2, before: -1, after: 1 }]);
  // 橡皮（-1）
  const erase = strokeAdd(NEW_STROKE, t, [{ r: 2, c: 0 }, { r: 0, c: 0 }], -1);
  expect(erase.changes).toEqual([{ r: 2, c: 0, before: 1, after: -1 }]);
  expect(layerWith(t, s).data[1]).toEqual([-1, -1, 1, -1]);
  expect(t.data[1]).toEqual([-1, -1, -1, -1]);
});

const LEVEL = (): LevelFile => ({ ...emptyLevel({ name: "", cols: 1, rows: 1, tile: 32, texture: "t", grid: [1, 1], solid: [] }), entities: [
  { id: "e1", type: "coin", x: 0, y: 0 },
  { id: "e7", type: "enemy", x: 10, y: 0, name: "大怪", props: { patrol: 2 } },
  { id: "spawn", type: "spawn", x: 0, y: 0 },
] });

test("新 id：e 加数字，比已有的都大", () => {
  expect(nextId(LEVEL())).toBe("e8");
  expect(nextId({ ...LEVEL(), entities: [] })).toBe("e1");
});

test("复制按场景树的顺序深拷贝；粘贴换新 id、名字加副本、偏移一格，原来的不变", () => {
  const level = LEVEL();
  const clip = copy(level, ["e7", "e1"]);
  expect(clip.map((e) => e.id)).toEqual(["e1", "e7"]);
  const out = pasted(level, clip, [32, 0]);
  expect(out).toEqual([
    { id: "e8", type: "coin", x: 32, y: 0 },
    { id: "e9", type: "enemy", x: 42, y: 0, name: "大怪 副本", props: { patrol: 2 } },
  ]);
  // 深拷贝：改粘贴出来的不影响原来的
  (out[1].props as Record<string, number>).patrol = 9;
  expect(level.entities[1].props).toEqual({ patrol: 2 });
  expect(clip[1].props).toEqual({ patrol: 2 });
});

test("新 id 一定是没用过的：编号到了 2^53 附近也不重", () => {
  const base = emptyLevel({ name: "", cols: 1, rows: 1, tile: 32, texture: "t", grid: [1, 1], solid: [] });
  const big: LevelFile = { ...base, entities: [{ id: "e9007199254740992", type: "coin", x: 0, y: 0 }, { id: "e9007199254740991", type: "coin", x: 0, y: 0 }, { id: "e1", type: "coin", x: 0, y: 0 }] };
  const id = nextId(big);
  expect(big.entities.some((e) => e.id === id)).toBe(false);
  const ids = pasted(big, big.entities, [0, 0]).map((e) => e.id);
  expect(new Set(ids).size).toBe(3);
  expect(ids.some((i) => big.entities.some((e) => e.id === i))).toBe(false);
});

test("预览瓦片：一笔里的格子超出现在的瓦片层时跳过", () => {
  const t = emptyLevel({ name: "", cols: 2, rows: 2, tile: 32, texture: "t", grid: [3, 1], solid: [] }).tiles[0];
  const out = layerWith(t, { changes: [{ r: 5, c: 0, before: -1, after: 1 }, { r: 0, c: 1, before: -1, after: 2 }] });
  expect(out.data).toEqual([[-1, 2], [-1, -1]]);
});

test("点选按画的先后：深度大的先命中，一样深时列表里靠后的先命中", () => {
  const es: Entity[] = [
    { id: "low", type: "coin", x: 0, y: 0 },
    { id: "high", type: "coin", x: 0, y: 0 },
    { id: "mid", type: "coin", x: 0, y: 0 },
  ];
  const b = (e: Entity) => ({ left: e.x - 5, right: e.x + 5, bottom: e.y - 5, top: e.y + 5 });
  const depth = (e: Entity) => (e.id === "high" ? 3 : e.id === "mid" ? 2 : 2);
  expect(hitTest(es, b, [0, 0], 0, depth)).toBe("high");
  expect(hitTest(es.filter((e) => e.id !== "high"), b, [0, 0], 0, depth)).toBe("mid");
});
