// 虚拟摇杆的纯逻辑（docs/touch-input.md）：偏移换成摇杆值、量化方向、跟踪一根手指、决定发什么。
export type Vec = [number, number];

export interface StickShape {
  /** 推到底的距离（point）。 */
  radius: number;
  /** 死区，占半径的比例（0 … 1）。 */
  deadZone: number;
}

export const STICK_DEFAULTS: StickShape = { radius: 60, deadZone: 0.2 };

/** `analog` 原样；`8` 八方向、`4` 四方向，分量是 −1 / 0 / 1。 */
export type Directions = "analog" | "8" | "4";

/**
 * 手指相对中心的偏移（屏幕坐标，y 朝下）换成摇杆值（y 朝上，长度 0 … 1）。
 * 死区里是 0；外面按 (长度 − 死区) / (半径 − 死区) 重新铺满，超过半径的按 1。
 */
export function stickValue(dx: number, dy: number, shape: StickShape = STICK_DEFAULTS): Vec {
  const len = Math.hypot(dx, dy);
  const dead = Math.max(0, Math.min(1, shape.deadZone)) * shape.radius;
  const span = shape.radius - dead;
  if (!(len > dead) || !(span > 0)) return [0, 0];
  const k = Math.min(1, (len - dead) / span) / len;
  return [dx * k, -dy * k];
}

/** 八方向按 45° 一格取最近的方向，四方向取绝对值大的轴。 */
export function snapDirection([x, y]: Vec, directions: "8" | "4"): Vec {
  if (x === 0 && y === 0) return [0, 0];
  if (directions === "4") return Math.abs(x) >= Math.abs(y) ? [Math.sign(x), 0] : [0, Math.sign(y)];
  const octant = Math.round(Math.atan2(y, x) / (Math.PI / 4));
  const angle = (octant * Math.PI) / 4;
  return [Math.round(Math.cos(angle)), Math.round(Math.sin(angle))].map((v) => v + 0) as Vec;
}

/** 摇杆头画在哪：偏移限在半径以内（屏幕坐标）。 */
export function knobOffset(dx: number, dy: number, radius: number): Vec {
  const len = Math.hypot(dx, dy);
  if (len <= radius) return [dx, dy];
  return [(dx / len) * radius, (dy / len) * radius];
}

/** 接管摇杆的那根手指：`center` 是摇杆中心，`at` 是手指现在的位置（区域里的坐标）。 */
export interface StickFinger {
  id: number;
  center: Vec;
  at: Vec;
}

export type StickState = StickFinger | null;

export interface StickTouch {
  phase: "start" | "move" | "end" | "cancel";
  id: number;
  x: number;
  y: number;
}

/**
 * 一次触摸变化之后的状态。没手指时第一根按下的接管（浮动模式中心是按下的点，固定模式是 `center`），
 * 别的手指不管；接管的那根抬起或被取消时放开；同一个 id 又按下（丢了 end）当新的一次。
 */
export function stickStep(state: StickState, touch: StickTouch, place: { mode: "float" | "fixed"; center: Vec }): StickState {
  const at: Vec = [touch.x, touch.y];
  const mine = state !== null && state.id === touch.id;
  if (touch.phase === "start") {
    if (state !== null && !mine) return state;
    return { id: touch.id, center: place.mode === "float" ? at : place.center, at };
  }
  if (!mine) return state;
  return touch.phase === "move" ? { ...state, at } : null;
}

/** 现在的摇杆值（没手指是 [0, 0]），按方向模式量化。 */
export function stickOutput(state: StickState, shape: StickShape, directions: Directions): Vec {
  if (state === null) return [0, 0];
  const v = stickValue(state.at[0] - state.center[0], state.at[1] - state.center[1], shape);
  return directions === "analog" ? v : snapDirection(v, directions);
}

const round2 = (v: number) => Math.round(v * 100) / 100 + 0;

/** 摇杆值从 `prev` 变成 `next` 时发什么（`act` 的 down / x / y）；保留两位小数后没变是 null，0 是松开。 */
export function stickCommand(prev: Vec, next: Vec): { down: boolean; x?: number; y?: number } | null {
  const [px, py] = [round2(prev[0]), round2(prev[1])];
  const [x, y] = [round2(next[0]), round2(next[1])];
  if (x === px && y === py) return null;
  return x === 0 && y === 0 ? { down: false } : { down: true, x, y };
}
