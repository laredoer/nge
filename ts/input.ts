// 输入映射的 TSX 部分（见 docs/input-actions.md）：绑定表、改键（按任意键、冲突、恢复默认、存在设备上），
// 引擎发来的动作状态和手柄归谁。动作的状态是引擎算的（`action` 事件），这里只记下来。
import { signal, Storage } from "nsh";

export type AnalogAxis = "LeftStickX" | "LeftStickY" | "RightStickX" | "RightStickY" | "L2" | "R2";
export type AnalogStick = "LeftStick" | "RightStick";

/** 一个动作绑了什么。键名和 `KeyInput.code` 一样（键盘的 "Space"，手柄的 "GamepadA"、"GamepadLeftStickLeft"、"GamepadL2"）。 */
export type Binding =
  | { kind: "button"; keys: string[] }
  | { kind: "axis"; negative: string[]; positive: string[]; analog?: AnalogAxis[] }
  | { kind: "axis2d"; left: string[]; right: string[]; down: string[]; up: string[]; analog?: AnalogStick[] };

/** 一个玩家的表：动作名 → 绑定。 */
export type Bindings = Record<string, Binding>;

/** 游戏视图的 prop `actions`。`capturing` 时所有动作都算没按。 */
export interface ActionMap {
  players: Bindings[];
  capturing?: boolean;
}

/** 动作变了：按钮按下 / 松开，轴的方向变了（`x`、`y` 是 −1 / 0 / 1）。 */
export interface ActionEvent {
  action: string;
  player: number;
  down: boolean;
  x: number;
  y: number;
}

/** 手柄接入、拔出、换了玩家；`player` 是它归第几个玩家（从 0 数），没分给谁是 `null`。 */
export interface PadEvent {
  id: number;
  connected: boolean;
  player: number | null;
}

export type Part = "keys" | "negative" | "positive" | "left" | "right" | "down" | "up";

/** 绑定表里的一个键：第几个玩家的哪个动作的哪一部分的第几个。 */
export interface Slot {
  player: number;
  action: string;
  part: Part;
  index: number;
}

/** 每种绑定里放键的部分，按显示的顺序。 */
const PARTS: Record<Binding["kind"], Part[]> = { button: ["keys"], axis: ["negative", "positive"], axis2d: ["left", "right", "down", "up"] };

function keysOf(binding: Binding | undefined, part: Part): string[] | undefined {
  if (binding === undefined || !PARTS[binding.kind].includes(part)) return undefined;
  return (binding as unknown as Record<Part, string[]>)[part];
}

/** 一个动作的每个键的位置。 */
export function slotsOf(players: Bindings[], player: number, action: string): Slot[] {
  const binding = players[player]?.[action];
  if (binding === undefined) return [];
  return PARTS[binding.kind].flatMap((part) => keysOf(binding, part)!.map((_, index) => ({ player, action, part, index })));
}

/** 这个位置上的键；没有是 `undefined`。 */
export function keyAt(players: Bindings[], slot: Slot): string | undefined {
  return keysOf(players[slot.player]?.[slot.action], slot.part)?.[slot.index];
}

/** 把 `slot` 换成 `code`（位置在末尾之后是加一个，`null` 是去掉），返回新的表；没有这个动作、这个部分时原样返回。 */
export function withKey(players: Bindings[], slot: Slot, code: string | null): Bindings[] {
  const binding = players[slot.player]?.[slot.action];
  const keys = keysOf(binding, slot.part);
  if (binding === undefined || keys === undefined) return players;
  const next = code === null ? keys.filter((_, i) => i !== slot.index) : slot.index < keys.length ? keys.map((k, i) => (i === slot.index ? code : k)) : [...keys, code];
  return players.map((p, i) => (i === slot.player ? { ...p, [slot.action]: { ...binding, [slot.part]: next } as Binding } : p));
}

/** `code` 绑到 `slot` 上会和哪些位置撞：键盘的键查所有玩家（键盘是共用的），手柄的键只查同一个玩家；同一个动作里的不算。 */
export function conflictsOf(players: Bindings[], slot: Slot, code: string): Slot[] {
  const pad = code.startsWith("Gamepad");
  return players.flatMap((bindings, player) => {
    if (pad && player !== slot.player) return [];
    return Object.keys(bindings)
      .filter((action) => !(player === slot.player && action === slot.action))
      .flatMap((action) => slotsOf(players, player, action).filter((s) => keyAt(players, s) === code));
  });
}

const strings = (v: unknown): v is string[] => Array.isArray(v) && v.every((k) => typeof k === "string");

/** 存的一个绑定按默认的对一遍：种类一样、键都是字符串才用存的键，模拟量总是默认的。 */
function bindingOf(raw: unknown, initial: Binding): Binding {
  const o = (typeof raw === "object" && raw !== null ? raw : {}) as Record<string, unknown>;
  const parts = PARTS[initial.kind];
  if (o.kind !== initial.kind || !parts.every((part) => strings(o[part]))) return initial;
  return { ...initial, ...Object.fromEntries(parts.map((part) => [part, o[part]])) } as Binding;
}

/** 读出来的存档（`{ v: 1, players }`）按默认的表对一遍：默认里没有的动作丢掉，少了的用默认的；结构、版本不对的是 `null`。 */
export function bindingsOf(raw: unknown, defaults: Bindings[]): Bindings[] | null {
  const o = raw as { v?: unknown; players?: unknown } | null;
  if (typeof o !== "object" || o === null || o.v !== VERSION || !Array.isArray(o.players)) return null;
  const stored = o.players as unknown[];
  return defaults.map((bindings, player) => {
    const saved = (typeof stored[player] === "object" && stored[player] !== null ? stored[player] : {}) as Record<string, unknown>;
    return Object.fromEntries(Object.entries(bindings).map(([action, initial]) => [action, bindingOf(saved[action], initial)]));
  });
}

/** 存档的版本。 */
const VERSION = 1;

/** 用到的存储接口（nsh 的 `Storage` 就是）。 */
export interface InputStore {
  getItem(key: string): Promise<string | null>;
  setItem(key: string, value: string): Promise<void>;
}

export interface InputMapOptions {
  /** 游戏名：存在 `nge:input:<key>`。 */
  key: string;
  /** 默认的绑定，每个玩家一张表。 */
  defaults: Bindings[];
  store?: InputStore;
}

/** 等玩家回答的冲突：要把 `code` 绑到 `slot`，它已经用在 `conflicts` 上了。 */
export interface PendingBind {
  slot: Slot;
  code: string;
  conflicts: Slot[];
}

export interface InputMap {
  /** 现在的绑定表。 */
  bindings(): Bindings[];
  /** 存的绑定读完了。 */
  ready(): boolean;
  loaded: Promise<void>;
  /** 给游戏视图的 `actions`。 */
  prop(): ActionMap;
  slots(player: number, action: string): Slot[];
  key(slot: Slot): string | undefined;
  conflicts(slot: Slot, code: string): Slot[];
  /** 把 `slot` 换成 `code`；返回冲突的位置，不为空时没换。 */
  bind(slot: Slot, code: string): Slot[];
  /** 去掉这个键。 */
  unbind(slot: Slot): void;
  /** 等下一个按下的键（`handleKey` 收到的）绑到 `slot`；Escape 取消。 */
  capture(slot: Slot): void;
  capturing(): Slot | null;
  /** 按下的键有冲突，等 `confirm()`（对调）或 `cancel()`。 */
  pending(): PendingBind | null;
  confirm(): void;
  cancel(): void;
  /** 恢复默认。 */
  reset(): void;
  /** 游戏视图的 `onKey` 交给它；正在绑键时返回 true（这次按键被拿去绑了，游戏别再处理）。 */
  handleKey(e: { code: string; down: boolean; repeat: boolean }): boolean;
  /** 游戏视图的 `onAction` 交给它。 */
  handle(e: ActionEvent): void;
  /** 动作按着没有（轴：推过一半）。 */
  down(action: string, player?: number): boolean;
  /** 动作的方向 `[x, y]`，各是 −1 / 0 / 1（按钮按着时 x 是 1）。 */
  value(action: string, player?: number): [number, number];
  /** 游戏视图的 `onPad` 交给它。 */
  handlePad(e: PadEvent): void;
  /** 连着的手柄。 */
  pads(): PadEvent[];
  /** 第 `player` 个玩家的手柄号；没有是 `null`。 */
  padOf(player: number): number | null;
}

/** 绑定表、改键和动作状态（见 docs/input-actions.md）。 */
export function inputMap(options: InputMapOptions): InputMap {
  const store: InputStore = options.store ?? Storage;
  const storageKey = `nge:input:${options.key}`;
  const bindings = signal<Bindings[]>(options.defaults);
  const ready = signal(false);
  const capturing = signal<Slot | null>(null);
  const pending = signal<PendingBind | null>(null);
  const states = signal<Record<string, ActionEvent>>({});
  const pads = signal<PadEvent[]>([]);
  /** 读完之前就改过：以改的为准。 */
  let changed = false;

  const loaded = store.getItem(storageKey).then(
    (raw) => {
      if (raw !== null && !changed) {
        let parsed: unknown = null;
        try {
          parsed = JSON.parse(raw);
        } catch {
          // 下面按结构不对报
        }
        const read = bindingsOf(parsed, options.defaults);
        if (read === null) console.warn(`存的键位 ${storageKey} 结构不对，用默认的`);
        else bindings.set(read);
      }
      ready.set(true);
    },
    (e) => {
      console.warn(`读键位 ${storageKey} 失败: ${e}`);
      ready.set(true);
    },
  );

  const commit = (next: Bindings[]) => {
    changed = true;
    bindings.set(next);
    store.setItem(storageKey, JSON.stringify({ v: VERSION, players: next })).catch((e) => console.warn(`存键位 ${storageKey} 失败: ${e}`));
  };
  const stateKey = (action: string, player: number) => `${player}:${action}`;

  const self: InputMap = {
    bindings,
    ready,
    loaded,
    prop: () => ({ players: bindings(), capturing: capturing() !== null || pending() !== null }),
    slots: (player, action) => slotsOf(bindings(), player, action),
    key: (slot) => keyAt(bindings(), slot),
    conflicts: (slot, code) => conflictsOf(bindings(), slot, code),
    bind(slot, code) {
      const conflicts = conflictsOf(bindings(), slot, code);
      if (conflicts.length === 0) commit(withKey(bindings(), slot, code));
      return conflicts;
    },
    unbind: (slot) => commit(withKey(bindings(), slot, null)),
    capture(slot) {
      pending.set(null);
      capturing.set(slot);
    },
    capturing,
    pending,
    confirm() {
      const p = pending();
      if (p === null) return;
      // 冲突的位置换成这个位置原来的键（对调）；这个位置是新加的时，冲突的那个键去掉。从后往前去，前面的下标不变
      const before = keyAt(bindings(), p.slot);
      const order = [...p.conflicts].sort((a, b) => b.index - a.index);
      const freed = order.reduce((players, c) => withKey(players, c, before ?? null), bindings());
      pending.set(null);
      commit(withKey(freed, p.slot, p.code));
    },
    cancel() {
      pending.set(null);
      capturing.set(null);
    },
    reset() {
      pending.set(null);
      capturing.set(null);
      commit(options.defaults);
    },
    handleKey(e) {
      const slot = capturing();
      if (slot === null) return pending() !== null;
      if (!e.down || e.repeat) return true;
      capturing.set(null);
      if (e.code === "Escape") return true;
      const conflicts = self.bind(slot, e.code);
      if (conflicts.length > 0) pending.set({ slot, code: e.code, conflicts });
      return true;
    },
    handle: (e) => states.set({ ...states(), [stateKey(e.action, e.player)]: e }),
    down: (action, player = 0) => states()[stateKey(action, player)]?.down ?? false,
    value(action, player = 0) {
      const s = states()[stateKey(action, player)];
      return s === undefined ? [0, 0] : [s.x, s.y];
    },
    handlePad(e) {
      const others = pads().filter((p) => p.id !== e.id);
      pads.set(e.connected ? [...others, e].sort((a, b) => a.id - b.id) : others);
    },
    pads,
    padOf: (player) => pads().find((p) => p.player === player)?.id ?? null,
  };
  return self;
}
