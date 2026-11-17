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

/** 认识的键名：键盘的（Web `KeyboardEvent.code` 里引擎转发的那些）和手柄的。存的绑定里别的名字不要。 */
export const KEY_NAMES: ReadonlySet<string> = new Set([
  ..."ABCDEFGHIJKLMNOPQRSTUVWXYZ".split("").map((c) => `Key${c}`),
  ..."0123456789".split("").map((d) => `Digit${d}`),
  ...Array.from({ length: 12 }, (_, i) => `F${i + 1}`),
  ...["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Space", "Enter", "Tab", "Escape", "Backspace", "Delete", "Home", "End", "PageUp", "PageDown"],
  ...["Minus", "Equal", "BracketLeft", "BracketRight", "Backslash", "Semicolon", "Quote", "Backquote", "Comma", "Period", "Slash"],
  ...["ShiftLeft", "ShiftRight", "ControlLeft", "ControlRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"],
  ...["A", "B", "X", "Y", "L1", "R1", "L2", "R2", "L3", "R3", "Menu", "Options", "Up", "Down", "Left", "Right"].map((b) => `Gamepad${b}`),
  ...["Left", "Right"].flatMap((stick) => ["Up", "Down", "Left", "Right"].map((d) => `Gamepad${stick}Stick${d}`)),
]);

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

/** 把 `slot` 换成 `code`（位置在末尾之后是加一个，`null` 是去掉），返回新的表；同一部分里不留重复的键。没有这个动作、这个部分时原样返回。 */
export function withKey(players: Bindings[], slot: Slot, code: string | null): Bindings[] {
  const binding = players[slot.player]?.[slot.action];
  const keys = keysOf(binding, slot.part);
  if (binding === undefined || keys === undefined) return players;
  const put = code === null ? keys.filter((_, i) => i !== slot.index) : slot.index < keys.length ? keys.map((k, i) => (i === slot.index ? code : k)) : [...keys, code];
  const next = put.filter((k, i) => put.indexOf(k) === i);
  return players.map((p, i) => (i === slot.player ? { ...p, [slot.action]: { ...binding, [slot.part]: next } as Binding } : p));
}

/**
 * `code` 绑到 `slot` 上会和哪些位置撞：键盘的键查所有玩家（键盘是共用的），手柄的键只查同一个玩家。
 * 同一个动作的另一部分（轴的另一边）也算；同一部分里的不算（同一部分不留重复的键，见 `withKey`）。
 */
export function conflictsOf(players: Bindings[], slot: Slot, code: string): Slot[] {
  const pad = code.startsWith("Gamepad");
  return players.flatMap((bindings, player) => {
    if (pad && player !== slot.player) return [];
    return Object.keys(bindings).flatMap((action) =>
      slotsOf(players, player, action).filter((s) => keyAt(players, s) === code && !(player === slot.player && action === slot.action && s.part === slot.part)),
    );
  });
}

/** 冲突的位置里去不掉键的：`replacement` 是能换给它的键，没有（`undefined`）时只能去掉，它那一部分只剩这一个键就去不掉。 */
function stuck(players: Bindings[], conflicts: Slot[], replacement: string | undefined): Slot[] {
  if (conflicts.length === 1 && replacement !== undefined) return [];
  return conflicts.filter((c) => keysOf(players[c.player]?.[c.action], c.part)?.length === 1);
}

/** 一部分的键：都是认识的键名，至少一个。 */
const names = (v: unknown): v is string[] => Array.isArray(v) && v.length > 0 && v.every((k) => typeof k === "string" && KEY_NAMES.has(k));

/** 存的一个绑定按默认的对一遍：种类一样、每部分都是认识的键名、整个动作里没有重复的键才用存的键，模拟量总是默认的。 */
function bindingOf(raw: unknown, initial: Binding): Binding {
  const o = (typeof raw === "object" && raw !== null ? raw : {}) as Record<string, unknown>;
  const parts = PARTS[initial.kind];
  if (o.kind !== initial.kind || !parts.every((part) => names(o[part]))) return initial;
  const all = parts.flatMap((part) => o[part] as string[]);
  if (new Set(all).size !== all.length) return initial;
  return { ...initial, ...Object.fromEntries(parts.map((part) => [part, o[part]])) } as Binding;
}

/** 存的动作里有键撞了别的动作的：这个动作回到默认的。回到默认后又撞上别的存的动作就接着退，直到不撞（默认的表自己撞的不管）。 */
function withoutClashes(players: Bindings[], defaults: Bindings[]): Bindings[] {
  const clashing = (table: Bindings[]) =>
    table.flatMap((bindings, player) =>
      Object.keys(bindings)
        .filter((action) => bindings[action] !== defaults[player][action])
        .filter((action) => slotsOf(table, player, action).some((s) => conflictsOf(table, s, keyAt(table, s)!).length > 0))
        .map((action) => ({ player, action })),
    );
  let table = players;
  for (let bad = clashing(table); bad.length > 0; bad = clashing(table)) {
    table = table.map((bindings, player) => ({ ...bindings, ...Object.fromEntries(bad.filter((b) => b.player === player).map((b) => [b.action, defaults[player][b.action]])) }));
  }
  return table;
}

/** 读出来的存档（`{ v: 1, players }`）按默认的表对一遍：默认里没有的动作丢掉，少了的用默认的；结构、版本不对的是 `null`。 */
export function bindingsOf(raw: unknown, defaults: Bindings[]): Bindings[] | null {
  const o = raw as { v?: unknown; players?: unknown } | null;
  if (typeof o !== "object" || o === null || o.v !== VERSION || !Array.isArray(o.players)) return null;
  const stored = o.players as unknown[];
  const read = defaults.map((bindings, player) => {
    const saved = (typeof stored[player] === "object" && stored[player] !== null ? stored[player] : {}) as Record<string, unknown>;
    return Object.fromEntries(Object.entries(bindings).map(([action, initial]) => [action, bindingOf(saved[action], initial)]));
  });
  return withoutClashes(read, defaults);
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

/** 等玩家回答的冲突：要把 `code` 绑到 `slot`，它已经用在 `conflicts` 上了。`blocked` 不为空时不能确认：那些位置是它们那一部分唯一的键，又没有键能换过去。 */
export interface PendingBind {
  slot: Slot;
  code: string;
  conflicts: Slot[];
  blocked: Slot[];
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
  /** 去掉这个键；它是那一部分唯一的键时不去（每部分至少留一个），返回 false。 */
  unbind(slot: Slot): boolean;
  /** 等下一个按下的键（`handleKey` 收到的）绑到 `slot`；Escape 取消。 */
  capture(slot: Slot): void;
  capturing(): Slot | null;
  /** 按下的键有冲突，等 `confirm()` 或 `cancel()`（Escape 也是取消）。 */
  pending(): PendingBind | null;
  /** 只有一个冲突时对调（冲突的位置换成这个位置原来的键）；有几个冲突、或者这个位置是新加的，冲突的位置去掉这个键。`blocked` 不为空时不做，返回 false。 */
  confirm(): boolean;
  cancel(): void;
  /** 最近一次存键位失败的原因；存成了是 `null`。 */
  saveError(): string | null;
  /** 排着的存盘都做完时 resolve（不会 reject，失败看 `saveError()`）。 */
  saved(): Promise<void>;
  /** 恢复默认。 */
  reset(): void;
  /** 游戏视图的 `onKey` 交给它；返回 true 时这次按键被拿去绑了（包括绑上去的那个键、Escape 之后的重复和松开），游戏别再处理。 */
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
  const saveError = signal<string | null>(null);
  /** 读完之前就改过：以改的为准。 */
  let changed = false;
  /** 存盘排成一队：先改的先存，后存的不会被先存的盖回去。 */
  let writes: Promise<void> = Promise.resolve();
  /** 拿去绑了的键：松开之前它的重复和松开都不给游戏。 */
  const swallowed = new Set<string>();

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
    const text = JSON.stringify({ v: VERSION, players: next });
    writes = writes
      // 放在 then 里：setItem 直接抛错也接得住
      .then(() => store.setItem(storageKey, text))
      .then(
        () => saveError.set(null),
        (e) => {
          console.warn(`存键位 ${storageKey} 失败: ${e}`);
          saveError.set(String(e));
        },
      );
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
    unbind(slot) {
      const keys = keysOf(bindings()[slot.player]?.[slot.action], slot.part);
      if (keys === undefined || slot.index >= keys.length || keys.length <= 1) return false;
      commit(withKey(bindings(), slot, null));
      return true;
    },
    capture(slot) {
      pending.set(null);
      capturing.set(slot);
    },
    capturing,
    pending,
    confirm() {
      const p = pending();
      if (p === null || p.blocked.length > 0) return false;
      // 只有一个冲突：它换成这个位置原来的键（对调）。有几个冲突、或者这个位置是新加的：冲突的位置去掉这个键。从后往前去，前面的下标不变
      const before = p.conflicts.length === 1 ? keyAt(bindings(), p.slot) : undefined;
      const order = [...p.conflicts].sort((a, b) => b.index - a.index);
      const freed = order.reduce((players, c) => withKey(players, c, before ?? null), bindings());
      pending.set(null);
      commit(withKey(freed, p.slot, p.code));
      return true;
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
      const fresh = e.down && !e.repeat;
      if (slot === null && pending() === null) {
        // 刚绑上去的键（或者取消用的 Escape）还按着：它的重复和松开不给游戏
        if (!swallowed.has(e.code) || fresh) return false;
        if (!e.down) swallowed.delete(e.code);
        return true;
      }
      if (!fresh) return true;
      swallowed.add(e.code);
      if (e.code === "Escape") {
        self.cancel();
        return true;
      }
      // 等冲突的回答时别的键不算
      if (slot === null) return true;
      capturing.set(null);
      const conflicts = self.bind(slot, e.code);
      if (conflicts.length > 0) {
        const replacement = keyAt(bindings(), slot);
        pending.set({ slot, code: e.code, conflicts, blocked: stuck(bindings(), conflicts, replacement) });
      }
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
    saveError,
    saved: () => writes,
    pads,
    padOf: (player) => pads().find((p) => p.player === player)?.id ?? null,
  };
  return self;
}
