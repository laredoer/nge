// 固定步长：画面画出来（引擎 ready）之前、暂停时不推进；卡了一下也只补有限几步。
import { expect, test } from "nsh/test";
import { DT } from "./player";
import { MAX_STEPS, pace } from "./pace";

test("引擎没画出第一帧之前不推进，也不攒时间", () => {
  expect(pace(0, 2, { ready: false, paused: false })).toEqual({ acc: 0, steps: 0 });
  expect(pace(DT * 3, 2, { ready: false, paused: false })).toEqual({ acc: DT * 3, steps: 0 });
});

test("暂停时不推进", () => {
  expect(pace(0, 1, { ready: true, paused: true }).steps).toBe(0);
});

test("跑起来按经过的时间补步数，余下的留到下一帧", () => {
  const r = pace(0, DT * 2.5, { ready: true, paused: false });
  expect(r.steps).toBe(2);
  expect(Math.abs(r.acc - DT * 0.5) < 1e-9).toBe(true);
});

test("卡了很久也只补 MAX_STEPS 步", () => {
  expect(pace(0, 10, { ready: true, paused: false }).steps).toBe(MAX_STEPS);
});
