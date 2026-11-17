// 触屏操作和结束画面的组件（docs/touch-input.md）：用假的游戏视图（fixtures/controls.tsx）看发出去的命令。
import { expect, open, test } from "nsh/test";

const log = (app: { texts(): string[] }) => app.texts().find((s) => s.includes("restarts")) ?? "";

test("HUD 每帧变：按着的摇杆不重建，不发松开，方向不变；抬起才松开", async () => {
  const app = await open("./fixtures/controls.tsx", { platform: "ios", locale: "zh-CN" });
  await app.dragAndHold("id:stick-move", { x: 80 });
  expect(log(app)).toBe("move down 1,0;restarts 0");
  for (let i = 0; i < 3; i++) await app.press("id:frame");
  expect(app.count("id:stick-move")).toBe(1);
  expect(log(app)).toBe("move down 1,0;restarts 0");
  await app.drop();
  expect(log(app)).toBe("move down 1,0 | move up;restarts 0");
});

test("攻击按住连发：按下后每 0.18 秒重按一次（第一次也不多等）", async () => {
  // 手动的钟：按住以后过了多久只看 wait
  const app = await open("./fixtures/controls.tsx", { platform: "ios", locale: "zh-CN", clock: {} });
  await app.dragAndHold("id:act-attack", { x: 1 });
  const held = log(app);
  await app.wait(200);
  expect(log(app)).toBe(held.replace(";", " | attack up | attack down;"));
  await app.drop();
});

test("怒气不够时突刺按钮变暗，够了恢复", async () => {
  const app = await open("./fixtures/controls.tsx", { platform: "ios", locale: "zh-CN" });
  expect(app.count("id:act-thrust-dim")).toBe(1);
  await app.press("id:rage");
  expect(app.count("id:act-thrust-dim")).toBe(0);
});

test("结束画面：刚出现时点不算，0.8 秒以后按下的才重开；HUD 每帧变也不重新计时", async () => {
  const app = await open("./fixtures/controls.tsx", { platform: "ios", locale: "zh-CN" });
  await app.press("id:over");
  expect(app.count("id:end")).toBe(1);
  // 结束时摇杆、按钮收起来
  expect(app.count("id:stick-move")).toBe(0);
  await app.press("id:end");
  expect(log(app)).toBe(";restarts 0");
  await app.press("id:later");
  await app.press("id:frame");
  await app.press("id:end");
  expect(log(app)).toBe(";restarts 1");
});

test("结束画面：出现之前就按着的手指抬起不算", async () => {
  const app = await open("./fixtures/controls.tsx", { platform: "ios", locale: "zh-CN" });
  await app.dragAndHold("id:stick-move", { x: 80 });
  await app.press("id:over");
  await app.press("id:later");
  await app.drop();
  expect(log(app).endsWith("restarts 0")).toBe(true);
});

test("结束画面：刚出现时按下、过了 0.8 秒才抬起也不算（按下的时间算数）", async () => {
  const app = await open("./fixtures/controls.tsx", { platform: "ios", locale: "zh-CN" });
  await app.press("id:over");
  await app.dragAndHold("id:end", { x: 1 });
  await app.press("id:later");
  await app.drop();
  expect(log(app)).toBe(";restarts 0");
});
