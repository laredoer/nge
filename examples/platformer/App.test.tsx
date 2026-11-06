// `nsh test`：无头 runner 里没有游戏视图，测按钮的显示和按住的手指怎么换成按键。
import { expect, open, test } from "nsh/test";
import { heldKey } from "../../ts/nge";
import { finish, LEVELS, NEW_PROGRESS, progressOf } from "./levels";

const BUTTONS = ["id:key-ArrowLeft", "id:key-ArrowRight", "id:key-Space", "id:key-KeyR"];

test("手机上有触屏按钮", async () => {
  const app = await open(".", { platform: "ios" });
  for (const button of BUTTONS) expect(app.count(button)).toBe(1);
});

test("Mac 上没有触屏按钮", async () => {
  const app = await open(".", { platform: "macos" });
  for (const button of BUTTONS) expect(app.count(button)).toBe(0);
});

test("第一根手指按下、最后一根松开时才发", () => {
  const sent: boolean[] = [];
  const held = heldKey((down) => sent.push(down));
  held.start([1]);
  held.start([2]);
  held.end([1]);
  expect(sent).toEqual([true]);
  held.end([2]);
  expect(sent).toEqual([true, false]);
});

test("没按着时的松开不发，同一根手指重复按下只算一次", () => {
  const sent: boolean[] = [];
  const held = heldKey((down) => sent.push(down));
  held.end([7]);
  expect(sent).toEqual([]);
  held.start([3]);
  held.start([3]);
  held.end([3]);
  expect(sent).toEqual([true, false]);
  held.end([3]);
  expect(sent).toEqual([true, false]);
});

test("别的手指松开不影响按着的", () => {
  const sent: boolean[] = [];
  const held = heldKey((down) => sent.push(down));
  held.start([1]);
  held.end([9]);
  expect(sent).toEqual([true]);
});

const hud = (app: { texts(): string[] }) => app.texts().join(" ");

test("存档里的最好成绩显示在 HUD 上", async () => {
  const app = await open(".", { platform: "macos", storage: { "nge:platformer": JSON.stringify({ unlocked: 2, best: { "1": 12.3 } }) } });
  await app.waitUntil(() => hud(app).includes("最好 12.3 秒"), { timeout: 1000 });
});

test("没有存档、存档坏了、结构不对都从头开始", async () => {
  const cases: [Record<string, string>, number][] = [[{}, 1], [{ "nge:platformer": "{坏的" }, 1], [{ "nge:platformer": "null" }, 1], [{ "nge:platformer": '{"unlocked":2}' }, 2]];
  for (const [storage, unlocked] of cases) {
    const app = await open(".", { platform: "macos", storage });
    // 等读档回来再看
    await app.wait(100);
    expect(hud(app)).toContain(`解锁了 ${unlocked} 关`);
    expect(hud(app)).toContain("第 1 关 草原");
    expect(hud(app)).not.toContain("最好");
    expect(app.storage()).toEqual(storage);
  }
});

test("通关解锁下一关，用时更短才记", () => {
  const first = finish(NEW_PROGRESS, 0, 30);
  expect(first).toEqual({ unlocked: 2, best: { "1": 30 } });
  expect(finish(first, 0, 40)).toEqual(first);
  expect(finish(first, 0, 25)).toEqual({ unlocked: 2, best: { "1": 25 } });
  // 最后一关通关不会解锁不存在的关；重玩前面的关不会把解锁的往回退
  const last = finish(first, LEVELS.length - 1, 50);
  expect(last.unlocked).toBe(LEVELS.length);
  expect(finish(last, 0, 99).unlocked).toBe(LEVELS.length);
  expect(NEW_PROGRESS).toEqual({ unlocked: 1, best: {} });
});

test("存档补齐：缺的按新存档，越界的收回来", () => {
  expect(progressOf(null)).toEqual(NEW_PROGRESS);
  expect(progressOf("x")).toEqual(NEW_PROGRESS);
  expect(progressOf({ unlocked: 2 })).toEqual({ unlocked: 2, best: {} });
  expect(progressOf({ unlocked: 99, best: { "1": 20, "2": "快" } })).toEqual({ unlocked: LEVELS.length, best: { "1": 20 } });
  expect(progressOf({ unlocked: 0.5, best: null })).toEqual(NEW_PROGRESS);
});
