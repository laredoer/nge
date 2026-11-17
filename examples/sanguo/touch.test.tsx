// 触屏操作（docs/touch-input.md）：摇杆和按钮按动作发，引擎算好的动作事件和键盘的输入合在一起。
import { expect, open, test } from "nsh/test";
import { NO_INPUT, newPlayer, THRUST } from "./player";
import { NO_TOUCH, thrustCooldown, TOUCH_ACTIONS, touchAction, withTouch } from "./touch";

const ev = (action: string, down: boolean, x = 0, y = 0) => ({ action, player: 0, down, x, y });

test("绑定表只有触屏驱动的动作，不绑键（键盘照旧走 onKey）", () => {
  const p = TOUCH_ACTIONS.players[0];
  expect(Object.keys(p).sort()).toEqual(["attack", "jump", "move", "thrust"]);
  expect(p.move.kind).toBe("axis2d");
  for (const b of Object.values(p)) for (const keys of Object.values(b).filter(Array.isArray)) expect(keys).toEqual([]);
});

test("摇杆的方向当成方向键，松开归零；别人的、别的玩家的不管", () => {
  let s = touchAction(NO_TOUCH, ev("move", true, 1, -1), false);
  expect(s.move).toEqual([1, -1]);
  expect(withTouch(NO_INPUT, s)).toMatchObject({ x: 1, y: -1 });
  s = touchAction(s, ev("move", false), false);
  expect(s.move).toEqual([0, 0]);
  // 松开就是归零，不看事件里带的方向
  expect(touchAction({ ...NO_TOUCH, move: [1, 0] }, ev("move", false, 1, 0), false).move).toEqual([0, 0]);
  expect(touchAction(NO_TOUCH, { ...ev("move", true, 1, 0), player: 1 }, false)).toEqual(NO_TOUCH);
  expect(touchAction(NO_TOUCH, ev("pause", true), false)).toEqual(NO_TOUCH);
});

test("攻击、突刺、跳按下的那一下算一次（连发的每一下都算），松开不算，暂停时不算", () => {
  let s = touchAction(NO_TOUCH, ev("attack", true), false);
  s = touchAction(s, ev("attack", false), false);
  s = touchAction(s, ev("attack", true), false);
  s = touchAction(s, ev("thrust", true), false);
  expect(s.pressed).toEqual(["attack", "attack", "thrust"]);
  expect(withTouch(NO_INPUT, s)).toMatchObject({ attack: true, thrust: true, jump: false });
  expect(touchAction(NO_TOUCH, ev("jump", true), true)).toEqual(NO_TOUCH);
});

test("和键盘合在一起：键盘按着方向时以键盘为准，按键和触屏有一个按下就算", () => {
  const touch = { move: [-1, 1] as [number, number], pressed: ["jump" as const] };
  expect(withTouch({ ...NO_INPUT, x: 1, attack: true }, touch)).toEqual({ x: 1, y: 1, attack: true, thrust: false, jump: true });
  expect(withTouch(NO_INPUT, NO_TOUCH)).toEqual(NO_INPUT);
});

test("突刺按钮的冷却：剩余冷却占总冷却的比例", () => {
  expect(thrustCooldown({ ...newPlayer(0, 0), cooldown: THRUST.cooldown })).toBe(1);
  expect(thrustCooldown({ ...newPlayer(0, 0), cooldown: THRUST.cooldown / 2 })).toBe(0.5);
  expect(thrustCooldown(newPlayer(0, 0))).toBe(0);
});

test("触屏设备上有摇杆和三个按钮，Mac 上没有", async () => {
  const ios = await open(".", { platform: "ios", locale: "zh-CN" });
  for (const id of ["id:stick-move", "id:act-attack", "id:act-thrust", "id:act-jump"]) expect(ios.count(id)).toBe(1);
  const mac = await open(".", { platform: "macos", locale: "zh-CN" });
  for (const id of ["id:stick-move", "id:act-attack", "id:act-thrust", "id:act-jump"]) expect(mac.count(id)).toBe(0);
});
