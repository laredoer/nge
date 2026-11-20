// 多个瓦片层、矩形填充和油漆桶、瓦片规则、视差层的拾取（docs/editor-more.md）。
import { expect, test } from "nsh/test";
import { emptyLevel, formatLevel, parseLevel, type LevelFile, type TileLayer } from "../../ts/level";
import { paintTiles } from "./commands";
import { changesOf, floodFill, rectCells } from "./fill";
import { execute, newEditor, undo } from "./history";
import { addLayer, frameRule, layerPoint, moveLayer, removeLayer, rulePatch, updateLayer } from "./layers";

const level = (): LevelFile => {
  const l = emptyLevel({ name: "", cols: 4, rows: 3, tile: 32, texture: "t", grid: [8, 1], solid: [0, 1], oneWay: [3], slopes: [{ frame: 4, left: 0, right: 1 }] });
  const data = [
    [-1, -1, 2, -1],
    [-1, 1, 2, -1],
    [1, 1, 1, 1],
  ];
  return { ...l, tiles: [{ ...l.tiles[0], data }] };
};

test("加一层：和当前层一样大、图集和规则一样、全空，插在它后面，id 不重；撤销去掉", () => {
  const l = level();
  const cmd = addLayer(l, 0);
  const two = cmd.apply(l);
  expect(two.tiles.map((t) => t.id)).toEqual(["level", "layer2"]);
  expect(two.tiles[1].data).toEqual([[-1, -1, -1, -1], [-1, -1, -1, -1], [-1, -1, -1, -1]]);
  expect([two.tiles[1].cols, two.tiles[1].rows, two.tiles[1].texture, two.tiles[1].oneWay]).toEqual([4, 3, "t", [3]]);
  expect(two.tiles[1].name).toBe("第 2 层");
  const three = addLayer(two, 0).apply(two);
  expect(three.tiles.map((t) => t.id)).toEqual(["level", "layer3", "layer2"]);
  expect(cmd.revert(two)).toEqual(l);
  // 存得下来
  expect(parseLevel(formatLevel(three)).ok).toBe(true);
});

test("删一层（最后一层不能删），撤销放回原位；挪层，到头了不动", () => {
  const l = addLayer(level(), 0).apply(level());
  expect(removeLayer(level(), 0)).toBe(null);
  const del = removeLayer(l, 0)!;
  const one = del.apply(l);
  expect(one.tiles.map((t) => t.id)).toEqual(["layer2"]);
  expect(del.revert(one)).toEqual(l);
  expect(moveLayer(l, 0, -1)).toBe(null);
  expect(moveLayer(l, 1, 2)).toBe(null);
  const moved = moveLayer(l, 0, 1)!;
  expect(moved.apply(l).tiles.map((t) => t.id)).toEqual(["layer2", "level"]);
  expect(moved.revert(moved.apply(l))).toEqual(l);
});

test("改层的字段：名字、显隐、锁定、视差、z；undefined 去掉；撤销回原样", () => {
  const l = level();
  const cmd = updateLayer(l, 0, { name: "地面", hidden: true, parallax: [0.5, 1], z: -3 });
  const next = cmd.apply(l);
  expect(next.tiles[0]).toMatchObject({ name: "地面", hidden: true, parallax: [0.5, 1], z: -3 });
  const cleared = updateLayer(next, 0, { hidden: undefined }).apply(next);
  expect("hidden" in cleared.tiles[0]).toBe(false);
  expect(cmd.revert(next)).toEqual(l);
});

test("矩形填充：两格围成的矩形，反着拉也一样；已经是这一帧的、层外面的不记", () => {
  expect(rectCells({ r: 2, c: 3 }, { r: 1, c: 2 })).toEqual([{ r: 1, c: 2 }, { r: 1, c: 3 }, { r: 2, c: 2 }, { r: 2, c: 3 }]);
  const t = level().tiles[0];
  expect(changesOf(t, rectCells({ r: 1, c: 2 }, { r: 2, c: 4 }), 1)).toEqual([{ r: 1, c: 2, before: 2, after: 1 }, { r: 1, c: 3, before: -1, after: 1 }]);
});

test("油漆桶：四连通、和起点一样的才换；斜着挨着的不算；起点已经是这一帧什么都不做", () => {
  const t = level().tiles[0];
  const changes = floodFill(t, { r: 0, c: 0 }, 5);
  const cells = changes.map((c) => `${c.r},${c.c}`).sort();
  // 左上角的空格：(0,0) (0,1) (1,0)；(0,3)(1,3) 被木板隔开
  expect(cells).toEqual(["0,0", "0,1", "1,0"]);
  expect(changes.every((c) => c.before === -1 && c.after === 5)).toBe(true);
  expect(floodFill(t, { r: 2, c: 0 }, 1)).toEqual([]);
  expect(floodFill(t, { r: 9, c: 0 }, 1)).toEqual([]);
  // 撤销一步回到原样
  const l = level();
  const ed = execute(newEditor(l), paintTiles(0, floodFill(t, { r: 2, c: 0 }, 0)));
  expect(ed.level.tiles[0].data[2]).toEqual([0, 0, 0, 0]);
  // (1, 1) 和底行挨着，一起换；木板不是同一帧，不换
  expect(ed.level.tiles[0].data[1]).toEqual([-1, 0, 2, -1]);
  expect(undo(ed).level).toEqual(l);
});

test("油漆桶在大地图上不爆栈", () => {
  const big = emptyLevel({ name: "", cols: 1000, rows: 200, tile: 32, texture: "t", grid: [3, 1], solid: [] }).tiles[0];
  expect(floodFill(big, { r: 0, c: 0 }, 1).length).toBe(200000);
});

test("一帧的规则：读出种类、坡、动画、材质；改种类时从别的列表里去掉，空了的列表去掉", () => {
  const t: TileLayer = { ...level().tiles[0], animated: [{ frame: 5, frames: [5, 6], fps: 4 }], materials: [{ frame: 1, friction: 0.1 }] };
  expect(frameRule(t, 0)).toEqual({ kind: "solid" });
  expect(frameRule(t, 1)).toEqual({ kind: "solid", friction: 0.1 });
  expect(frameRule(t, 3)).toEqual({ kind: "oneWay" });
  expect(frameRule(t, 4)).toEqual({ kind: "slope", slope: { left: 0, right: 1 } });
  expect(frameRule(t, 5)).toEqual({ kind: "none", animation: { frames: [5, 6], fps: 4 } });
  // 单向改成实心
  const a = rulePatch(t, 3, { kind: "solid" });
  expect([a.solid, a.oneWay]).toEqual([[0, 1, 3], undefined]);
  // 实心改成斜坡，带两头的高
  const b = rulePatch(t, 1, { kind: "slope", slope: { left: 0.5, right: 1 }, friction: 0.1 });
  expect([b.solid, b.slopes]).toEqual([[0], [{ frame: 1, left: 0.5, right: 1 }, { frame: 4, left: 0, right: 1 }]]);
  expect(b.materials).toEqual([{ frame: 1, friction: 0.1 }]);
  // 去掉动画和材质
  const c = rulePatch(t, 5, { kind: "none" });
  expect([c.animated, c.materials]).toEqual([undefined, [{ frame: 1, friction: 0.1 }]]);
  const d = rulePatch(t, 1, { kind: "solid", restitution: 0.5 });
  expect(d.materials).toEqual([{ frame: 1, restitution: 0.5 }]);
  // 改完的层存得下来
  const l = level();
  const next = updateLayer({ ...l, tiles: [t] }, 0, b).apply({ ...l, tiles: [t] });
  expect(parseLevel(formatLevel(next)).ok).toBe(true);
  expect(frameRule(next.tiles[0], 1)).toEqual({ kind: "slope", slope: { left: 0.5, right: 1 }, friction: 0.1 });
});

test("视差层上的位置：世界坐标减去相机 ×（1 − 视差），没有视差不变", () => {
  const t = level().tiles[0];
  const view = { x: 100, y: 40, zoom: 1 };
  expect(layerPoint(t, view, [10, 20])).toEqual([10, 20]);
  expect(layerPoint({ ...t, parallax: [0.5, 1] }, view, [10, 20])).toEqual([-40, 20]);
  expect(layerPoint({ ...t, parallax: [0, 0.5] }, view, [10, 20])).toEqual([-90, 0]);
});
