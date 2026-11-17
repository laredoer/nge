// 触屏控制的纯逻辑（docs/touch-input.md）：摇杆的数学和手指跟踪、手势状态机、按钮的连发和冷却。
import { expect, test } from "nsh/test";
import { GESTURE_IDLE, gestureHandlers, gestureStep, type Gesture, type GestureInput, type GestureState } from "../../ts/gestures";
import { knobOffset, snapDirection, stickCommand, stickOutput, stickStep, stickValue, type StickState, type StickTouch } from "../../ts/stick";
import { cooldownFill, pressedLook, repeater } from "../../ts/touch";

const near = (a: number[], b: number[], eps = 1e-6) => expect(a.every((v, i) => Math.abs(v - b[i]) < eps)).toBe(true);

test("摇杆：死区里是 0，死区外从 0 开始重新铺满，到半径是 1，y 朝上", () => {
  const shape = { radius: 60, deadZone: 0.2 };
  expect(stickValue(0, 0, shape)).toEqual([0, 0]);
  expect(stickValue(11, 0, shape)).toEqual([0, 0]);
  // 死区边上一点点
  near(stickValue(13, 0, shape), [1 / 48, 0]);
  near(stickValue(36, 0, shape), [0.5, 0]);
  near(stickValue(60, 0, shape), [1, 0]);
  // 屏幕往上推是正的 y
  near(stickValue(0, -60, shape), [0, 1]);
  near(stickValue(-36, 0, shape), [-0.5, 0]);
});

test("摇杆：超过半径按 1，方向不变；斜着推长度也不超过 1", () => {
  const shape = { radius: 60, deadZone: 0.2 };
  near(stickValue(300, 0, shape), [1, 0]);
  const v = stickValue(100, -100, shape);
  near(v, [Math.SQRT1_2, Math.SQRT1_2]);
  // 死区 0：按长度比例
  near(stickValue(30, 0, { radius: 60, deadZone: 0 }), [0.5, 0]);
  // 坏的半径不出 NaN
  expect(stickValue(10, 0, { radius: 0, deadZone: 0.2 })).toEqual([0, 0]);
});

test("摇杆：八方向按 45° 一格、四方向取大的轴，各分量是 −1 / 0 / 1", () => {
  expect(snapDirection([0, 0], "8")).toEqual([0, 0]);
  expect(snapDirection([0.9, 0.1], "8")).toEqual([1, 0]);
  expect(snapDirection([0.6, 0.5], "8")).toEqual([1, 1]);
  expect(snapDirection([-0.5, -0.6], "8")).toEqual([-1, -1]);
  expect(snapDirection([-0.1, 0.9], "8")).toEqual([0, 1]);
  expect(snapDirection([-0.9, 0.3], "8")).toEqual([-1, 0]);
  expect(snapDirection([0.6, 0.5], "4")).toEqual([1, 0]);
  expect(snapDirection([0.5, -0.6], "4")).toEqual([0, -1]);
  expect(snapDirection([-0.7, 0.2], "4")).toEqual([-1, 0]);
});

test("摇杆头限在半径以内", () => {
  expect(knobOffset(10, -20, 60)).toEqual([10, -20]);
  near(knobOffset(120, 0, 60), [60, 0]);
  near(knobOffset(-90, 120, 60), [-36, 48]);
});

const run = (touches: StickTouch[], place: { mode: "float" | "fixed"; center: [number, number] }) => touches.reduce<StickState>((s, t) => stickStep(s, t, place), null);
const FLOAT = { mode: "float" as const, center: [80, 300] as [number, number] };

test("浮动摇杆：第一根按下的手指接管，按下的地方是中心；别的手指不管", () => {
  let s = run([{ phase: "start", id: 1, x: 100, y: 200 }], FLOAT);
  expect(s).toEqual({ id: 1, center: [100, 200], at: [100, 200] });
  s = stickStep(s, { phase: "start", id: 2, x: 10, y: 10 }, FLOAT);
  s = stickStep(s, { phase: "move", id: 2, x: 0, y: 0 }, FLOAT);
  expect(s).toEqual({ id: 1, center: [100, 200], at: [100, 200] });
  s = stickStep(s, { phase: "move", id: 1, x: 160, y: 200 }, FLOAT);
  near(stickOutput(s, { radius: 60, deadZone: 0.2 }, "analog"), [1, 0]);
  // 别的手指抬起不放开
  s = stickStep(s, { phase: "end", id: 2, x: 0, y: 0 }, FLOAT);
  expect(s?.id).toBe(1);
});

test("固定摇杆：中心是给定的位置", () => {
  const s = run([{ phase: "start", id: 5, x: 110, y: 300 }], { mode: "fixed", center: [80, 300] });
  expect(s).toEqual({ id: 5, center: [80, 300], at: [110, 300] });
  near(stickOutput(s, { radius: 60, deadZone: 0 }, "analog"), [0.5, 0]);
});

test("手指抬起、被取消都放开，放开后下一根手指重新接管；丢了 end 时同一个 id 再按下当新的一次", () => {
  for (const phase of ["end", "cancel"] as const) {
    let s = run([{ phase: "start", id: 1, x: 0, y: 0 }, { phase: "move", id: 1, x: 50, y: 0 }], FLOAT);
    s = stickStep(s, { phase, id: 1, x: 50, y: 0 }, FLOAT);
    expect(s).toBe(null);
    expect(stickOutput(s, { radius: 60, deadZone: 0.2 }, "analog")).toEqual([0, 0]);
    s = stickStep(s, { phase: "start", id: 2, x: 30, y: 40 }, FLOAT);
    expect(s).toEqual({ id: 2, center: [30, 40], at: [30, 40] });
  }
  const again = run([{ phase: "start", id: 1, x: 0, y: 0 }, { phase: "move", id: 1, x: 50, y: 0 }, { phase: "start", id: 1, x: 200, y: 200 }], FLOAT);
  expect(again).toEqual({ id: 1, center: [200, 200], at: [200, 200] });
  // 没接管的 move / end 不起作用
  expect(run([{ phase: "move", id: 3, x: 1, y: 1 }, { phase: "end", id: 3, x: 1, y: 1 }], FLOAT)).toBe(null);
});

test("摇杆输出按方向模式量化", () => {
  const s = run([{ phase: "start", id: 1, x: 0, y: 0 }, { phase: "move", id: 1, x: 40, y: -35 }], FLOAT);
  const shape = { radius: 60, deadZone: 0.2 };
  expect(stickOutput(s, shape, "8")).toEqual([1, 1]);
  expect(stickOutput(s, shape, "4")).toEqual([1, 0]);
  // 死区里量化后也是 0
  const still = run([{ phase: "start", id: 1, x: 0, y: 0 }, { phase: "move", id: 1, x: 5, y: 0 }], FLOAT);
  expect(stickOutput(still, shape, "8")).toEqual([0, 0]);
});

test("摇杆发什么：值变了才发，0 是松开，保留两位小数", () => {
  expect(stickCommand([0, 0], [0, 0])).toBe(null);
  expect(stickCommand([0, 0], [0.5, 0.25])).toEqual({ down: true, x: 0.5, y: 0.25 });
  expect(stickCommand([0.5, 0.25], [0.501, 0.2499])).toBe(null);
  expect(stickCommand([0.5, 0.25], [0.333333, -1])).toEqual({ down: true, x: 0.33, y: -1 });
  expect(stickCommand([0.5, 0.25], [0.001, 0])).toEqual({ down: false });
});

// ——— 手势 ———

const OPTS = { slop: 10, longPress: 500, swipeDistance: 30, swipeSpeed: 300 };

function feed(inputs: GestureInput[], from: GestureState = GESTURE_IDLE): { state: GestureState; events: Gesture[] } {
  let state = from;
  const events: Gesture[] = [];
  for (const i of inputs) {
    const r = gestureStep(state, i, OPTS);
    state = r.state;
    events.push(...r.events);
  }
  return { state, events };
}
const kinds = (events: Gesture[]) => events.map((e) => e.kind);
const down = (id: number, x: number, y: number, t: number): GestureInput => ({ phase: "start", id, x, y, t });
const move = (id: number, x: number, y: number, t: number): GestureInput => ({ phase: "move", id, x, y, t });
const up = (id: number, x: number, y: number, t: number): GestureInput => ({ phase: "end", id, x, y, t });
const cancel = (id: number, x: number, y: number, t: number): GestureInput => ({ phase: "cancel", id, x, y, t });

test("轻点：没挪出 slop、长按之前抬起；挪一点点也算", () => {
  const r = feed([down(1, 100, 100, 0), move(1, 105, 104, 50), up(1, 106, 104, 120)]);
  expect(r.events).toEqual([{ kind: "tap", x: 100, y: 100 }]);
  expect(r.state).toEqual(GESTURE_IDLE);
});

test("长按：计时到了发，抬起时不再发轻点；计时器晚了、抬起时已经超时也补发", () => {
  const r = feed([down(1, 50, 60, 0), move(1, 52, 60, 200), { phase: "hold", id: 1 }]);
  expect(r.events).toEqual([{ kind: "longPress", x: 52, y: 60 }]);
  expect(kinds(feed([up(1, 52, 60, 900)], r.state).events)).toEqual([]);
  const late = feed([down(1, 50, 60, 0), up(1, 50, 60, 600)]);
  expect(late.events).toEqual([{ kind: "longPress", x: 50, y: 60 }]);
  // 挪出 slop 以后计时到了不算长按；已经抬起的手指的计时也不算
  expect(kinds(feed([down(1, 0, 0, 0), move(1, 20, 0, 100), { phase: "hold", id: 1 }]).events)).toEqual(["dragStart"]);
  expect(kinds(feed([down(1, 0, 0, 0), up(1, 0, 0, 100), { phase: "hold", id: 1 }]).events)).toEqual(["tap"]);
});

test("拖动：挪出 slop 开始，每步带这一步和总的位移，抬起时带速度", () => {
  const r = feed([down(1, 0, 0, 0), move(1, 5, 0, 16), move(1, 15, 0, 32), move(1, 25, 5, 48), up(1, 25, 5, 64)]);
  expect(kinds(r.events)).toEqual(["dragStart", "drag", "dragEnd"]);
  expect(r.events[0]).toEqual({ kind: "dragStart", x: 15, y: 0, dx: 15, dy: 0, tx: 15, ty: 0 });
  expect(r.events[1]).toEqual({ kind: "drag", x: 25, y: 5, dx: 10, dy: 5, tx: 25, ty: 5 });
  const end = r.events[2] as Extract<Gesture, { kind: "dragEnd" }>;
  expect(end.cancelled).toBe(false);
  // 最后 100 ms 里从 (0, 0) 到 (25, 5)，用了 64 ms
  near([end.vx, end.vy], [25 / 0.064, 5 / 0.064], 1e-3);
  // 长按以后再挪也是拖动，不发轻点
  const held = feed([down(1, 0, 0, 0), { phase: "hold", id: 1 }, move(1, 30, 0, 700), up(1, 30, 0, 2000)]);
  expect(kinds(held.events)).toEqual(["longPress", "dragStart", "dragEnd"]);
});

test("滑动：拖动正常抬起、够远够快时发，方向按位移大的轴，屏幕上方是 up", () => {
  const fast = feed([down(1, 100, 100, 0), move(1, 100, 60, 40), move(1, 105, 20, 80), up(1, 105, 20, 90)]);
  expect(kinds(fast.events)).toEqual(["dragStart", "drag", "dragEnd", "swipe"]);
  const swipe = fast.events[3] as Extract<Gesture, { kind: "swipe" }>;
  expect(swipe.direction).toBe("up");
  expect(swipe.speed > 300).toBe(true);
  expect(feed([down(1, 0, 0, 0), move(1, -50, 10, 50), up(1, -50, 10, 60)]).events.at(-1)).toMatchObject({ kind: "swipe", direction: "left" });
  // 慢：只有拖动
  expect(kinds(feed([down(1, 0, 0, 0), move(1, 40, 0, 500), move(1, 41, 0, 1000), up(1, 41, 0, 1500)]).events)).toEqual(["dragStart", "drag", "dragEnd"]);
  // 不够远：快也不算
  expect(kinds(feed([down(1, 0, 0, 0), move(1, 20, 0, 10), up(1, 20, 0, 20)]).events)).toEqual(["dragStart", "dragEnd"]);
  // 取消：不算滑动
  const cancelled = feed([down(1, 0, 0, 0), move(1, 80, 0, 40), cancel(1, 80, 0, 50)]);
  expect(kinds(cancelled.events)).toEqual(["dragStart", "dragEnd"]);
  expect((cancelled.events[1] as Extract<Gesture, { kind: "dragEnd" }>).cancelled).toBe(true);
});

test("捏合：第二根手指按下开始，scale 是距离比，factor 是这一步的倍数，中点和中点的位移", () => {
  const r = feed([down(1, 100, 100, 0), down(2, 200, 100, 10), move(2, 300, 100, 20), move(1, 0, 100, 30), up(2, 300, 100, 40)]);
  expect(kinds(r.events)).toEqual(["pinchStart", "pinch", "pinch", "pinchEnd"]);
  expect(r.events[0]).toEqual({ kind: "pinchStart", x: 150, y: 100, scale: 1, rotation: 0 });
  const p1 = r.events[1] as Extract<Gesture, { kind: "pinch" }>;
  near([p1.scale, p1.factor, p1.x, p1.y, p1.dx, p1.dy], [2, 2, 200, 100, 50, 0]);
  const p2 = r.events[2] as Extract<Gesture, { kind: "pinch" }>;
  near([p2.scale, p2.factor, p2.x, p2.dx], [3, 1.5, 150, -50]);
  expect(r.events[3]).toMatchObject({ kind: "pinchEnd", cancelled: false, scale: 3 });
  // 一根抬起后剩下的手指不算任何手势，全部抬起后重新开始
  const rest = feed([move(1, 50, 50, 50), up(1, 50, 50, 60)], r.state);
  expect(rest.events).toEqual([]);
  expect(rest.state).toEqual(GESTURE_IDLE);
  expect(kinds(feed([down(3, 0, 0, 70), up(3, 0, 0, 80)], rest.state).events)).toEqual(["tap"]);
});

test("旋转：屏幕上顺时针为正，跨过 ±180° 按最短方向算", () => {
  const r = feed([down(1, 0, 0, 0), down(2, 100, 0, 0), move(2, 0, 100, 10)]);
  const p = r.events[1] as Extract<Gesture, { kind: "pinch" }>;
  near([p.rotation, p.turn, p.scale], [90, 90, 1]);
  // 从 170° 转到 -170°：是 +20°，不是 -340°
  const a = (deg: number) => [Math.cos((deg * Math.PI) / 180) * 100, Math.sin((deg * Math.PI) / 180) * 100];
  const [x1, y1] = a(170);
  const [x2, y2] = a(-170);
  const w = feed([down(1, 0, 0, 0), down(2, x1, y1, 0), move(2, x2, y2, 10)]);
  near([(w.events[1] as Extract<Gesture, { kind: "pinch" }>).turn], [20], 1e-6);
});

test("拖动中第二根手指按下：拖动以取消结束，换成捏合；第三根手指不管", () => {
  const r = feed([down(1, 0, 0, 0), move(1, 30, 0, 10), down(2, 130, 0, 20), down(3, 500, 500, 25), move(3, 0, 0, 26), up(3, 0, 0, 27), move(2, 230, 0, 30)]);
  expect(kinds(r.events)).toEqual(["dragStart", "dragEnd", "pinchStart", "pinch"]);
  expect((r.events[1] as Extract<Gesture, { kind: "dragEnd" }>).cancelled).toBe(true);
  near([(r.events[3] as Extract<Gesture, { kind: "pinch" }>).factor], [2]);
});

test("捏合被取消：pinchEnd 带 cancelled，剩下的手指不算", () => {
  const r = feed([down(1, 0, 0, 0), down(2, 100, 0, 0), cancel(1, 0, 0, 10), up(2, 100, 0, 20)]);
  expect(kinds(r.events)).toEqual(["pinchStart", "pinchEnd"]);
  expect((r.events[1] as Extract<Gesture, { kind: "pinchEnd" }>).cancelled).toBe(true);
  expect(r.state).toEqual(GESTURE_IDLE);
});

test("两指叠在一起开始不除零；不认识的手指不管；丢了 end 的 id 再按下当新的", () => {
  const r = feed([down(1, 50, 50, 0), down(2, 50, 50, 0), move(2, 60, 50, 10)]);
  for (const e of r.events) if (e.kind === "pinch") expect(Number.isFinite(e.scale) && Number.isFinite(e.factor)).toBe(true);
  expect(feed([move(9, 1, 1, 0), up(9, 1, 1, 0), cancel(9, 1, 1, 0)]).events).toEqual([]);
  expect(kinds(feed([down(1, 0, 0, 0), down(1, 300, 300, 100), up(1, 300, 300, 150)]).events)).toEqual(["tap"]);
});

test("gestureHandlers：每根变了的手指喂进状态机，按下时开长按计时，抬起时关掉", () => {
  const got: Gesture[] = [];
  const pending = new Map<number, () => void>();
  let next = 1;
  const timers = {
    set: (fn: () => void, _ms: number) => {
      pending.set(next, fn);
      return next++;
    },
    clear: (h: number) => void pending.delete(h),
  };
  const h = gestureHandlers((g) => got.push(g), {}, timers);
  const ev = (touches: [number, number, number, number][]) => ({ changedTouches: touches.map(([identifier, locationX, locationY, timestamp]) => ({ identifier, locationX, locationY, timestamp })) }) as never;
  h.onTouchStart(ev([[1, 10, 10, 0]]));
  expect(pending.size).toBe(1);
  h.onTouchEnd(ev([[1, 10, 10, 50]]));
  expect(pending.size).toBe(0);
  expect(got).toEqual([{ kind: "tap", x: 10, y: 10 }]);
  h.onTouchStart(ev([[2, 0, 0, 100]]));
  for (const fn of [...pending.values()]) fn();
  expect(got.at(-1)).toEqual({ kind: "longPress", x: 0, y: 0 });
  h.onTouchCancel(ev([[2, 0, 0, 200]]));
  // 一个事件里两根手指同时按下
  h.onTouchStart(ev([[3, 0, 0, 300], [4, 100, 0, 300]]));
  expect(got.at(-1)).toEqual({ kind: "pinchStart", x: 50, y: 0, scale: 1, rotation: 0 });
});

// ——— 按钮 ———

function fakeTimers() {
  const pending = new Map<number, { fn: () => void; ms: number }>();
  let next = 1;
  return {
    pending,
    timers: {
      set: (fn: () => void, ms: number) => {
        pending.set(next, { fn, ms });
        return next++;
      },
      clear: (h: number) => void pending.delete(h),
    },
    fire() {
      const [[h, t]] = [...pending];
      pending.delete(h);
      t.fn();
      return t.ms;
    },
  };
}

test("按住连发：按下立刻发，等 delay 后每隔 interval 重按一次（一对松开、按下），松开时停", () => {
  const sent: boolean[] = [];
  const f = fakeTimers();
  const r = repeater((d) => sent.push(d), { delay: 0.4, interval: 0.15 }, f.timers);
  r.start();
  expect(sent).toEqual([true]);
  expect(f.fire()).toBe(400);
  expect(sent).toEqual([true, false, true]);
  expect(f.fire()).toBe(150);
  expect(sent).toEqual([true, false, true, false, true]);
  r.stop();
  expect(sent.at(-1)).toBe(false);
  expect(f.pending.size).toBe(0);
  // 没有 interval：不连发
  const plain: boolean[] = [];
  const g = fakeTimers();
  const p = repeater((d) => plain.push(d), {}, g.timers);
  p.start();
  p.stop();
  expect(plain).toEqual([true, false]);
  expect(g.pending.size).toBe(0);
});

test("冷却：截断在 0 … 1，坏的值当 0；按下时的样子和平时不一样", () => {
  expect([cooldownFill(0.4), cooldownFill(-1), cooldownFill(3), cooldownFill(Number.NaN)]).toEqual([0.4, 0, 1, 0]);
  expect(pressedLook(true) === pressedLook(false)).toBe(false);
});
