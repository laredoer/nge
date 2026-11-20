// 编辑器的界面（App.tsx）：物件库、场景树、属性、快捷键、文件、试玩。
import { expect, open, test } from "nsh/test";

const start = () => open(".", { platform: "macos", width: 1280, height: 800 });
const has = (texts: string[], part: string) => texts.some((t) => t.includes(part));

test("打开就是一个能玩的新关卡：场景树里出生点、箱子、终点，右边是关卡设置", async () => {
  const app = await start();
  expect(app.shows("tree-e1")).toBe(true);
  expect(app.shows("tree-e3")).toBe(true);
  expect(has(app.texts(), "场景（3）")).toBe(true);
  expect(app.value("level-name")).toBe("新关卡");
  expect(app.value("level-cols")).toBe("60");
  expect(app.enabled("undo")).toBe(false);
});

test("从物件库放一个金币，改坐标：不合规的给出原因；撤销、重做", async () => {
  const app = await start();
  await app.press("lib-coin");
  expect(app.shows("tree-e4")).toBe(true);
  expect(app.shows("field-x")).toBe(true);
  expect(app.value("file")).toBe(null);
  expect(has(app.texts(), "未命名 ●")).toBe(true);
  await app.clear("field-x");
  await app.type("field-x", "abc\n");
  expect(has(app.texts(), "要是数")).toBe(true);
  await app.clear("field-x");
  await app.type("field-x", "320\n");
  expect(app.value("field-x")).toBe("320");
  await app.keyboard.press("mod+z");
  expect(app.value("field-x")).not.toBe("320");
  await app.keyboard.press("mod+z");
  expect(app.shows("tree-e4")).toBe(false);
  await app.keyboard.press("mod+shift+z");
  expect(app.shows("tree-e4")).toBe(true);
});

test("快捷键：Delete 删掉选中的，⌘D 复制一份，方向键挪一下", async () => {
  const app = await start();
  await app.press("tree-e2");
  const x = Number(app.value("field-x"));
  await app.keyboard.press("right");
  expect(app.value("field-x")).toBe(String(x + 1));
  await app.keyboard.press("mod+d");
  expect(app.shows("tree-e4")).toBe(true);
  await app.keyboard.press("delete");
  expect(app.shows("tree-e4")).toBe(false);
  expect(app.shows("tree-e2")).toBe(true);
});

test("属性里的选项：敌人的巡逻格数超范围不改", async () => {
  const app = await start();
  await app.press("lib-enemy");
  expect(app.value("prop-patrol")).toBe("3");
  await app.clear("prop-patrol");
  await app.type("prop-patrol", "50\n");
  expect(has(app.texts(), "要在 1 到 20 之间")).toBe(true);
  await app.clear("prop-patrol");
  await app.type("prop-patrol", "6\n");
  expect(app.value("prop-patrol")).toBe("6");
});

test("保存：没文件名先起名，存好后标题不带 ●；再打开读回来", async () => {
  const app = await start();
  await app.press("lib-coin");
  await app.keyboard.press("mod+s");
  expect(app.shows("file-name")).toBe(true);
  await app.type("file-name", "测试关");
  await app.press("file-save");
  expect(app.shows("file-name")).toBe(false);
  expect(has(app.texts(), "测试关.json")).toBe(true);
  expect(has(app.texts(), "测试关.json ●")).toBe(false);
  // 无界面测试里没有文件权限：退到应用存储
  const saved = Object.entries(app.storage()).find(([k]) => k.endsWith("测试关.json"));
  expect(saved !== undefined && saved[1].includes('"type": "coin"')).toBe(true);
  await app.press("new");
  expect(app.shows("tree-e4")).toBe(false);
  await app.press("open");
  await app.press("file-测试关.json");
  expect(app.shows("tree-e4")).toBe(true);
});

test("有没保存的修改时新建要问：取消就留着", async () => {
  const app = await start();
  const asked: string[] = [];
  app.onDialog((d) => {
    asked.push(d.title ?? "");
    return "取消";
  });
  await app.press("lib-coin");
  await app.press("new");
  expect(asked).toEqual(["有没保存的修改"]);
  expect(app.shows("tree-e4")).toBe(true);
  app.onDialog(() => "不保存");
  await app.press("new");
  expect(app.shows("tree-e4")).toBe(false);
});

test("⌘R 试玩当前关卡：两边的面板还在，改了关卡就地更新、不重开；Esc 回来，改动还在", async () => {
  const app = await start();
  await app.press("lib-coin");
  await app.keyboard.press("mod+r");
  expect(has(app.texts(), "试玩，按 Esc 回到编辑")).toBe(true);
  expect(has(app.texts(), "0 / 1 coin")).toBe(true);
  // 试玩时场景树、属性面板还在：再放一个金币，正在玩的这一关多了一个（状态栏的金币数跟着变）
  expect(app.shows("tree-e1")).toBe(true);
  await app.press("lib-coin");
  expect(has(app.texts(), "0 / 2 coin")).toBe(true);
  // 撤销照常，正在玩的也跟着撤
  await app.keyboard.press("mod+z");
  expect(has(app.texts(), "0 / 1 coin")).toBe(true);
  // 不合规的改动（删掉终点）：试玩里先不更新，状态栏说为什么
  await app.press("tree-e3");
  await app.press("delete");
  expect(has(app.texts(), "试玩里先不更新")).toBe(true);
  expect(has(app.texts(), "试玩，按 Esc 回到编辑")).toBe(true);
  await app.keyboard.press("mod+z");
  await app.press("stop");
  expect(app.shows("stop")).toBe(false);
  expect(app.shows("play")).toBe(true);
  expect(app.shows("tree-e4")).toBe(true);
  expect(has(app.texts(), "未命名 ●")).toBe(true);
  await app.keyboard.press("mod+r");
  await app.keyboard.press("escape");
  expect(app.shows("play")).toBe(true);
});

test("层：加一层、设为当前层、刷子画在当前层；锁上的层画不上；矩形、油漆桶的快捷键", async () => {
  const app = await start();
  await app.press("layer-add");
  expect(app.shows("layer-1")).toBe(true);
  expect(app.value("layer-name")).toBe("第 2 层");
  await app.clear("layer-px");
  await app.type("layer-px", "0.5\n");
  expect(has(app.texts(), "第 2 层 · 视差")).toBe(true);
  await app.press("layer-1-locked");
  await app.press("tile-0");
  expect(app.shows("layer-1-locked")).toBe(true);
  await app.keyboard.press("r");
  await app.keyboard.press("g");
  await app.press("layer-0");
  await app.press("layer-remove");
  expect(app.shows("layer-1")).toBe(false);
  await app.keyboard.press("mod+z");
  expect(app.shows("layer-1")).toBe(true);
});

test("分组：放一个分组和金币，属性面板里填父节点；场景树缩进、能折叠；拖一行到别的行上改父节点", async () => {
  const app = await start();
  await app.press("lib-group");
  await app.press("lib-coin");
  await app.clear("field-parent");
  await app.type("field-parent", "e4\n");
  expect(app.shows("fold-e4")).toBe(true);
  await app.press("fold-e4");
  expect(app.shows("tree-e5")).toBe(false);
  await app.press("fold-e4");
  expect(app.shows("tree-e5")).toBe(true);
  // 父节点不能是自己的子孙
  await app.press("tree-e4");
  await app.clear("field-parent");
  await app.type("field-parent", "e5\n");
  expect(has(app.texts(), "不能放进自己")).toBe(true);
  // 把出生点（第一行）拖到分组那一行上（往下 3 行）
  await app.press("tree-e1");
  await app.drag("grip-e1", { y: 24 * 3 });
  expect(app.value("field-parent")).toBe("e4");
});

test("走地怪：加路线点、删路线点；预制体：加覆盖、删覆盖，写错的说原因", async () => {
  const app = await start();
  await app.press("lib-walker");
  await app.press("point-add");
  await app.press("point-add");
  expect(app.shows("point-1-remove")).toBe(true);
  await app.press("point-0-remove");
  expect(app.shows("point-1-remove")).toBe(false);
  await app.press("lib-prefab");
  await app.type("override-path", "b/crown");
  await app.type("override-field", "y");
  await app.type("override-value", "110");
  await app.press("override-add");
  expect(has(app.texts(), "b/crown / y = 110")).toBe(true);
  await app.press("override-b/crown-y-remove");
  expect(has(app.texts(), "b/crown / y = 110")).toBe(false);
  await app.clear("override-path");
  await app.type("override-path", "b//crown");
  await app.press("override-add");
  expect(app.shows("override-error")).toBe(true);
});

test("不能试玩的关卡不进试玩：删掉终点，状态栏说原因", async () => {
  const app = await start();
  await app.press("tree-e3");
  await app.press("delete");
  await app.press("play");
  expect(app.shows("tree-e1")).toBe(true);
  expect(has(app.texts(), "终点要有 1 个")).toBe(true);
});

test("文件名输入框有焦点时 Esc 也能关掉文件面板", async () => {
  const app = await start();
  await app.press("saveAs");
  await app.type("file-name", "abc");
  await app.keyboard.press("escape");
  expect(app.shows("file-name")).toBe(false);
});
