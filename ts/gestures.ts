// 手势识别（docs/touch-input.md）：纯函数状态机，输入触摸点序列，输出轻点、长按、拖动、滑动、双指捏合和旋转。
import type { GestureResponderEvent } from "nsh";

export interface GestureOptions {
  /** 挪出多远（point）就不算点了。 */
  slop: number;
  /** 按住多久（毫秒）算长按。 */
  longPress: number;
  /** 滑动至少多远（point）。 */
  swipeDistance: number;
  /** 滑动抬起时至少多快（point / 秒）。 */
  swipeSpeed: number;
}

export const GESTURE_DEFAULTS: GestureOptions = { slop: 10, longPress: 500, swipeDistance: 30, swipeSpeed: 300 };

/** 一根手指的变化（`t` 是事件的毫秒时间戳，只拿来算差），或者长按的计时到了。 */
export type GestureInput = { phase: "start" | "move" | "end" | "cancel"; id: number; x: number; y: number; t: number } | { phase: "hold"; id: number };

export type SwipeDirection = "left" | "right" | "up" | "down";

/** 坐标都是视图里的（左上角原点，y 朝下）；角度是度，屏幕上顺时针为正。 */
export type Gesture =
  | { kind: "tap"; x: number; y: number }
  | { kind: "longPress"; x: number; y: number }
  | { kind: "dragStart" | "drag"; x: number; y: number; dx: number; dy: number; tx: number; ty: number }
  | { kind: "dragEnd"; x: number; y: number; dx: number; dy: number; tx: number; ty: number; vx: number; vy: number; cancelled: boolean }
  | { kind: "swipe"; direction: SwipeDirection; vx: number; vy: number; speed: number; tx: number; ty: number }
  | { kind: "pinchStart"; x: number; y: number; scale: number; rotation: number }
  | { kind: "pinch"; x: number; y: number; scale: number; rotation: number; factor: number; turn: number; dx: number; dy: number }
  | { kind: "pinchEnd"; x: number; y: number; scale: number; rotation: number; cancelled: boolean };

interface Sample {
  x: number;
  y: number;
  t: number;
}

interface Finger {
  id: number;
  start: Sample;
  at: Sample;
  /** 最近 100 ms 的位置，算速度用。 */
  recent: Sample[];
}

interface Pinch {
  /** 开始时两指的距离。 */
  d0: number;
  dist: number;
  angle: number;
  center: [number, number];
  rotation: number;
}

/**
 * `press` 一根手指按着没挪；`held` 长按过了；`drag` 一根手指在拖；`pinch` 两根手指；
 * `done` 这一轮的手势结束了（取消、捏合时抬起了一根），剩下的手指都抬起来之前什么都不算。
 */
export interface GestureState {
  mode: "idle" | "press" | "held" | "drag" | "pinch" | "done";
  /** 按着的手指，按按下的顺序；前两根是捏合的那两根。 */
  fingers: Finger[];
  pinch: Pinch | null;
}

export const GESTURE_IDLE: GestureState = { mode: "idle", fingers: [], pinch: null };

const VELOCITY_WINDOW_MS = 100;

type Out = { state: GestureState; events: Gesture[] };

export function gestureStep(state: GestureState, input: GestureInput, options: GestureOptions = GESTURE_DEFAULTS): Out {
  if (input.phase === "hold") return hold(state, input.id);
  const at: Sample = { x: input.x, y: input.y, t: input.t };
  if (input.phase === "start") {
    // 丢了 end 的 id 又按下：先当它被取消了，补上上一轮的结束事件
    const known = state.fingers.some((f) => f.id === input.id);
    const before = known ? lift(state, input.id, at, true, options) : { state, events: [] };
    const after = press(before.state, { id: input.id, start: at, at, recent: [at] });
    return { state: after.state, events: [...before.events, ...after.events] };
  }
  const finger = state.fingers.find((f) => f.id === input.id);
  if (finger === undefined) return { state, events: [] };
  if (input.phase === "move") return moved(state, finger, at, options);
  return lift(state, input.id, at, input.phase === "cancel", options);
}

function press(state: GestureState, finger: Finger): Out {
  const fingers = [...state.fingers, finger];
  if (state.mode === "idle") return { state: { mode: "press", fingers, pinch: null }, events: [] };
  if (state.mode === "press" || state.mode === "held" || state.mode === "drag") {
    const [a, b] = fingers;
    const pinch: Pinch = { d0: distance(a.at, b.at), dist: distance(a.at, b.at), angle: angleOf(a.at, b.at), center: middle(a.at, b.at), rotation: 0 };
    const events: Gesture[] = state.mode === "drag" ? [dragEnd(state.fingers[0], true)] : [];
    events.push({ kind: "pinchStart", x: pinch.center[0], y: pinch.center[1], scale: 1, rotation: 0 });
    return { state: { mode: "pinch", fingers, pinch }, events };
  }
  // 捏合中的第三根以后、结束了的这一轮：只记着，等它抬起
  return { state: { ...state, fingers }, events: [] };
}

function moved(state: GestureState, finger: Finger, at: Sample, options: GestureOptions): Out {
  const recent = [...finger.recent.filter((s) => s.t >= at.t - VELOCITY_WINDOW_MS), at];
  const next: Finger = { ...finger, at, recent };
  const fingers = state.fingers.map((f) => (f.id === finger.id ? next : f));
  const primary = state.fingers[0].id === finger.id;
  const tx = at.x - finger.start.x;
  const ty = at.y - finger.start.y;
  if ((state.mode === "press" || state.mode === "held") && primary) {
    if (Math.hypot(tx, ty) <= options.slop) return { state: { ...state, fingers }, events: [] };
    return { state: { ...state, mode: "drag", fingers }, events: [{ kind: "dragStart", x: at.x, y: at.y, dx: tx, dy: ty, tx, ty }] };
  }
  if (state.mode === "drag" && primary) {
    return { state: { ...state, fingers }, events: [{ kind: "drag", x: at.x, y: at.y, dx: at.x - finger.at.x, dy: at.y - finger.at.y, tx, ty }] };
  }
  const pair = fingers.slice(0, 2);
  if (state.mode === "pinch" && state.pinch !== null && pair.some((f) => f.id === finger.id)) {
    const [a, b] = pair;
    const dist = distance(a.at, b.at);
    const angle = angleOf(a.at, b.at);
    const center = middle(a.at, b.at);
    const p = state.pinch;
    const turn = wrapDegrees(angle - p.angle);
    const pinch: Pinch = { ...p, dist, angle, center, rotation: p.rotation + turn };
    const event: Gesture = {
      kind: "pinch",
      x: center[0],
      y: center[1],
      scale: ratio(dist, p.d0),
      rotation: pinch.rotation,
      factor: ratio(dist, p.dist),
      turn,
      dx: center[0] - p.center[0],
      dy: center[1] - p.center[1],
    };
    return { state: { ...state, fingers, pinch }, events: [event] };
  }
  return { state: { ...state, fingers }, events: [] };
}

function lift(state: GestureState, id: number, at: Sample, cancelled: boolean, options: GestureOptions): Out {
  const index = state.fingers.findIndex((f) => f.id === id);
  const finger = { ...state.fingers[index], at };
  const fingers = state.fingers.filter((f) => f.id !== id);
  const after = (mode: GestureState["mode"]): GestureState => (fingers.length === 0 ? GESTURE_IDLE : { mode, fingers, pinch: null });
  const events: Gesture[] = [];
  if (state.mode === "press" && !cancelled) {
    const late = at.t - finger.start.t >= options.longPress;
    events.push({ kind: late ? "longPress" : "tap", x: finger.start.x, y: finger.start.y });
  } else if (state.mode === "drag") {
    const end = dragEnd({ ...finger, recent: [...finger.recent.filter((s) => s.t >= at.t - VELOCITY_WINDOW_MS), at] }, cancelled);
    events.push(end);
    if (!cancelled && end.kind === "dragEnd") {
      const speed = Math.hypot(end.vx, end.vy);
      if (Math.hypot(end.tx, end.ty) >= options.swipeDistance && speed >= options.swipeSpeed) {
        events.push({ kind: "swipe", direction: directionOf(end.tx, end.ty), vx: end.vx, vy: end.vy, speed, tx: end.tx, ty: end.ty });
      }
    }
  } else if (state.mode === "pinch" && index < 2 && state.pinch !== null) {
    const p = state.pinch;
    events.push({ kind: "pinchEnd", x: p.center[0], y: p.center[1], scale: ratio(p.dist, p.d0), rotation: p.rotation, cancelled });
    return { state: after("done"), events };
  } else if (state.mode === "pinch") {
    return { state: { ...state, fingers }, events };
  }
  return { state: after("done"), events };
}

function hold(state: GestureState, id: number): Out {
  if (state.mode !== "press" || state.fingers[0]?.id !== id) return { state, events: [] };
  // 和抬起时补发的一样，用按下的位置
  const { start } = state.fingers[0];
  return { state: { ...state, mode: "held" }, events: [{ kind: "longPress", x: start.x, y: start.y }] };
}

function dragEnd(finger: Finger, cancelled: boolean): Gesture {
  const { at, start, recent } = finger;
  const first = recent[0] ?? at;
  const dt = (at.t - first.t) / 1000;
  const [vx, vy] = dt > 0 ? [(at.x - first.x) / dt, (at.y - first.y) / dt] : [0, 0];
  const prev = recent.length >= 2 ? recent[recent.length - 2] : at;
  return { kind: "dragEnd", x: at.x, y: at.y, dx: at.x - prev.x, dy: at.y - prev.y, tx: at.x - start.x, ty: at.y - start.y, vx, vy, cancelled };
}

function directionOf(tx: number, ty: number): SwipeDirection {
  if (Math.abs(tx) >= Math.abs(ty)) return tx < 0 ? "left" : "right";
  return ty < 0 ? "up" : "down";
}

const distance = (a: Sample, b: Sample) => Math.hypot(b.x - a.x, b.y - a.y);
const angleOf = (a: Sample, b: Sample) => (Math.atan2(b.y - a.y, b.x - a.x) * 180) / Math.PI;
const middle = (a: Sample, b: Sample): [number, number] => [(a.x + b.x) / 2, (a.y + b.y) / 2];
/** 两指距离小于这个（point）按这个算：叠在一起不除零，每步的 factor 乘起来正好是 scale。 */
const MIN_PINCH_DISTANCE = 10;
/** 两个距离的比。 */
const ratio = (now: number, before: number) => Math.max(now, MIN_PINCH_DISTANCE) / Math.max(before, MIN_PINCH_DISTANCE);

/** 角度差换到 (−180, 180]。 */
function wrapDegrees(d: number): number {
  const w = ((d % 360) + 360) % 360;
  return w > 180 ? w - 360 : w;
}

export interface GestureTimers {
  set(fn: () => void, ms: number): number;
  clear(handle: number): void;
}

const SYSTEM_TIMERS: GestureTimers = { set: (fn, ms) => setTimeout(fn, ms), clear: (h) => clearTimeout(h) };

/**
 * 接到视图上：返回 `onTouchStart` / `onTouchMove` / `onTouchEnd` / `onTouchCancel`。坐标用 `locationX/Y`（相对按下时的视图），
 * 所以放在子视图不收触摸的视图上（游戏视图正好）。按下时开一个长按的计时器。
 * 按下、挪动时跟着的手指不在 `touches` 里（end 丢了）当取消；`reset()` 取消所有手指、关掉计时器（切后台、视图卸载时调，见 `touchGestures`）。
 */
export function gestureHandlers(onGesture: (g: Gesture) => void, options: Partial<GestureOptions> = {}, timers: GestureTimers = SYSTEM_TIMERS) {
  const opts = { ...GESTURE_DEFAULTS, ...options };
  let state = GESTURE_IDLE;
  const holds = new Map<number, number>();
  const feed = (input: GestureInput) => {
    const r = gestureStep(state, input, opts);
    state = r.state;
    for (const g of r.events) onGesture(g);
  };
  const stopHold = (id: number) => {
    const h = holds.get(id);
    if (h !== undefined) timers.clear(h);
    holds.delete(id);
  };
  const cancelAll = (keep: (id: number) => boolean) => {
    for (const f of state.fingers) {
      if (keep(f.id)) continue;
      stopHold(f.id);
      feed({ phase: "cancel", id: f.id, x: f.at.x, y: f.at.y, t: f.at.t });
    }
  };
  const each = (phase: "start" | "move" | "end" | "cancel") => (e: GestureResponderEvent) => {
    const touching = (e.touches ?? []).map((t) => t.identifier);
    const changed = e.changedTouches.map((t) => t.identifier);
    if ((phase === "start" || phase === "move") && touching.length > 0) cancelAll((id) => touching.includes(id) || changed.includes(id));
    for (const t of e.changedTouches) {
      const id = t.identifier;
      if (phase === "end" || phase === "cancel" || phase === "start") stopHold(id);
      feed({ phase, id, x: t.locationX, y: t.locationY, t: t.timestamp });
      if (phase === "start") holds.set(id, timers.set(() => (holds.delete(id), feed({ phase: "hold", id })), opts.longPress));
    }
  };
  const reset = () => {
    cancelAll(() => false);
    for (const h of holds.values()) timers.clear(h);
    holds.clear();
    state = GESTURE_IDLE;
  };
  return { onTouchStart: each("start"), onTouchMove: each("move"), onTouchEnd: each("end"), onTouchCancel: each("cancel"), reset };
}
