// 触屏控制（docs/touch-input.md、docs/touch-controls.md）：按动作发的屏幕按钮（连发、冷却、按下反馈）和虚拟摇杆。
import { cleanup, signal, Text, untracked, View, type GestureResponderEvent, type NativeComponentRef, type ViewProps } from "nsh";
import type { GestureTimers } from "./gestures";
import { knobOffset, STICK_DEFAULTS, stickCommand, stickOutput, stickStep, type Directions, type StickState, type Vec } from "./stick";

/**
 * 直接让一个动作按下 / 松开（触屏按钮、虚拟摇杆，见 docs/input-actions.md）：不经过键和绑定表，改键管不到它，也不参与冲突检查。
 * `x`、`y` 是方向（按钮不用填）；同一个动作的几个来源（`source`，比如左、右两个按钮）各自按下松开，方向加起来。
 */
export function act(game: NativeComponentRef | undefined, action: string, down: boolean, options: { player?: number; x?: number; y?: number; source?: string } = {}) {
  game?.command("action", { action, down, ...options });
}

/** 方向的名字（"left"、"up"、"left-down"…）；没有方向是 ""。 */
function directionName(x = 0, y = 0): string {
  return [x < 0 ? "left" : x > 0 ? "right" : "", y > 0 ? "up" : y < 0 ? "down" : ""].filter((s) => s !== "").join("-");
}

/** `<ActionButton>` 的 testID：`act-<动作>`，带方向的是 `act-<动作>-<方向>`。 */
export function actionButtonId(action: string, x?: number, y?: number): string {
  const direction = directionName(x, y);
  return direction === "" ? `act-${action}` : `act-${action}-${direction}`;
}

/** 一个屏幕按钮上按着的手指：第一根按下时 `send(true)`，最后一根松开时 `send(false)`；没按着时的松开不发。 */
export function heldKey(send: (down: boolean) => void) {
  const fingers = new Set<number>();
  return {
    start(ids: number[]) {
      const was = fingers.size;
      for (const id of ids) fingers.add(id);
      if (was === 0 && fingers.size > 0) send(true);
    },
    end(ids: number[]) {
      const was = fingers.size;
      for (const id of ids) fingers.delete(id);
      if (was > 0 && fingers.size === 0) send(false);
    },
  };
}

const SYSTEM_TIMERS: GestureTimers = { set: (fn, ms) => setTimeout(fn, ms), clear: (h) => clearTimeout(h) };

/**
 * 按住连发：`start` 立刻 `send(true)`，等 `delay` 秒后每隔 `interval` 秒重按一次（`send(false)` 再 `send(true)`）；
 * `stop` 停掉并 `send(false)`。没有 `interval` 时就是普通的按下、松开。
 */
export function repeater(send: (down: boolean) => void, { delay = 0.4, interval }: { delay?: number; interval?: number }, timers: GestureTimers = SYSTEM_TIMERS) {
  let handle: number | undefined;
  const tick = () => {
    send(false);
    send(true);
    handle = timers.set(tick, interval! * 1000);
  };
  return {
    start() {
      send(true);
      if (interval !== undefined && interval > 0) handle = timers.set(tick, delay * 1000);
    },
    stop() {
      if (handle !== undefined) timers.clear(handle);
      handle = undefined;
      send(false);
    },
  };
}

/** 冷却盖住按钮的比例：截断在 0 … 1，不是数当 0。 */
export function cooldownFill(v: number): number {
  return Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0;
}

/** 按着时按钮的样子：亮一点、缩小一点。 */
export function pressedLook(pressed: boolean): string {
  return pressed ? "bg-white/45 scale-90" : "bg-white/20";
}

/**
 * 按住就让一个动作按着的屏幕按钮。按钮（跳）只填 `action`；方向键（往左走）再填 `x` / `y`，同一个动作的几个方向键各算一个来源。
 * `repeat`（秒）：按住时每隔这么久再按一次（先等 `repeatDelay`，默认 0.4 秒）。`cooldown`：0 … 1，按钮上盖一层从下往上退的暗色（1 刚用过）。
 * 用触摸而不是 press：同时按住两个按钮（跑着跳）要靠各自的手指。
 */
export function ActionButton(props: {
  game: () => NativeComponentRef | undefined;
  action: string;
  label: string;
  x?: number;
  y?: number;
  player?: number;
  repeat?: number;
  repeatDelay?: number;
  cooldown?: () => number;
  class?: string;
}) {
  const direction = directionName(props.x, props.y);
  const options = { ...(props.player === undefined ? {} : { player: props.player }), ...(direction === "" ? {} : { x: props.x ?? 0, y: props.y ?? 0, source: direction }) };
  const pressed = signal(false);
  const presses = repeater((down) => act(props.game(), props.action, down, options), { delay: props.repeatDelay, interval: props.repeat });
  const held = heldKey((down) => {
    pressed.set(down);
    if (down) presses.start();
    else presses.stop();
  });
  cleanup(() => {
    if (untracked(pressed)) presses.stop();
  });
  const ids = (e: GestureResponderEvent) => e.changedTouches.map((t) => t.identifier);
  const fill = () => (props.cooldown === undefined ? 0 : cooldownFill(props.cooldown()));
  return (
    <View
      testID={actionButtonId(props.action, props.x, props.y)}
      class={`w-16 h-16 rounded-full items-center justify-center overflow-hidden ${pressedLook(pressed())} ${props.class ?? ""}`}
      onTouchStart={(e: GestureResponderEvent) => held.start(ids(e))}
      onTouchEnd={(e: GestureResponderEvent) => held.end(ids(e))}
      onTouchCancel={(e: GestureResponderEvent) => held.end(ids(e))}
    >
      {fill() > 0 ? <View testID={`${actionButtonId(props.action, props.x, props.y)}-cooldown`} class="absolute left-0 right-0 bottom-0 bg-black/50" style={{ pointerEvents: "none", height: `${Math.round(fill() * 100)}%` }} /> : null}
      <Text class="text-white text-2xl">{props.label}</Text>
    </View>
  );
}

/**
 * 虚拟摇杆：自己是一块收触摸的区域（用 `class` / `style` 摆位置和大小），按下的手指驱动动作 `action`（`act`，来源 `source`，默认 "stick"）。
 * `mode` "float"（默认）以按下的点为中心，"fixed" 以 `center` 为中心；`radius` 推到底的距离，`deadZone` 死区占半径的比例；
 * `directions` "analog"（默认，连续值）、"8"、"4"。别的手指按在区域里不管，多根手指的摇杆和按钮各算各的。
 */
export function VirtualStick(props: {
  game: () => NativeComponentRef | undefined;
  action: string;
  player?: number;
  mode?: "float" | "fixed";
  radius?: number;
  deadZone?: number;
  directions?: Directions;
  center?: Vec;
  source?: string;
  class?: string;
  style?: ViewProps["style"];
  testID?: string;
}) {
  const shape = { radius: props.radius ?? STICK_DEFAULTS.radius, deadZone: props.deadZone ?? STICK_DEFAULTS.deadZone };
  const size = signal<Vec>([0, 0]);
  const finger = signal<StickState>(null);
  let sent: Vec = [0, 0];
  /** 没按着时底座在哪；固定模式的中心。 */
  const rest = (): Vec => props.center ?? [shape.radius + 16, size()[1] - shape.radius - 16];
  const send = (next: Vec) => {
    const c = stickCommand(sent, next);
    if (c === null) return;
    sent = c.down ? [c.x!, c.y!] : [0, 0];
    const player = props.player === undefined ? {} : { player: props.player };
    act(props.game(), props.action, c.down, { ...player, source: props.source ?? "stick", ...(c.down ? { x: c.x, y: c.y } : {}) });
  };
  const on = (phase: "start" | "move" | "end" | "cancel") => (e: GestureResponderEvent) => {
    const place = { mode: props.mode ?? "float", center: untracked(rest) };
    let s = untracked(finger);
    for (const t of e.changedTouches) s = stickStep(s, { phase, id: t.identifier, x: t.locationX, y: t.locationY }, place);
    finger.set(s);
    send(stickOutput(s, shape, props.directions ?? "analog"));
  };
  cleanup(() => send([0, 0]));
  const center = (): Vec => finger()?.center ?? rest();
  const knob = (): Vec => {
    const f = finger();
    return f === null ? [0, 0] : knobOffset(f.at[0] - f.center[0], f.at[1] - f.center[1], shape.radius);
  };
  const r = shape.radius;
  /** 摇杆头的直径。 */
  const k = r;
  return (
    <View
      testID={props.testID ?? `stick-${props.action}`}
      class={props.class}
      style={props.style}
      onLayout={(e) => size.set([e.bounds.size.width, e.bounds.size.height])}
      onTouchStart={on("start")}
      onTouchMove={on("move")}
      onTouchEnd={on("end")}
      onTouchCancel={on("cancel")}
    >
      <View
       
        class={`absolute rounded-full border-2 border-white/40 ${finger() === null ? "bg-white/5 opacity-50" : "bg-white/15"}`}
        style={{ pointerEvents: "none", left: center()[0] - r, top: center()[1] - r, width: 2 * r, height: 2 * r }}
      />
      <View
       
        class={`absolute rounded-full ${finger() === null ? "bg-white/20" : "bg-white/50"}`}
        style={{ pointerEvents: "none", left: center()[0] + knob()[0] - k / 2, top: center()[1] + knob()[1] - k / 2, width: k, height: k }}
      />
    </View>
  );
}
