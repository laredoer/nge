// 触屏操作（docs/touch-input.md）：摇杆和按钮按动作发给引擎，引擎发回的动作事件在这里和键盘的输入合在一起。
import type { ActionEvent, ActionMap } from "../../ts/nge";
import { THRUST, type Input, type Player } from "./player";

type Press = "attack" | "thrust" | "jump";

/** 只给触屏用的绑定表：动作都不绑键，键盘和手柄照旧走 `onKey`。 */
export const TOUCH_ACTIONS: ActionMap = {
  players: [
    {
      move: { kind: "axis2d", left: [], right: [], down: [], up: [] },
      attack: { kind: "button", keys: [] },
      thrust: { kind: "button", keys: [] },
      jump: { kind: "button", keys: [] },
    },
  ],
};

/** 攻击按住时每隔多久再按一次（秒）：连段靠它。 */
export const ATTACK_REPEAT = 0.18;

export interface TouchState {
  /** 摇杆的方向（−1 / 0 / 1，y 朝上是往里）。 */
  move: [number, number];
  /** 这一步之前按下过的按钮（连发的每一下各算一次）。 */
  pressed: Press[];
}

export const NO_TOUCH: TouchState = { move: [0, 0], pressed: [] };

const PRESSES: readonly string[] = ["attack", "thrust", "jump"];

/** 收到一个动作事件：第一个玩家的 `move` 记下方向，按钮按下记一次（暂停时不记）。 */
export function touchAction(s: TouchState, e: ActionEvent, paused: boolean): TouchState {
  if (e.player !== 0) return s;
  if (e.action === "move") return { ...s, move: e.down ? [e.x, e.y] : [0, 0] };
  if (PRESSES.includes(e.action) && e.down && !paused) return { ...s, pressed: [...s.pressed, e.action as Press] };
  return s;
}

/** 键盘的输入加上触屏的：键盘按着方向时以键盘为准，按键和触屏有一个按下就算。 */
export function withTouch(input: Input, s: TouchState): Input {
  const has = (p: Press) => s.pressed.includes(p);
  return {
    ...input,
    x: input.x !== 0 ? input.x : s.move[0],
    y: input.y !== 0 ? input.y : s.move[1],
    attack: input.attack || has("attack"),
    thrust: input.thrust || has("thrust"),
    jump: input.jump || has("jump"),
  };
}

/** 突刺按钮上盖的冷却：剩余冷却占总冷却的比例。 */
export function thrustCooldown(p: Player): number {
  return p.cooldown / THRUST.cooldown;
}
