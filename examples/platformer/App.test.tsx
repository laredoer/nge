// `nsh test`：无头 runner 里没有游戏视图，测按钮的显示和按住的手指怎么换成按键。
import { expect, open, test } from "nsh/test";
import { heldKey } from "../../ts/nge";

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
