// 血条和关卡名气泡的数值（画面在 Rust 那边测，见 crates/nge-scene/src/world_ui.rs）。
import { expect, test } from "nsh/test";
import { HEARTS, hpBar, outOfHearts, padPlace, titleBubble } from "./hud";

test("血条跟着角色，失手一次少一格，最后一格变红", () => {
  expect(hpBar(0).follow).toBe("player");
  expect(hpBar(0).value).toBe(1);
  expect(hpBar(1).value).toBe(2 / 3);
  expect(hpBar(1).color).toBe("#4ade80");
  expect(hpBar(2).value).toBe(1 / 3);
  expect(hpBar(2).color).toBe("#ef4444");
  expect(hpBar(HEARTS + 2).value).toBe(0);
});

test("失手三次整关重来", () => {
  expect(outOfHearts(HEARTS - 1)).toBe(false);
  expect(outOfHearts(HEARTS)).toBe(true);
});

test("关卡名气泡在血条上面，一个字一个字打出来", () => {
  const b = titleBubble("草地");
  expect(b.text).toBe("草地");
  expect(b.follow).toBe("player");
  expect(b.y!).toBeGreaterThan(hpBar(0).y!);
  expect(b.typing!).toBeGreaterThan(0);
});

test("触屏按钮躲开系统栏和刘海：安全区的边距加在原来的边距上", () => {
  expect(padPlace({ top: 0, right: 0, bottom: 0, left: 0 })).toEqual({ bottom: 32, left: 24, right: 24 });
  expect(padPlace({ top: 24, right: 48, bottom: 16, left: 30 })).toEqual({ bottom: 48, left: 54, right: 72 });
});
