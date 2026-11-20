// 试玩热重载（docs/editor-more.md）：瓦片地图只有格子变了时声明不动、变了的格子用 setTiles 发；字段变了整张换。
import { expect, test } from "nsh/test";
import type { TilemapNode } from "../../ts/nge";
import { liveTiles, startLive } from "./live";

const map = (data: number[], extra: Partial<TilemapNode> = {}): TilemapNode => ({ id: "level", texture: "t", tile: 32, cols: 3, data, solid: [0], ...extra });

test("只改了格子：声明还是最初的，改了的格子按 [列, 行, 帧号] 发", () => {
  const first = [map([0, -1, -1, 1, 1, 1])];
  const live = startLive(first);
  const r = liveTiles(live, [map([0, 2, -1, 1, -1, 1])]);
  expect(r.maps.declared).toEqual(first);
  expect(r.edits).toEqual([{ id: "level", tiles: [[1, 0, 2], [1, 1, -1]] }]);
  // 再改回去：按引擎现在显示的比，不是按声明比
  const back = liveTiles(r.maps, [map([0, -1, -1, 1, 1, 1])]);
  expect(back.edits).toEqual([{ id: "level", tiles: [[1, 0, -1], [1, 1, 1]] }]);
  expect(back.maps.declared).toEqual(first);
  // 没变：什么都不发
  expect(liveTiles(back.maps, [map([0, -1, -1, 1, 1, 1])]).edits).toEqual([]);
});

test("地图的字段变了（规则、视差、大小）或者加了层：声明新的，引擎整张重建，不发 setTiles", () => {
  const live = startLive([map([0, -1, -1, 1, 1, 1])]);
  const ruled = liveTiles(live, [map([0, 2, -1, 1, 1, 1], { oneWay: [2] })]);
  expect(ruled.edits).toEqual([]);
  expect(ruled.maps.declared[0].oneWay).toEqual([2]);
  expect(ruled.maps.declared[0].data[1]).toBe(2);
  const taller = liveTiles(ruled.maps, [map([0, 2, -1, 1, 1, 1, 1, 1, 1], { oneWay: [2] })]);
  expect(taller.edits).toEqual([]);
  expect(taller.maps.declared[0].data.length).toBe(9);
  const more = liveTiles(taller.maps, [taller.maps.shown[0], map([1, 1, 1], { id: "far" })]);
  expect(more.maps.declared.map((m) => m.id)).toEqual(["level", "far"]);
  expect(more.edits).toEqual([]);
  // 加的那层之后只改格子：照样走 setTiles，声明留着
  const edit = liveTiles(more.maps, [more.maps.shown[0], map([1, -1, 1], { id: "far" })]);
  expect(edit.edits).toEqual([{ id: "far", tiles: [[1, 0, -1]] }]);
  expect(edit.maps.declared[1].data).toEqual([1, 1, 1]);
});

test("关卡还没建好时不能发 setTiles：直接声明新的", () => {
  const live = startLive([map([0, -1, -1, 1, 1, 1])]);
  const r = liveTiles(live, [map([0, 2, -1, 1, 1, 1])], false);
  expect(r.edits).toEqual([]);
  expect(r.maps.declared[0].data[1]).toBe(2);
});
