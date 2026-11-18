// 触屏组件接到假的游戏视图上（fixtures/touch.tsx）：摇杆、按钮发出去的命令，按下的样子，冷却，卸载和切后台时松开（docs/touch-input.md）。
import { expect, open, test } from "nsh/test";

const log = (app: { texts(): string[] }) => (app.texts().find((s) => s.startsWith("frame ")) ?? "").replace(/^frame \d+: /, "");
const fixture = () => open("./fixtures/touch.tsx", { platform: "ios" });
/** 推着摇杆：模拟量一路往上发，最后推到底，中间没有松开。 */
const pushed = (s: string) => s.endsWith("move down 1,0 p1 pad") && !s.includes("up");

test("摇杆：按住推过去发按下（来源、玩家、方向），父组件的信号变了不受影响，抬起发松开", async () => {
  const app = await fixture();
  await app.dragAndHold("id:stick-move", { x: 80 });
  const held = log(app);
  expect(pushed(held)).toBe(true);
  await app.press("id:frame");
  await app.press("id:frame");
  expect(log(app)).toBe(held);
  await app.drop();
  expect(log(app)).toBe(`${held} | move up p1 pad`);
});

test("摇杆：往下推过一半同时按着「下」，回来或抬起松开", async () => {
  const app = await fixture();
  await app.dragAndHold("id:stick-move", { x: 0, y: 80 });
  const held = log(app);
  expect(held).toContain("crouch down p1 pad:down");
  expect(held.split(" | ").filter((s) => s.startsWith("crouch")).length).toBe(1);
  await app.drop();
  expect(log(app).endsWith("crouch up p1 pad:down | move up p1 pad")).toBe(true);
  // 横着推不按「下」
  const app2 = await fixture();
  await app2.dragAndHold("id:stick-move", { x: 80 });
  expect(log(app2)).not.toContain("crouch");
  await app2.drop();
});

test("摇杆：按着时卸载发松开", async () => {
  const app = await fixture();
  await app.dragAndHold("id:stick-move", { x: 80 });
  const held = log(app);
  await app.press("id:unmount");
  expect(app.count("id:stick-move")).toBe(0);
  expect(log(app)).toBe(`${held} | move up p1 pad`);
  await app.drop();
});

test("摇杆、按钮：按着时 App 切到后台就松开", async () => {
  const app = await fixture();
  await app.dragAndHold("id:stick-move", { x: 80 });
  const held = `${log(app)} | move up p1 pad`;
  await app.setLifecycle("background");
  expect(log(app)).toBe(held);
  await app.setLifecycle("active");
  await app.drop();
  expect(log(app)).toBe(held);
  await app.dragAndHold("id:act-fire", { x: 1 });
  await app.setLifecycle("inactive");
  expect(log(app).endsWith("fire down | fire up")).toBe(true);
  await app.setLifecycle("active");
  await app.drop();
});

test("按钮：按着时样子变了（字变黄），松开变回来", async () => {
  const app = await fixture();
  const before = app.spanStyle("text:火").color;
  await app.dragAndHold("id:act-fire", { x: 1 });
  expect(app.spanStyle("text:火").color === before).toBe(false);
  await app.drop();
  expect(app.spanStyle("text:火").color).toBe(before);
});

test("按钮：冷却大于 0 时盖一层，回到 0 拿掉", async () => {
  const app = await fixture();
  expect(app.count("id:act-fire-cooldown")).toBe(0);
  await app.press("id:cool");
  expect(app.count("id:act-fire-cooldown")).toBe(1);
  await app.press("id:frame");
  expect(app.count("id:act-fire-cooldown")).toBe(1);
  await app.press("id:warm");
  expect(app.count("id:act-fire-cooldown")).toBe(0);
});

test("按钮：按住连发接到 act 上，卸载时停掉并松开", async () => {
  const app = await fixture();
  await app.dragAndHold("id:act-fire", { x: 1 });
  await app.wait(250);
  expect(log(app).startsWith("fire down | fire up | fire down")).toBe(true);
  await app.press("id:unmount");
  const after = log(app);
  expect(after.endsWith("fire down | fire up")).toBe(true);
  await app.wait(300);
  expect(log(app)).toBe(after);
  await app.drop();
});

test("手势：卸载时关掉长按计时，之后不再发长按", async () => {
  const app = await fixture();
  await app.dragAndHold("id:gestures", { x: 1 });
  await app.press("id:unmount");
  await app.wait(700);
  expect(log(app).includes("longPress")).toBe(false);
  await app.drop();
});

test("手势：拖着时 App 切到后台，拖动以取消结束", async () => {
  const app = await fixture();
  await app.dragAndHold("id:gestures", { x: 40 });
  expect(log(app).includes("dragStart")).toBe(true);
  await app.setLifecycle("background");
  expect(log(app).endsWith("dragEnd cancelled")).toBe(true);
  await app.setLifecycle("active");
  await app.drop();
  expect(log(app).endsWith("dragEnd cancelled")).toBe(true);
});
