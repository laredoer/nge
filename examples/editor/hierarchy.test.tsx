// 父子层级、预制体覆盖、路线点（docs/editor-more.md）。
import { expect, test } from "nsh/test";
import { emptyLevel, formatLevel, parseLevel, type Entity, type LevelFile } from "../../ts/level";
import { blankLevel } from "./store";
import { copy, pasted } from "./clipboard";
import { canParent, descendantsOf, dropTarget, setParent, treeRows, withDescendants } from "./hierarchy";
import { overrideList, parseOverride, withOverride, withoutOverride } from "./overrides";
import { addPoint, movePoint, pointAt, removePoint } from "./points";

const level = (): LevelFile => ({
  ...emptyLevel({ name: "", cols: 4, rows: 2, tile: 32, texture: "t", grid: [3, 1], solid: [] }),
  entities: [
    { id: "g", type: "group", x: 0, y: 0 },
    { id: "a", type: "coin", x: 1, y: 1, parent: "g" },
    { id: "h", type: "group", x: 0, y: 0, parent: "g" },
    { id: "b", type: "coin", x: 2, y: 2, parent: "h" },
    { id: "c", type: "coin", x: 3, y: 3 },
  ],
});

test("子孙：一层层往下，不含自己；选中的加上子孙，按数组顺序", () => {
  const l = level();
  expect(descendantsOf(l, "g")).toEqual(["a", "h", "b"]);
  expect(descendantsOf(l, "c")).toEqual([]);
  expect(withDescendants(l, ["h", "c"])).toEqual(["h", "b", "c"]);
});

test("能不能当父节点：不能是自己、不能是自己的子孙、要在关卡里；放回最外层总行", () => {
  const l = level();
  expect(canParent(l, "g", "b")).toBe(false);
  expect(canParent(l, "g", "g")).toBe(false);
  expect(canParent(l, "c", "nope")).toBe(false);
  expect(canParent(l, "c", "h")).toBe(true);
  expect(canParent(l, "a", undefined)).toBe(true);
});

test("场景树的行：父节点后面跟着子孙、缩进一层；折起来的不展开", () => {
  const l = level();
  const rows = treeRows(l, new Set());
  expect(rows.map((r) => [r.id, r.depth, r.parent])).toEqual([["g", 0, true], ["a", 1, false], ["h", 1, true], ["b", 2, false], ["c", 0, false]]);
  const folded = treeRows(l, new Set(["h"]));
  expect(folded.map((r) => r.id)).toEqual(["g", "a", "h", "c"]);
  expect(folded[2].collapsed).toBe(true);
  // 没有孩子的折了也不算折
  expect(treeRows(l, new Set(["c"]))[4].collapsed).toBe(false);
});

test("拖到哪一行：没出这一行不算，最后一行下面是回到最外层", () => {
  const rows = treeRows(level(), new Set());
  expect(dropTarget(rows, 4, 4 * 24 + 3, 24)).toBe(undefined);
  expect(dropTarget(rows, 4, 2 * 24 + 1, 24)).toBe("h");
  expect(dropTarget(rows, 0, 9 * 24, 24)).toBe(null);
  expect(dropTarget(rows, 0, -5, 24)).toBe(undefined);
});

test("换父节点：放进分组、移出分组，绕成圈的跳过，都没变是 null；撤销还原；存得下来", () => {
  const l = level();
  const into = setParent(l, ["c"], "h")!;
  const next = into.apply(l);
  expect(next.entities.find((e) => e.id === "c")!.parent).toBe("h");
  expect(into.revert(next)).toEqual(l);
  const out = setParent(l, ["a"], undefined)!;
  expect("parent" in out.apply(l).entities[1]).toBe(false);
  expect(setParent(l, ["g"], "b")).toBe(null);
  expect(setParent(l, ["a"], "g")).toBe(null);
  // 父子一起选中：只挪上面的，孩子跟着
  const both = setParent(l, ["h", "b"], undefined)!.apply(l);
  expect(both.entities.find((e) => e.id === "h")!.parent).toBe(undefined);
  expect(both.entities.find((e) => e.id === "b")!.parent).toBe("h");
  expect(parseLevel(formatLevel(next)).ok).toBe(true);
});

test("预制体覆盖：路径、字段名按规则，值按数、布尔、文字认；加、换、删", () => {
  expect(parseOverride("b/crown", "y", "110")).toEqual({ ok: true, value: { path: "b/crown", field: "y", value: 110 } });
  expect(parseOverride("crown", "unlit", "true")).toEqual({ ok: true, value: { path: "crown", field: "unlit", value: true } });
  expect(parseOverride("crown", "color", "#fff")).toEqual({ ok: true, value: { path: "crown", field: "color", value: "#fff" } });
  expect(parseOverride("b//crown", "y", "1").ok).toBe(false);
  expect(parseOverride("crown", "__proto__", "1").ok).toBe(false);
  expect(parseOverride("crown", "y", "1e9").ok).toBe(false);
  const one = withOverride(undefined, "b/crown", "y", 110);
  const two = withOverride(one, "b/crown", "color", "#fff");
  expect(two).toEqual({ "b/crown": { y: 110, color: "#fff" } });
  expect(withOverride(two, "b/crown", "y", 90)["b/crown"].y).toBe(90);
  expect(overrideList(two)).toEqual([{ path: "b/crown", field: "y", value: 110 }, { path: "b/crown", field: "color", value: "#fff" }]);
  expect(withoutOverride(two, "b/crown", "y")).toEqual({ "b/crown": { color: "#fff" } });
  expect(withoutOverride(withoutOverride(two, "b/crown", "y"), "b/crown", "color")).toBe(undefined);
  expect(withoutOverride(two, "a", "y")).toBe(two);
});

test("路线点：加在最后一个右边一格，删、挪；点中最近的，只找给的实体", () => {
  const w = { id: "w", type: "walker", x: 100, y: 12 };
  const one = addPoint(w, 32);
  expect(one).toEqual([[132, 12]]);
  const two = addPoint({ ...w, points: one }, 32);
  expect(two).toEqual([[132, 12], [164, 12]]);
  expect(removePoint({ ...w, points: two }, 0)).toEqual([[164, 12]]);
  expect(removePoint({ ...w, points: one }, 0)).toBe(undefined);
  expect(movePoint({ ...w, points: two }, 1, 0, 0)).toEqual([[132, 12], [0, 0]]);
  const entities = [{ ...w, points: two }, { id: "v", type: "walker", x: 0, y: 0, points: [[160, 12]] as [number, number][] }];
  expect(pointAt(entities, ["w"], [160, 14], 6)).toEqual({ id: "w", index: 1 });
  expect(pointAt(entities, ["v"], [160, 14], 6)).toEqual({ id: "v", index: 0 });
  expect(pointAt(entities, ["w"], [148, 30], 6)).toBe(null);
  // 没选中的实体的点更近也不算
  expect(pointAt(entities, ["v"], [165, 12], 6)).toEqual({ id: "v", index: 0 });
  expect(pointAt(entities, [], [160, 12], 6)).toBe(null);
  // 满了不加
  expect(addPoint({ ...w, points: Array.from({ length: 64 }, () => [0, 0] as [number, number]) }, 32).length).toBe(64);
});

test("复制分组带着子孙，粘贴换新 id、父节点换成新的；只复制孩子时留在原来的分组里；路线点一起挪", () => {
  const l = level();
  const clip = copy(l, ["h"]);
  expect(clip.map((e) => e.id)).toEqual(["h", "b"]);
  const out = pasted(l, clip, [32, 0]);
  expect(out.map((e) => [e.id, e.parent])).toEqual([["e1", "g"], ["e2", "e1"]]);
  const child = pasted(l, copy(l, ["a"]), [32, 0]);
  expect(child[0].parent).toBe("g");
  const gone = pasted({ ...l, entities: [] }, copy(l, ["a"]), [0, 0]);
  expect(gone[0].parent).toBe(undefined);
  const walker = pasted(l, [{ id: "w", type: "walker", x: 0, y: 0, points: [[10, 0]] }], [32, 8]);
  expect(walker[0].points).toEqual([[42, 8]]);
});

test("大关卡：5000 个实体的场景树、2000 个里选 300 个换父节点、带子孙都是线性的，很快算完", () => {
  const base = blankLevel();
  const many = (n: number): LevelFile => ({
    ...base,
    entities: [
      ...base.entities,
      // 一半挂在前一个分组下面，形成几层
      ...Array.from({ length: n }, (_, i): Entity => ({ id: `c${i}`, type: i % 2 === 0 ? "group" : "coin", x: i, y: 0, ...(i % 2 === 1 ? { parent: `c${i - 1}` } : {}) })),
    ],
  });
  const big = many(5000);
  let t = Date.now();
  expect(treeRows(big, new Set()).length).toBe(5003);
  expect(withDescendants(big, big.entities.map((e) => e.id)).length).toBe(5003);
  expect(Date.now() - t).toBeLessThan(300);
  const mid = many(2000);
  const ids = mid.entities.slice(3, 303).map((e) => e.id);
  t = Date.now();
  const cmd = setParent(mid, ids, "e1");
  expect(Date.now() - t).toBeLessThan(300);
  // 选中的里面有父子的只挪最上面的：150 个分组
  expect(cmd!.apply(mid).entities.filter((e) => e.parent === "e1").length).toBe(150);
});

test("属性面板加覆盖：路径里有 __proto__ 这一段的不收", () => {
  expect(parseOverride("__proto__", "y", "1").ok).toBe(false);
  expect(parseOverride("b/__proto__", "y", "1").ok).toBe(false);
  expect(parseOverride("b/__crown", "y", "1").ok).toBe(true);
});

test("父节点不在关卡里的（不该有，保险）：场景树里当最外层显示，不丢", () => {
  const l = level();
  const orphan = { ...l, entities: [...l.entities, { id: "o", type: "coin", x: 0, y: 0, parent: "gone" }] };
  expect(treeRows(orphan, new Set()).find((r) => r.id === "o")).toEqual({ id: "o", depth: 0, parent: false, collapsed: false });
});
