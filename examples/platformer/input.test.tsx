// 输入映射的 TSX 部分（见 docs/input-actions.md）：改键、冲突、恢复默认、存取，动作和手柄的事件。用内存里的存储测。
import { expect, test } from "nsh/test";
import { bindingsOf, conflictsOf, inputMap, KEY_NAMES, keyAt, slotsOf, withKey, type Bindings, type InputStore, type Slot } from "../../ts/input";

const P1: Bindings = {
  jump: { kind: "button", keys: ["Space", "GamepadA"] },
  move: { kind: "axis", negative: ["KeyA", "GamepadLeft"], positive: ["KeyD", "GamepadRight"], analog: ["LeftStickX"] },
  aim: { kind: "axis2d", left: ["KeyF"], right: ["KeyH"], down: ["KeyG"], up: ["KeyT"], analog: ["RightStick"] },
};
const P2: Bindings = {
  jump: { kind: "button", keys: ["KeyI", "GamepadA"] },
  move: { kind: "axis", negative: ["KeyJ"], positive: ["KeyL"] },
};
const DEFAULTS = [P1, P2];
const KEY = "nge:input:test";

function memory(init: Record<string, string> = {}) {
  const items = new Map(Object.entries(init));
  const store: InputStore & { full: boolean; items: Map<string, string> } = {
    full: false,
    items,
    getItem: async (k) => items.get(k) ?? null,
    setItem: async (k, v) => {
      if (store.full) throw new Error("存储满了");
      items.set(k, v);
    },
  };
  return store;
}

const make = (store: InputStore = memory()) => inputMap({ key: "test", defaults: DEFAULTS, store });
const slot = (player: number, action: string, part: Slot["part"], index: number): Slot => ({ player, action, part, index });
const down = (code: string) => ({ code, down: true, repeat: false });
/** 等存储的读写落地。 */
const settle = () => new Promise((r) => setTimeout(r, 0));

test("一个动作的每个键是一个位置", () => {
  expect(slotsOf(DEFAULTS, 0, "jump")).toEqual([slot(0, "jump", "keys", 0), slot(0, "jump", "keys", 1)]);
  expect(slotsOf(DEFAULTS, 0, "move").map((s) => `${s.part}${s.index}`)).toEqual(["negative0", "negative1", "positive0", "positive1"]);
  expect(slotsOf(DEFAULTS, 0, "aim").map((s) => s.part)).toEqual(["left", "right", "down", "up"]);
  expect(slotsOf(DEFAULTS, 0, "nope")).toEqual([]);
  expect(slotsOf(DEFAULTS, 5, "jump")).toEqual([]);
  expect(keyAt(DEFAULTS, slot(0, "move", "positive", 1))).toBe("GamepadRight");
  expect(keyAt(DEFAULTS, slot(0, "jump", "negative", 0))).toBeUndefined();
  expect(keyAt(DEFAULTS, slot(0, "jump", "keys", 9))).toBeUndefined();
});

test("换一个键得到新的表，原来的不动", () => {
  const next = withKey(DEFAULTS, slot(0, "jump", "keys", 0), "KeyX");
  expect(keyAt(next, slot(0, "jump", "keys", 0))).toBe("KeyX");
  expect(keyAt(DEFAULTS, slot(0, "jump", "keys", 0))).toBe("Space");
  expect(next[1]).toEqual(P2);
  // 位置在末尾之后：加一个键
  const more = withKey(DEFAULTS, slot(0, "jump", "keys", 2), "KeyZ");
  expect(slotsOf(more, 0, "jump")).toHaveLength(3);
  // 没有这个动作、这个部分：不变
  expect(withKey(DEFAULTS, slot(0, "nope", "keys", 0), "KeyZ")).toEqual(DEFAULTS);
  expect(withKey(DEFAULTS, slot(0, "jump", "up", 0), "KeyZ")).toEqual(DEFAULTS);
  // null：去掉这个键
  expect(slotsOf(withKey(DEFAULTS, slot(0, "jump", "keys", 0), null), 0, "jump")).toHaveLength(1);
});

test("键盘的键查所有玩家，手柄的键只查同一个玩家，同一个动作里的不算", () => {
  const target = slot(0, "jump", "keys", 0);
  expect(conflictsOf(DEFAULTS, target, "KeyD")).toEqual([slot(0, "move", "positive", 0)]);
  expect(conflictsOf(DEFAULTS, target, "KeyL")).toEqual([slot(1, "move", "positive", 0)]);
  expect(conflictsOf(DEFAULTS, target, "KeyX")).toEqual([]);
  expect(conflictsOf(DEFAULTS, target, "GamepadA")).toEqual([]);
  expect(conflictsOf(DEFAULTS, slot(0, "move", "negative", 0), "GamepadA")).toEqual([slot(0, "jump", "keys", 1)]);
  expect(conflictsOf(DEFAULTS, slot(1, "move", "negative", 0), "GamepadLeft")).toEqual([]);
});

test("没冲突直接换，有冲突不换并给出冲突的位置", () => {
  const input = make();
  expect(input.bindings()).toEqual(DEFAULTS);
  expect(input.bind(slot(0, "jump", "keys", 0), "KeyX")).toEqual([]);
  expect(input.key(slot(0, "jump", "keys", 0))).toBe("KeyX");
  expect(input.bind(slot(0, "jump", "keys", 0), "KeyA")).toEqual([slot(0, "move", "negative", 0)]);
  expect(input.key(slot(0, "jump", "keys", 0))).toBe("KeyX");
  expect(input.slots(0, "jump")).toHaveLength(2);
});

test("按任意键来绑：下一个按下的键换上去", () => {
  const input = make();
  expect(input.handleKey(down("KeyX"))).toBe(false);
  input.capture(slot(0, "jump", "keys", 0));
  expect(input.capturing()).toEqual(slot(0, "jump", "keys", 0));
  expect(input.prop().capturing).toBe(true);
  // 松开、自动重复不算，但也不漏给游戏
  expect(input.handleKey({ code: "KeyQ", down: false, repeat: false })).toBe(true);
  expect(input.handleKey({ code: "KeyQ", down: true, repeat: true })).toBe(true);
  expect(input.capturing()).not.toBeNull();
  expect(input.handleKey(down("GamepadY"))).toBe(true);
  expect(input.key(slot(0, "jump", "keys", 0))).toBe("GamepadY");
  expect(input.capturing()).toBeNull();
  expect(input.prop().capturing).toBe(false);
  expect(input.handleKey(down("KeyX"))).toBe(false);
});

test("Escape 取消", () => {
  const input = make();
  input.capture(slot(0, "jump", "keys", 0));
  expect(input.handleKey(down("Escape"))).toBe(true);
  expect(input.capturing()).toBeNull();
  expect(input.bindings()).toEqual(DEFAULTS);
});

test("冲突时先问：确认就对调，取消就不变", () => {
  const input = make();
  const target = slot(0, "jump", "keys", 0);
  input.capture(target);
  input.handleKey(down("KeyA"));
  expect(input.bindings()).toEqual(DEFAULTS);
  expect(input.pending()).toEqual({ slot: target, code: "KeyA", conflicts: [slot(0, "move", "negative", 0)], blocked: [] });
  expect(input.prop().capturing).toBe(true);
  // 等着回答时再按键不算
  expect(input.handleKey(down("KeyZ"))).toBe(true);
  expect(input.pending()?.code).toBe("KeyA");
  input.cancel();
  expect(input.pending()).toBeNull();
  expect(input.bindings()).toEqual(DEFAULTS);

  input.capture(target);
  input.handleKey(down("KeyA"));
  input.confirm();
  expect(input.pending()).toBeNull();
  expect(input.key(target)).toBe("KeyA");
  expect(input.key(slot(0, "move", "negative", 0))).toBe("Space");
  expect(input.prop().capturing).toBe(false);
});

test("加新键时冲突，确认后对方那个键去掉", () => {
  const input = make();
  const fresh = slot(0, "jump", "keys", 2);
  input.capture(fresh);
  input.handleKey(down("KeyD"));
  input.confirm();
  expect(input.key(fresh)).toBe("KeyD");
  expect(input.slots(0, "move").map((s) => input.key(s))).toEqual(["KeyA", "GamepadLeft", "GamepadRight"]);
});

test("恢复默认", () => {
  const input = make();
  input.bind(slot(0, "jump", "keys", 0), "KeyX");
  input.capture(slot(0, "jump", "keys", 1));
  input.reset();
  expect(input.bindings()).toEqual(DEFAULTS);
  expect(input.capturing()).toBeNull();
});

test("改了就存，下次读回来", async () => {
  const store = memory();
  const input = make(store);
  await input.loaded;
  expect(store.items.size).toBe(0);
  input.bind(slot(0, "jump", "keys", 0), "KeyX");
  await settle();
  expect(JSON.parse(store.items.get(KEY)!)).toMatchObject({ v: 1, players: [{ jump: { kind: "button", keys: ["KeyX", "GamepadA"] } }, {}] });

  const again = make(store);
  expect(again.ready()).toBe(false);
  expect(again.bindings()).toEqual(DEFAULTS);
  await again.loaded;
  expect(again.ready()).toBe(true);
  expect(again.key(slot(0, "jump", "keys", 0))).toBe("KeyX");
  again.reset();
  await settle();
  expect(make(store).bindings()).toEqual(DEFAULTS);
  expect(JSON.parse(store.items.get(KEY)!).players).toEqual(DEFAULTS);
});

test("读完之前改的以改的为准", async () => {
  const stored = JSON.stringify({ v: 1, players: withKey(DEFAULTS, slot(0, "jump", "keys", 0), "KeyX") });
  const input = make(memory({ [KEY]: stored }));
  input.bind(slot(0, "jump", "keys", 0), "KeyZ");
  await input.loaded;
  expect(input.key(slot(0, "jump", "keys", 0))).toBe("KeyZ");
});

test("存的表按默认的对一遍", () => {
  const stored = {
    // 改过的留着；模拟量不能改，用默认的
    jump: { kind: "button", keys: ["KeyX"] },
    move: { kind: "axis", negative: ["KeyQ"], positive: ["KeyE"], analog: ["R2"] },
    // 种类变了（游戏更新了）：用默认的
    aim: { kind: "button", keys: ["KeyP"] },
    // 默认里没有的：丢掉
    old: { kind: "button", keys: ["KeyO"] },
  };
  const got = bindingsOf({ v: 1, players: [stored] }, DEFAULTS)!;
  expect(got[0].jump).toEqual({ kind: "button", keys: ["KeyX"] });
  expect(got[0].move).toEqual({ kind: "axis", negative: ["KeyQ"], positive: ["KeyE"], analog: ["LeftStickX"] });
  expect(got[0].aim).toEqual(P1.aim);
  expect("old" in got[0]).toBe(false);
  // 少了的玩家、少了的动作用默认的
  expect(got[1]).toEqual(P2);
  expect(bindingsOf({ v: 1, players: [{ jump: { kind: "button", keys: ["KeyX", 3] } }] }, DEFAULTS)![0].jump).toEqual(P1.jump);
  expect(bindingsOf({ v: 1, players: [null] }, DEFAULTS)![0]).toEqual(P1);
  // 种类不一样，就算凑巧带着同名的部分也不用
  expect(bindingsOf({ v: 1, players: [{ jump: { kind: "axis", keys: ["KeyX"], negative: [], positive: [] } }] }, DEFAULTS)![0].jump).toEqual(P1.jump);
});

test("结构不对、版本不对的整份不要", async () => {
  for (const bad of [null, 3, "x", [], {}, { v: 2, players: [] }, { v: 1 }, { v: 1, players: {} }]) {
    expect(bindingsOf(bad, DEFAULTS)).toBeNull();
  }
  for (const raw of ["{坏的", "null", '{"v":9,"players":[]}']) {
    const input = make(memory({ [KEY]: raw }));
    await input.loaded;
    expect(input.bindings()).toEqual(DEFAULTS);
    expect(input.ready()).toBe(true);
  }
});

test("存不进去不影响这一次的绑定", async () => {
  const store = memory();
  store.full = true;
  const input = make(store);
  input.bind(slot(0, "jump", "keys", 0), "KeyX");
  await settle();
  expect(input.key(slot(0, "jump", "keys", 0))).toBe("KeyX");
  expect(store.items.size).toBe(0);
});

test("动作的状态来自 action 事件", () => {
  const input = make();
  expect(input.down("jump")).toBe(false);
  expect(input.value("move")).toEqual([0, 0]);
  input.handle({ action: "jump", player: 0, down: true, x: 1, y: 0 });
  input.handle({ action: "move", player: 1, down: true, x: -1, y: 0 });
  expect(input.down("jump")).toBe(true);
  expect(input.down("jump", 1)).toBe(false);
  expect(input.value("move", 1)).toEqual([-1, 0]);
  expect(input.value("move")).toEqual([0, 0]);
  input.handle({ action: "jump", player: 0, down: false, x: 0, y: 0 });
  expect(input.down("jump")).toBe(false);
});

test("手柄接入、拔出、换玩家", () => {
  const input = make();
  expect(input.padOf(0)).toBeNull();
  input.handlePad({ id: 4, connected: true, player: 0 });
  input.handlePad({ id: 7, connected: true, player: null });
  expect(input.pads().map((p) => p.id)).toEqual([4, 7]);
  expect([input.padOf(0), input.padOf(1)]).toEqual([4, null]);
  // 同一个手柄再报一遍：换掉，不重复
  input.handlePad({ id: 7, connected: true, player: 1 });
  expect(input.pads()).toHaveLength(2);
  expect(input.padOf(1)).toBe(7);
  input.handlePad({ id: 4, connected: false, player: null });
  expect(input.pads().map((p) => p.id)).toEqual([7]);
  expect(input.padOf(0)).toBeNull();
  input.handlePad({ id: 99, connected: false, player: null });
  expect(input.pads()).toHaveLength(1);
});

test("prop 是现在的绑定表", () => {
  const input = make();
  input.bind(slot(1, "jump", "keys", 0), "KeyU");
  expect(input.prop()).toEqual({ players: input.bindings(), capturing: false });
  expect(input.prop().players[1].jump).toEqual({ kind: "button", keys: ["KeyU", "GamepadA"] });
});

test("同一个动作的另一部分绑了同一个键也算冲突，同一部分里的不算", () => {
  expect(conflictsOf(DEFAULTS, slot(0, "move", "negative", 0), "KeyD")).toEqual([slot(0, "move", "positive", 0)]);
  expect(conflictsOf(DEFAULTS, slot(0, "move", "negative", 0), "GamepadLeft")).toEqual([]);
  expect(conflictsOf(DEFAULTS, slot(0, "aim", "up", 0), "KeyG")).toEqual([slot(0, "aim", "down", 0)]);
  // 确认：轴的两边对调
  const input = make();
  input.capture(slot(0, "move", "negative", 0));
  input.handleKey(down("KeyD"));
  expect(input.pending()?.conflicts).toEqual([slot(0, "move", "positive", 0)]);
  expect(input.confirm()).toBe(true);
  expect(input.bindings()[0].move).toMatchObject({ negative: ["KeyD", "GamepadLeft"], positive: ["KeyA", "GamepadRight"] });
});

test("同一部分里不留重复的键", () => {
  expect(keysOfJump(withKey(DEFAULTS, slot(0, "jump", "keys", 0), "GamepadA"))).toEqual(["GamepadA"]);
  expect(keysOfJump(withKey(DEFAULTS, slot(0, "jump", "keys", 2), "Space"))).toEqual(["Space", "GamepadA"]);
  const input = make();
  expect(input.bind(slot(0, "jump", "keys", 2), "GamepadA")).toEqual([]);
  expect(input.slots(0, "jump")).toHaveLength(2);
});
const keysOfJump = (players: Bindings[]) => (players[0].jump as { keys: string[] }).keys;

test("有几个冲突时不对调，冲突的位置都去掉这个键", () => {
  // 默认的表自己就把 KeyZ 绑了两处
  const defaults: Bindings[] = [{
    jump: { kind: "button", keys: ["Space"] },
    a: { kind: "button", keys: ["KeyZ", "KeyQ"] },
    b: { kind: "axis", negative: ["KeyZ", "KeyE"], positive: ["KeyC"] },
  }];
  const input = inputMap({ key: "test", defaults, store: memory() });
  input.capture(slot(0, "jump", "keys", 0));
  input.handleKey(down("KeyZ"));
  expect(input.pending()?.conflicts).toHaveLength(2);
  expect(input.confirm()).toBe(true);
  expect(input.bindings()[0]).toEqual({
    jump: { kind: "button", keys: ["KeyZ"] },
    a: { kind: "button", keys: ["KeyQ"] },
    b: { kind: "axis", negative: ["KeyE"], positive: ["KeyC"] },
  });
});

test("对方只剩这一个键、又没有键能换给它：不能确认", () => {
  const defaults: Bindings[] = [{ jump: { kind: "button", keys: ["Space"] }, slot: { kind: "button", keys: ["KeyS"] } }];
  const input = inputMap({ key: "test", defaults, store: memory() });
  // 给跳加一个键（没有原来的键能换过去）
  input.capture(slot(0, "jump", "keys", 1));
  input.handleKey(down("KeyS"));
  expect(input.pending()).toEqual({ slot: slot(0, "jump", "keys", 1), code: "KeyS", conflicts: [slot(0, "slot", "keys", 0)], blocked: [slot(0, "slot", "keys", 0)] });
  expect(input.confirm()).toBe(false);
  expect(input.pending()).not.toBeNull();
  expect(input.bindings()).toEqual(defaults);
  input.cancel();
  // 换掉已有的键：对调，谁都不会空
  input.capture(slot(0, "jump", "keys", 0));
  input.handleKey(down("KeyS"));
  expect(input.pending()?.blocked).toEqual([]);
  expect(input.confirm()).toBe(true);
  expect(input.bindings()[0]).toEqual({ jump: { kind: "button", keys: ["KeyS"] }, slot: { kind: "button", keys: ["Space"] } });
});

test("去掉一个键：一个部分至少留一个", () => {
  const input = make();
  expect(input.unbind(slot(0, "jump", "keys", 0))).toBe(true);
  expect(input.slots(0, "jump").map((s) => input.key(s))).toEqual(["GamepadA"]);
  expect(input.unbind(slot(0, "jump", "keys", 0))).toBe(false);
  expect(input.slots(0, "jump")).toHaveLength(1);
  expect(input.unbind(slot(0, "jump", "keys", 5))).toBe(false);
  expect(input.unbind(slot(0, "nope", "keys", 0))).toBe(false);
});

test("绑上去的那个键松开时不漏给游戏，Escape 也一样", () => {
  const input = make();
  const up = (code: string) => ({ code, down: false, repeat: false });
  input.capture(slot(0, "jump", "keys", 0));
  expect(input.handleKey(down("KeyX"))).toBe(true);
  expect(input.handleKey({ code: "KeyX", down: true, repeat: true })).toBe(true);
  expect(input.handleKey(up("KeyX"))).toBe(true);
  expect(input.handleKey(up("KeyX"))).toBe(false);
  expect(input.handleKey(down("KeyX"))).toBe(false);
  input.capture(slot(0, "jump", "keys", 0));
  expect(input.handleKey(down("Escape"))).toBe(true);
  expect(input.handleKey(up("Escape"))).toBe(true);
  expect(input.handleKey(down("Escape"))).toBe(false);
});

test("等冲突的回答时 Escape 取消", () => {
  const input = make();
  input.capture(slot(0, "jump", "keys", 0));
  input.handleKey(down("KeyA"));
  expect(input.pending()).not.toBeNull();
  expect(input.handleKey(down("Escape"))).toBe(true);
  expect(input.pending()).toBeNull();
  expect(input.prop().capturing).toBe(false);
  expect(input.bindings()).toEqual(DEFAULTS);
  expect(input.handleKey({ code: "Escape", down: false, repeat: false })).toBe(true);
});

test("存的键要是认识的键名、不重复、不和别的动作冲突，不然那个动作用默认的", () => {
  expect(KEY_NAMES.has("Space") && KEY_NAMES.has("GamepadLeftStickLeft") && KEY_NAMES.has("MetaRight") && !KEY_NAMES.has("Numpad1")).toBe(true);
  const stored = (jump: unknown, move: unknown = P1.move) => bindingsOf({ v: 1, players: [{ jump, move }] }, DEFAULTS)![0];
  expect(stored({ kind: "button", keys: [""] }).jump).toEqual(P1.jump);
  expect(stored({ kind: "button", keys: ["KeyX", "Banana"] }).jump).toEqual(P1.jump);
  expect(stored({ kind: "button", keys: ["KeyX", "KeyX"] }).jump).toEqual(P1.jump);
  expect(stored({ kind: "button", keys: [] }).jump).toEqual(P1.jump);
  // 轴两边同一个键
  expect(stored(P1.jump, { kind: "axis", negative: ["KeyQ"], positive: ["KeyQ"] }).move).toEqual(P1.move);
  // 两个动作抢一个键：两个都回到默认
  const clash = stored({ kind: "button", keys: ["KeyQ"] }, { kind: "axis", negative: ["KeyQ"], positive: ["KeyE"] });
  expect(clash.jump).toEqual(P1.jump);
  expect(clash.move).toEqual(P1.move);
  // 存的键撞上了别的动作默认的键（那个动作没存）：存的这个不要
  expect(stored({ kind: "button", keys: ["KeyT"] }).jump).toEqual(P1.jump);
  // 第二个玩家存的键盘键撞了第一个玩家的
  const two = bindingsOf({ v: 1, players: [{}, { jump: { kind: "button", keys: ["Space"] } }] }, DEFAULTS)!;
  expect(two[1].jump).toEqual(P2.jump);
  expect(two[0]).toEqual(P1);
  // 合规的照用
  expect(stored({ kind: "button", keys: ["KeyX", "GamepadY"] }).jump).toEqual({ kind: "button", keys: ["KeyX", "GamepadY"] });
});

test("存储抛错接得住并报出来，下次存成了就清掉", async () => {
  const store = memory();
  const boom: InputStore = { getItem: store.getItem, setItem: () => { throw new Error("坏了"); } };
  const input = inputMap({ key: "test", defaults: DEFAULTS, store: boom });
  expect(input.saveError()).toBeNull();
  input.bind(slot(0, "jump", "keys", 0), "KeyX");
  await input.saved();
  expect(input.saveError()).toContain("坏了");
  expect(input.key(slot(0, "jump", "keys", 0))).toBe("KeyX");

  const full = memory();
  full.full = true;
  const again = make(full);
  again.bind(slot(0, "jump", "keys", 0), "KeyX");
  await again.saved();
  expect(again.saveError()).toContain("存储满了");
  full.full = false;
  again.bind(slot(0, "jump", "keys", 0), "KeyZ");
  await again.saved();
  expect(again.saveError()).toBeNull();
});

test("连着存：后存的为准，不会被先存的盖回去", async () => {
  const items = new Map<string, string>();
  let delay = 30;
  const slow: InputStore = {
    getItem: async () => null,
    setItem: (k, v) => new Promise((done) => setTimeout(() => (items.set(k, v), done()), (delay -= 25))),
  };
  const input = inputMap({ key: "test", defaults: DEFAULTS, store: slow });
  input.bind(slot(0, "jump", "keys", 0), "KeyX");
  input.bind(slot(0, "jump", "keys", 0), "KeyZ");
  await input.saved();
  expect(JSON.parse(items.get(KEY)!).players[0].jump.keys[0]).toBe("KeyZ");
  // 先存的那次慢（5 ms），等它也落地：没有盖回去
  await new Promise((r) => setTimeout(r, 30));
  expect(JSON.parse(items.get(KEY)!).players[0].jump.keys[0]).toBe("KeyZ");
});
