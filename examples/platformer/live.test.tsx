// 试玩热重载（docs/editor-more.md）：瓦片地图只有格子变了时声明不动、变了的格子用 setTiles 发；字段变了整张换。
import { expect, test } from "nsh/test";
import type { TilemapNode } from "../../ts/nge";
import { build, MEADOW, type Built } from "./levels";
import { enemyTweenOf, followStep, liftTweenOf, liveTiles, restartFollow, startFollow, startLive } from "./live";

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

// ---- 重开、补间、id ----

const built = (over: Partial<Built> = {}): Built => ({ ...build(MEADOW), lifts: [], enemies: [], ...over });

test("地图的 id 不一样（同一个位置换了一层）：声明新的，不往别的层上发 setTiles", () => {
  const live = startLive([map([0, -1, -1, 1, 1, 1])]);
  const r = liveTiles(live, [map([0, 2, -1, 1, 1, 1], { id: "other" })]);
  expect(r.edits).toEqual([]);
  expect(r.maps.declared[0].id).toBe("other");
});

test("热重载中整关重开（通关、血掉光）：重开后声明就是现在的完整地图，之后照样只发变了的格子", () => {
  const first = built({ tilemaps: [map([0, -1, -1, 1, 1, 1])] });
  let f = startFollow(first);
  const edited = followStep(f, { ...first, tilemaps: [map([0, 2, -1, 1, 1, 1])] }, true);
  expect(edited.edits).toEqual([{ id: "level", tiles: [[1, 0, 2]] }]);
  expect(edited.tilemaps[0].data[1]).toBe(-1);
  f = restartFollow(edited.follow);
  // 引擎整关重建时按声明建：声明里要是现在的格子，不然 setTiles 发过的改动就丢了
  expect(f.maps.declared).toEqual(f.maps.shown);
  expect(f.maps.declared[0].data[1]).toBe(2);
  // 重开后撤销：按重建后的样子比，发回 -1
  const undone = followStep(f, { ...first, tilemaps: [map([0, -1, -1, 1, 1, 1])] }, true);
  expect(undone.edits).toEqual([{ id: "level", tiles: [[1, 0, -1]] }]);
  expect(undone.tilemaps[0].data[1]).toBe(2);
});

test("补间带着出生点做起点：改了巡逻格数、平台距离后重发的补间还是以出生点为准；只发变了的", () => {
  const lift = { node: { id: "lift:a", x: 64, y: 10, w: 96, h: 16 }, to: { y: 100 }, duration: 3 };
  expect(liftTweenOf(lift)).toEqual({ id: "lift:a", from: { y: 10 }, to: { y: 100 }, duration: 3, ease: "easeInOut", repeat: -1, yoyo: true });
  expect(liftTweenOf({ ...lift, to: { x: 200 } }).from).toEqual({ x: 64 });
  const enemy = { id: "enemy-b", x: 200, y: 12, to: { x: 264 } };
  expect(enemyTweenOf(enemy)).toEqual({ id: "enemy-b", from: { x: 200 }, to: { x: 264 }, duration: 1.5, ease: "linear", repeat: -1, yoyo: true });
  const first = built({ lifts: [lift], enemies: [enemy, { id: "enemy-c", x: 400, y: 12, to: { x: 464 } }] });
  const f = startFollow(first);
  const farther = { ...enemy, to: { x: 328 } };
  const r = followStep(f, { ...first, enemies: [farther, first.enemies[1]] }, true);
  expect(r.tweens).toEqual([enemyTweenOf(farther)]);
  // 关卡还没建好时不发（levelStart 时统一发）
  expect(followStep(f, { ...first, enemies: [farther, first.enemies[1]] }, false).tweens).toEqual([]);
});
