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

test("⌘R 试玩当前关卡，Esc 回来，改动还在", async () => {
  const app = await start();
  await app.press("lib-coin");
  await app.keyboard.press("mod+r");
  expect(has(app.texts(), "试玩，按 Esc 回到编辑")).toBe(true);
  expect(app.shows("tree-e1")).toBe(false);
  await app.keyboard.press("escape");
  expect(app.shows("tree-e4")).toBe(true);
  expect(has(app.texts(), "未命名 ●")).toBe(true);
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
