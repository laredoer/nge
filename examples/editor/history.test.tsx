// 命令和撤销 / 重做（history.ts、commands.ts）：都是纯函数，不改传进去的关卡。
import { expect, test } from "nsh/test";
import { emptyLevel, type LevelFile } from "../../ts/level";
import { addEntities, compose, moveTo, paintTiles, removeEntities, reorder, resizeLayer, setLevel, updateEntity } from "./commands";
import { execute, HISTORY_LIMIT, isDirty, markSaved, newEditor, redo, undo, type Editor } from "./history";

const level = (): LevelFile => ({
  ...emptyLevel({ name: "测试", cols: 4, rows: 3, tile: 32, texture: "t", grid: [3, 1], solid: [0] }),
  entities: [
    { id: "a", type: "coin", x: 0, y: 0 },
    { id: "b", type: "enemy", x: 10, y: 0, props: { patrol: 3 } },
    { id: "c", type: "coin", x: 20, y: 0, name: "三号" },
  ],
});
const ids = (ed: Editor) => ed.level.entities.map((e) => e.id);
const frozen = <T,>(v: T): T => {
  const walk = (o: unknown) => {
    if (typeof o === "object" && o !== null) {
      Object.freeze(o);
      Object.values(o).forEach(walk);
    }
  };
  walk(v);
  return v;
};

test("执行、撤销、重做：撤销回到原样，重做再来一次", () => {
  const start = frozen(level());
  let ed = newEditor(start);
  ed = execute(ed, addEntities([{ id: "d", type: "coin", x: 5, y: 5 }]));
  expect(ids(ed)).toEqual(["a", "b", "c", "d"]);
  ed = undo(ed);
  expect(ed.level).toEqual(start);
  ed = redo(ed);
  expect(ids(ed)).toEqual(["a", "b", "c", "d"]);
  // 没有可撤销 / 重做的时候原样返回
  expect(redo(ed)).toBe(ed);
  expect(undo(undo(newEditor(start))).level).toBe(start);
});

test("新命令清掉重做栈", () => {
  let ed = newEditor(frozen(level()));
  ed = execute(ed, addEntities([{ id: "d", type: "coin", x: 0, y: 0 }]));
  ed = undo(ed);
  ed = execute(ed, addEntities([{ id: "e", type: "coin", x: 0, y: 0 }]));
  expect(redo(ed)).toBe(ed);
  expect(ids(ed)).toEqual(["a", "b", "c", "e"]);
});

test("删掉的撤销时放回原来的位置", () => {
  const start = frozen(level());
  let ed = execute(newEditor(start), removeEntities(start, ["a", "c"]));
  expect(ids(ed)).toEqual(["b"]);
  ed = undo(ed);
  expect(ed.level).toEqual(start);
});

test("移动：一组实体到新位置，撤销回去", () => {
  const start = frozen(level());
  let ed = execute(newEditor(start), moveTo(start, [{ id: "a", x: 32, y: 64 }, { id: "c", x: 1, y: 2 }]));
  expect(ed.level.entities.map((e) => [e.x, e.y])).toEqual([[32, 64], [10, 0], [1, 2]]);
  ed = undo(ed);
  expect(ed.level).toEqual(start);
});

test("改字段：重命名、改属性、去掉字段（写 undefined），撤销都还原", () => {
  const start = frozen(level());
  let ed = execute(newEditor(start), updateEntity(start, "c", { name: undefined, w: 40 }));
  expect(ed.level.entities[2]).toEqual({ id: "c", type: "coin", x: 20, y: 0, w: 40 });
  ed = execute(ed, updateEntity(ed.level, "b", { props: { patrol: 5 } }));
  expect(ed.level.entities[1].props).toEqual({ patrol: 5 });
  ed = undo(undo(ed));
  expect(ed.level).toEqual(start);
});

test("排序：移到第几个，撤销放回", () => {
  const start = frozen(level());
  let ed = execute(newEditor(start), reorder(start, "a", 2));
  expect(ids(ed)).toEqual(["b", "c", "a"]);
  ed = execute(ed, reorder(ed.level, "a", 0));
  expect(ids(ed)).toEqual(["a", "b", "c"]);
  ed = undo(ed);
  expect(ids(ed)).toEqual(["b", "c", "a"]);
  // 越界的按两头算
  expect(ids(execute(newEditor(start), reorder(start, "c", -5)))).toEqual(["c", "a", "b"]);
});

test("画瓦片：一组格子，撤销回到原来的帧号", () => {
  const start = frozen(level());
  let ed = execute(newEditor(start), paintTiles(0, [{ r: 2, c: 0, before: -1, after: 1 }, { r: 2, c: 3, before: -1, after: 2 }]));
  expect(ed.level.tiles[0].data[2]).toEqual([1, -1, -1, 2]);
  expect(start.tiles[0].data[2]).toEqual([-1, -1, -1, -1]);
  ed = undo(ed);
  expect(ed.level).toEqual(start);
});

test("改瓦片层大小：底边不动，多出来的是空格，撤销整层还原", () => {
  const start = frozen(level());
  let ed = execute(newEditor(start), paintTiles(0, [{ r: 2, c: 0, before: -1, after: 1 }]));
  const before = ed.level;
  ed = execute(ed, resizeLayer(ed.level, 0, 5, 4));
  const t = ed.level.tiles[0];
  expect([t.cols, t.rows, t.y]).toEqual([5, 4, 96]);
  expect(t.data[3]).toEqual([1, -1, -1, -1, -1]);
  ed = execute(ed, resizeLayer(ed.level, 0, 2, 1));
  expect(ed.level.tiles[0].data).toEqual([[1, -1]]);
  expect(ed.level.tiles[0].y).toBe(0);
  ed = undo(undo(ed));
  expect(ed.level).toEqual(before);
});

test("关卡设置和合成命令：一步撤销", () => {
  const start = frozen(level());
  let ed = execute(newEditor(start), compose("一组", [setLevel(start, { name: "新名字", night: true }), addEntities([{ id: "z", type: "coin", x: 0, y: 0 }], 0)]));
  expect([ed.level.name, ed.level.night, ids(ed)[0]]).toEqual(["新名字", true, "z"]);
  expect(ed.history.done.at(-1)!.cmd.label).toBe("一组");
  ed = undo(ed);
  expect(ed.level).toEqual(start);
});

test("未保存的修改：保存后干净，改了脏，撤销回保存的那一步又干净；保存后撤销再做新的永远是脏的", () => {
  let ed = newEditor(frozen(level()));
  expect(isDirty(ed)).toBe(false);
  ed = execute(ed, addEntities([{ id: "d", type: "coin", x: 0, y: 0 }]));
  expect(isDirty(ed)).toBe(true);
  ed = markSaved(ed);
  expect(isDirty(ed)).toBe(false);
  ed = execute(ed, addEntities([{ id: "e", type: "coin", x: 0, y: 0 }]));
  expect(isDirty(ed)).toBe(true);
  ed = undo(ed);
  expect(isDirty(ed)).toBe(false);
  ed = undo(ed);
  expect(isDirty(ed)).toBe(true);
  ed = execute(ed, addEntities([{ id: "f", type: "coin", x: 0, y: 0 }]));
  expect(isDirty(ed)).toBe(true);
  ed = undo(ed);
  expect(isDirty(ed)).toBe(true);
});

test("同一组实体 600 毫秒内的微移合并成一步", () => {
  const start = frozen(level());
  const nudge = (ed: Editor, at: number) => execute(ed, moveTo(ed.level, [{ id: "a", x: ed.level.entities[0].x + 1, y: 0 }], "nudge:a"), at);
  let ed = nudge(nudge(nudge(newEditor(start), 0), 100), 200);
  expect(ed.level.entities[0].x).toBe(3);
  expect(ed.history.done.length).toBe(1);
  // 隔久了另起一步
  ed = nudge(ed, 2000);
  expect(ed.history.done.length).toBe(2);
  ed = undo(ed);
  expect(ed.level.entities[0].x).toBe(3);
  ed = undo(ed);
  expect(ed.level).toEqual(start);
  // 合并后保存过的那一步也算改了
  let saved = markSaved(nudge(newEditor(start), 0));
  saved = nudge(saved, 100);
  expect(isDirty(saved)).toBe(true);
});

test("历史最多 200 步，最早的丢掉", () => {
  let ed = newEditor(frozen(level()));
  for (let i = 0; i < HISTORY_LIMIT + 10; i++) ed = execute(ed, moveTo(ed.level, [{ id: "a", x: i, y: 0 }]));
  expect(ed.history.done.length).toBe(HISTORY_LIMIT);
  for (let i = 0; i < HISTORY_LIMIT + 10; i++) ed = undo(ed);
  expect(ed.level.entities[0].x).toBe(9);
  // 撤销到底也不是新建时的样子：还是改过的
  expect(isDirty(ed)).toBe(true);
});

test("找不到的 id 不出错，什么也不变", () => {
  const start = frozen(level());
  expect(execute(newEditor(start), moveTo(start, [{ id: "nope", x: 1, y: 1 }])).level.entities).toEqual(start.entities);
  expect(execute(newEditor(start), removeEntities(start, ["nope"])).level.entities).toEqual(start.entities);
  expect(execute(newEditor(start), updateEntity(start, "nope", { name: "x" })).level.entities).toEqual(start.entities);
});
