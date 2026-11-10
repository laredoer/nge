// 编辑器的操作（store.ts）：撤销重做、复制、微移、属性、关卡设置、文件、试玩。
import { expect, test } from "nsh/test";
import { parseLevel } from "../../ts/level";
import { memoryStore } from "./files";
import { blankLevel, createStore, EDIT_MAX_COLS, EDIT_MAX_ROWS, type Ask } from "./store";

const setup = (answers: (string | null)[] = [], files: Record<string, string> = {}) => {
  const disk = memoryStore(files);
  const asked: string[] = [];
  const ask: Ask = async (title) => {
    asked.push(title);
    return answers.shift() ?? null;
  };
  let t = 0;
  // 每步隔 1 秒，不会被合并
  const store = createStore({ files: async () => disk, ask, now: () => (t += 1000) });
  return { store, disk, asked };
};

test("新建的关卡能直接试玩", () => {
  const { store } = setup();
  expect(store.dirty()).toBe(false);
  store.play();
  expect(store.playing()?.built.goal.x).toBe(1792);
  store.stop();
  expect(store.playing()).toBe(null);
});

test("删除、撤销、重做，选中跟着走", () => {
  const { store } = setup();
  store.selection.set(["e2"]);
  store.remove();
  expect(store.entity("e2")).toBe(undefined);
  expect(store.selection()).toEqual([]);
  expect(store.dirty()).toBe(true);
  store.undo();
  expect(store.entity("e2")?.type).toBe("crate");
  store.redo();
  expect(store.entity("e2")).toBe(undefined);  // 撤销了添加：选中的跟着去掉
  store.addAtCenter("coin");
  expect(store.selection()).toEqual(["e4"]);
  store.undo();
  expect(store.selection()).toEqual([]);
});

test("复制一份：错开一格，选中新的，名字带副本", () => {
  const { store } = setup();
  store.selection.set(["e2"]);
  store.duplicate();
  const [id] = store.selection();
  expect(id).toBe("e4");
  expect(store.entity(id)!.x).toBe(store.entity("e2")!.x + 32);
  store.copy();
  store.paste();
  expect(store.selection()).toEqual(["e5"]);
  expect(store.level().entities.length).toBe(5);
});

test("连着微移合成一步撤销；没选中时平移视图", () => {
  const { store } = setup();
  const x = store.entity("e2")!.x;
  // 合并要在 MERGE_MS 之内：用同一时刻
  const fast = createStoreAtSameTime();
  fast.selection.set(["e2"]);
  fast.nudge(1, 0);
  fast.nudge(1, 0);
  fast.nudge(1, 0);
  expect(fast.entity("e2")!.x).toBe(x + 3);
  fast.undo();
  expect(fast.entity("e2")!.x).toBe(x);
  fast.selection.set([]);
  const vx = fast.view().x;
  fast.nudge(-1, 0);
  expect(fast.view().x).toBe(vx - 4);
});

function createStoreAtSameTime() {
  return createStore({ files: async () => memoryStore(), ask: async () => null, now: () => 0 });
}

test("站在地上的东西微移时高度不变", () => {
  const { store } = setup();
  store.addAtCenter("tower");
  const [id] = store.selection();
  const y = store.entity(id)!.y;
  store.nudge(0, 5);
  expect(store.entity(id)!.y).toBe(y);
});

test("从物件库放到视图中间，吸附到网格", () => {
  const { store } = setup();
  store.view.set({ x: 500, y: 210, zoom: 1 });
  store.addAtCenter("coin");
  const [id] = store.selection();
  // 金币 14 比网格小：吸到格子中间
  expect([store.entity(id)!.x, store.entity(id)!.y]).toEqual([496, 208]);
  expect(store.placeType()).toBe("coin");
});

test("属性：不合规的不改并给出原因，没变化的不进历史", () => {
  const { store } = setup();
  expect(store.setField("e2", "x", "abc")).toBe("要是数");
  expect(store.setField("e2", "w", "10")).toBe("这种东西的大小是固定的");
  expect(store.dirty()).toBe(false);
  expect(store.setField("e2", "x", String(store.entity("e2")!.x))).toBe(null);
  expect(store.dirty()).toBe(false);
  expect(store.setField("e2", "x", "640")).toBe(null);
  expect(store.entity("e2")!.x).toBe(640);
  store.addAtCenter("enemy");
  const [id] = store.selection();
  expect(store.setProp(id, "patrol", "99")).toBe("要在 1 到 20 之间");
  expect(store.setProp(id, "patrol", "5")).toBe(null);
  expect(store.entity(id)!.props!.patrol).toBe(5);
});

test("关卡设置：改名、夜里、宽高（不合规的不改）", () => {
  const { store } = setup();
  store.setLevelField("name", "试一关");
  store.setLevelField("night", "true");
  expect([store.level().name, store.level().night]).toEqual(["试一关", true]);
  expect(store.setLevelField("cols", "0")).toBe(`要在 1 到 ${EDIT_MAX_COLS} 之间`);
  expect(store.setLevelField("rows", "20")).toBe(null);
  expect([store.level().tiles[0].cols, store.level().tiles[0].rows]).toEqual([60, 20]);
  store.undo();
  expect(store.level().tiles[0].rows).toBe(15);
});

test("保存：没文件名先另存为；格式化的 JSON 能读回来；保存后不脏", async () => {
  const { store, disk } = setup();
  store.setLevelField("name", "改过");
  expect(await store.save()).toBe(false);
  expect(store.panel()).toBe("saveAs");
  expect(await store.saveAs("../坏")).toBe(false);
  expect(await store.saveAs("我的关卡")).toBe(true);
  expect(store.file()).toBe("我的关卡.json");
  expect(store.dirty()).toBe(false);
  const text = disk.files["我的关卡.json"];
  expect(text.includes("\n")).toBe(true);
  const r = parseLevel(JSON.parse(text));
  expect(r.ok && r.level.name).toBe("改过");
});

test("另存为已有的文件要问；取消就不写", async () => {
  const { store, disk, asked } = setup(["cancel", "overwrite"], { "a.json": "旧的" });
  expect(await store.saveAs("a")).toBe(false);
  expect(disk.files["a.json"]).toBe("旧的");
  expect(await store.saveAs("a")).toBe(true);
  expect(asked).toEqual(["a.json 已经有了", "a.json 已经有了"]);
  expect(disk.files["a.json"].startsWith("{")).toBe(true);
});

test("有没保存的修改：新建前要问，取消就留着，不保存就丢掉", async () => {
  const { store, asked } = setup(["cancel", "discard"]);
  store.selection.set(["e1"]);
  store.remove();
  await store.newLevel();
  expect(store.entity("e1")).toBe(undefined);
  await store.newLevel();
  expect(store.entity("e1")?.type).toBe("spawn");
  expect(store.dirty()).toBe(false);
  expect(asked.length).toBe(2);
});

test("打开：坏文件不崩，状态栏说原因，当前关卡不变", async () => {
  const good = JSON.stringify({ ...blankLevel(), name: "好的" });
  const { store } = setup([], { "bad.json": "{不是 JSON", "good.json": good });
  await store.open("bad.json");
  expect(store.status().startsWith("打不开 bad.json")).toBe(true);
  expect(store.level().name).toBe("新关卡");
  await store.open("good.json");
  expect([store.level().name, store.file(), store.dirty()]).toEqual(["好的", "good.json", false]);
  await store.refreshFiles();
  expect(store.files()).toEqual(["bad.json", "good.json"]);
});

test("不能试玩的关卡不进试玩，状态栏说原因", () => {
  const { store } = setup();
  store.selection.set(["e3"]);
  store.remove();
  store.play();
  expect(store.playing()).toBe(null);
  expect(store.status().includes("终点要有 1 个")).toBe(true);
});

test("画布：点选、shift 加选、拖动移动后一步撤销", () => {
  const { store } = setup();
  store.view.set({ x: 0, y: 0, zoom: 1 });
  store.size.set([800, 600]);
  // 世界 (x, y) 在屏幕上是 (400 + x, 300 - y)
  const at = (id: string): [number, number] => [400 + store.entity(id)!.x, 300 - store.entity(id)!.y];
  store.pointer("down", at("e1"));
  store.pointer("up", at("e1"));
  expect(store.selection()).toEqual(["e1"]);
  store.view.set({ x: 1600, y: 0, zoom: 1 });
  const at2 = (id: string): [number, number] => [400 + store.entity(id)!.x - 1600, 300 - store.entity(id)!.y];
  store.pointer("down", at2("e2"), true);
  store.pointer("up", at2("e2"), true);
  expect(store.selection()).toEqual(["e1", "e2"]);
  store.pointer("down", at2("e2"), true);
  store.pointer("up", at2("e2"), true);
  expect(store.selection()).toEqual(["e1"]);
  // 拖箱子往右：箱子 40 宽，左边吸到网格上（1580 → 1632）
  const [sx, sy] = at2("e2");
  const x = store.entity("e2")!.x;
  store.pointer("down", [sx, sy]);
  store.pointer("move", [sx + 40, sy]);
  expect(store.sceneInput().level.entities.find((e) => e.id === "e2")!.x).toBe(x + 52);
  expect(store.entity("e2")!.x).toBe(x);
  store.pointer("up", [sx + 64, sy]);
  expect(store.entity("e2")!.x).toBe(x + 52);
  store.undo();
  expect(store.entity("e2")!.x).toBe(x);
});

test("画布：刷子画一笔是一步，框选框画在场景里", () => {
  const { store } = setup();
  store.tool.set("brush");
  store.frame.set(2);
  // 第 0 行在 y 416 到 448；屏幕 (416, 316) 是世界 (16, 432)
  store.view.set({ x: 0, y: 448, zoom: 1 });
  store.pointer("down", [416, 316]);
  store.pointer("move", [480, 316]);
  store.pointer("up", [480, 316]);
  const row = store.level().tiles[0].data[0];
  expect(row.slice(0, 4)).toEqual([2, 2, 2, -1]);
  store.undo();
  expect(store.level().tiles[0].data[0][0]).toBe(-1);
  store.tool.set("select");
  store.pointer("down", [10, 10]);
  store.pointer("move", [60, 60]);
  expect(store.sceneInput().box).not.toBe(null);
  store.pointer("cancel", [60, 60]);
  expect(store.sceneInput().box).toBe(null);
});

test("属性：站在地上的不能改高度，固定大小的不能改宽高，名字带控制字符的不改", () => {
  const { store } = setup();
  store.addAtCenter("tower");
  const [id] = store.selection();
  expect(store.setField(id, "y", "100")).toBe("站在地上的东西不能改高度");
  expect(store.setField(id, "name", "a\u0007")).toBe("名字太长或有控制字符");
  store.addAtCenter("lift");
  const [lift] = store.selection();
  expect(store.setField(lift, "w", "96")).toBe(null);
  expect(store.entity(lift)!.w).toBe(96);
  expect(store.setField(lift, "w", "0")).toBe("要在 1 到 1000000 之间");
});

test("关卡名太长、带控制字符的不改；存不回来的关卡不保存", async () => {
  const { store, disk } = setup();
  expect(store.setLevelField("name", "x".repeat(300))).toBe("名字太长或有控制字符");
  expect(store.setLevelField("name", "a\nb")).toBe("名字太长或有控制字符");
  expect(store.level().name).toBe("新关卡");
  // 绕过界面塞进一个坏名字：保存前读一遍，不通过就不写
  store.editor.set({ ...store.editor(), level: { ...store.level(), name: "坏\u0007" } });
  expect(await store.saveAs("bad")).toBe(false);
  expect(disk.files["bad.json"]).toBe(undefined);
  expect(store.status().startsWith("不能保存")).toBe(true);
});

test("连着微移上万次，撤销一步回到原处，不爆栈", () => {
  let t = 0;
  const store = createStore({ files: async () => memoryStore(), ask: async () => null, now: () => (t += 10) });
  store.selection.set(["e2"]);
  const x = store.entity("e2")!.x;
  for (let i = 0; i < 20000; i++) store.nudge(1, 0);
  expect(store.entity("e2")!.x).toBe(x + 20000);
  store.undo();
  expect(store.entity("e2")!.x).toBe(x);
  store.redo();
  expect(store.entity("e2")!.x).toBe(x + 20000);
});

test("写盘时又改了：写完还是脏的；撤销回保存时的样子不脏", async () => {
  const disk = memoryStore();
  let release = () => {};
  const slow = { ...disk, write: (n: string, text: string) => new Promise<void>((res) => (release = () => ((disk.files[n] = text), res()))) };
  let t = 0;
  const store = createStore({ files: async () => slow, ask: async () => null, now: () => (t += 1000) });
  store.selection.set(["e2"]);
  const saving = store.saveAs("race");
  await new Promise((r) => setTimeout(r, 10));
  store.remove();
  release();
  expect(await saving).toBe(true);
  expect(disk.files["race.json"].includes('"id": "e2"')).toBe(true);
  expect(store.dirty()).toBe(true);
  store.undo();
  expect(store.dirty()).toBe(false);
});

test("拖动中删掉、撤销：拖动作废，不抛错", () => {
  const { store } = setup();
  store.size.set([800, 600]);
  store.view.set({ x: 96, y: 16, zoom: 1 });
  store.pointer("down", [400, 300]);
  store.pointer("move", [420, 300]);
  expect(store.drag().kind).toBe("move");
  store.remove();
  expect(store.drag().kind).toBe("idle");
  store.pointer("move", [440, 300]);
  store.pointer("up", [440, 300]);
  store.undo();
  expect(store.entity("e1")!.x).toBe(96);
  // 刷子画到一半撤销
  store.tool.set("brush");
  store.view.set({ x: 0, y: 448, zoom: 1 });
  store.pointer("down", [416, 316]);
  store.undo();
  expect(store.drag().kind).toBe("idle");
});

test("另存为：只差大小写的也算重名，覆盖时写回原来的文件名；列不出文件时不写", async () => {
  const { store, disk, asked } = setup(["cancel", "overwrite"], { "Stairs.json": "旧" });
  expect(await store.saveAs("stairs")).toBe(false);
  expect(Object.keys(disk.files)).toEqual(["Stairs.json"]);
  expect(await store.saveAs("stairs")).toBe(true);
  expect(Object.keys(disk.files)).toEqual(["Stairs.json"]);
  expect(store.file()).toBe("Stairs.json");
  expect(asked.length).toBe(2);
  const broken = { ...memoryStore(), list: () => Promise.reject(new Error("坏了")) };
  const s2 = createStore({ files: async () => broken, ask: async () => "overwrite" });
  expect(await s2.saveAs("x")).toBe(false);
  expect(broken.files["x.json"]).toBe(undefined);
});

test("⌘D 不动剪贴板", () => {
  const { store } = setup();
  store.selection.set(["e1"]);
  store.copy();
  store.selection.set(["e2"]);
  store.duplicate();
  store.paste();
  expect(store.entity(store.selection()[0])!.type).toBe("spawn");
});

test("没变化的操作不进历史、不标脏", () => {
  const { store } = setup();
  store.setLevelField("night", "false");
  store.selection.set(["e1"]);
  store.reorderBy("e1", -1);
  const last = store.level().entities.at(-1)!.id;
  store.reorderBy(last, 1);
  expect(store.dirty()).toBe(false);
  expect(store.editor().history.done.length).toBe(0);
  // 站在地上的按上下没变化
  store.addAtCenter("tower");
  store.nudge(0, 1);
  store.nudge(0, -1);
  expect(store.editor().history.done.length).toBe(1);
});

test("出生点、终点在地图外面的不进试玩", () => {
  const { store } = setup();
  store.setField("e3", "x", "-500");
  store.play();
  expect(store.playing()).toBe(null);
  expect(store.status().includes("终点在地图外面")).toBe(true);
  store.undo();
  store.setField("e1", "y", "5000");
  store.play();
  expect(store.status().includes("出生点在地图外面")).toBe(true);
});

test("编辑器里瓦片层不能超过编辑器的上限", () => {
  const { store } = setup();
  expect(store.setLevelField("cols", "1500")).toBe(`要在 1 到 ${EDIT_MAX_COLS} 之间`);
  expect(store.setLevelField("rows", "400")).toBe(`要在 1 到 ${EDIT_MAX_ROWS} 之间`);
});

test("叠在一起的：点中画在上面的；同种东西下移以后点中另一个，敌人总在金币上面", () => {
  const { store } = setup();
  store.view.set({ x: 500, y: 210, zoom: 1 });
  store.size.set([800, 600]);
  store.addAtCenter("coin");
  store.addAtCenter("coin");
  const click = () => {
    store.pointer("down", [400, 300]);
    store.pointer("up", [400, 300]);
    return store.selection()[0];
  };
  expect(click()).toBe("e5");
  store.reorderBy("e5", -1);
  expect(click()).toBe("e4");
  // 敌人画在金币上面：后放的金币也点不中，点中的是敌人
  store.addAtCenter("enemy");
  store.reorderBy("e6", -10);
  expect(click()).toBe("e6");
});
