// 存档槽（见 docs/saves.md）：几个槽各存一份带版本号、保存时间和概要的数据；旧版本一级一级升级，坏档报出来不崩。
import { signal, Storage } from "nsh";

/** 存档用到的存储接口（nsh 的 `Storage` 就是）。 */
export interface SaveStore {
  getItem(key: string): Promise<string | null>;
  setItem(key: string, value: string): Promise<void>;
  removeItem(key: string): Promise<void>;
}

/** 列表里的一个槽：只读外面那层，不跑升级。 */
export type SlotInfo<S> =
  | { status: "empty" }
  | { status: "ok"; savedAt: number; version: number; summary: S | undefined }
  | { status: "broken"; reason: string; newer?: true };

/** `newer` 的坏档是比当前版本还新的游戏存的，不要覆盖。 */
export type LoadResult<T> = { status: "empty" } | { status: "ok"; data: T; savedAt: number } | { status: "broken"; reason: string; newer?: true };

export interface SaveSlotsOptions<T, S> {
  /** 游戏名：键是 `nge:save:<key>:<槽号>`。 */
  key: string;
  /** 几个槽。 */
  slots: number;
  /** 当前数据的版本号。 */
  version: number;
  /** `migrations[n]` 把第 n 版的数据升成第 n + 1 版（同步函数）。 */
  migrations?: Record<number, (data: unknown) => unknown>;
  /** 升到当前版本后补齐、校验成 `T`；抛异常算坏档。 */
  parse: (data: unknown) => T;
  /** 列表里显示的概要（存的时候算好一起存）。 */
  summary?: (data: T) => S;
  /** 第一个槽空着时，把 `saved(key)` 的旧存档当成第 `version` 版的数据接过来。 */
  adopt?: { key: string; version: number };
  store?: SaveStore;
  now?: () => number;
}

export interface SaveSlots<T, S> {
  /** 每个槽一项；读完之前是空数组。 */
  list(): SlotInfo<S>[];
  ready(): boolean;
  /** 列表读完时 resolve。 */
  loaded: Promise<void>;
  load(slot: number): Promise<LoadResult<T>>;
  save(slot: number, data: T): Promise<void>;
  remove(slot: number): Promise<void>;
  /** 最近存的槽号；没有就是 `null`。 */
  latest(): number | null;
}

/** 存档外面那层。 */
interface Envelope {
  v: number;
  at: number;
  summary?: unknown;
  data: unknown;
}

type Read = { status: "empty" } | { status: "ok"; envelope: Envelope } | { status: "broken"; reason: string; newer?: true };

/** 解析一个槽的原始字符串。 */
function envelopeOf(raw: string | null, version: number): Read {
  if (raw === null) return { status: "empty" };
  let e: Partial<Envelope>;
  try {
    e = JSON.parse(raw);
  } catch {
    return { status: "broken", reason: "不是合法的 JSON" };
  }
  if (typeof e !== "object" || e === null || !Number.isInteger(e.v) || typeof e.at !== "number" || !("data" in e)) {
    return { status: "broken", reason: "存档结构不对" };
  }
  return newer(e.v!, version) ?? { status: "ok", envelope: e as Envelope };
}

/** 比当前版本还新的存档：报成坏档，并标上 `newer`，游戏不该覆盖它（回到旧版本游戏时会丢掉新版本的进度）。 */
function newer(v: number, version: number): Read | undefined {
  return v > version ? { status: "broken", reason: `存档是更新的第 ${v} 版存的（现在是第 ${version} 版）`, newer: true } : undefined;
}

export function saveSlots<T, S = undefined>(options: SaveSlotsOptions<T, S>): SaveSlots<T, S> {
  const { key, slots, version, migrations = {}, parse, summary } = options;
  if (!Number.isInteger(slots) || slots < 1) throw new RangeError(`存档槽数要是正整数，现在是 ${slots}`);
  if (!Number.isInteger(version) || version < 1) throw new RangeError(`存档版本号要是正整数，现在是 ${version}`);
  if (options.adopt !== undefined && (!Number.isInteger(options.adopt.version) || options.adopt.version > version)) {
    throw new RangeError(`adopt.version 要是不大于 ${version} 的整数，现在是 ${options.adopt.version}`);
  }
  const store = options.store ?? Storage;
  const now = options.now ?? Date.now;
  const slotKey = (slot: number) => `nge:save:${key}:${slot}`;
  const adoptedKey = `nge:save:${key}:adopted`;
  /** 这次运行里已经写过「不再接旧档」的标记。 */
  let adopted = false;
  const list = signal<SlotInfo<S>[]>([]);
  const ready = signal(false);

  // 所有读写排成一队，一个做完再做下一个：读档的几次读之间不会被存档插进来，读完列表之前存的也排在后面
  let queue: Promise<unknown> = Promise.resolve();
  const run = <R>(job: () => Promise<R>): Promise<R> => {
    const done = queue.then(job);
    queue = done.catch(() => undefined);
    return done;
  };

  const check = (slot: number) => {
    if (!Number.isInteger(slot) || slot < 0 || slot >= slots) throw new RangeError(`没有第 ${slot} 个存档槽（共 ${slots} 个，从 0 数）`);
  };
  /** 读一个槽；第一个槽空着时按 `adopt` 接旧存档。存储读出错算坏档。 */
  const read = async (slot: number): Promise<Read> => {
    try {
      const own = envelopeOf(await store.getItem(slotKey(slot)), version);
      if (own.status !== "empty" || slot !== 0 || options.adopt === undefined) return own;
      // 接过一次以后（存过、删过第一个槽）就不再接：删掉的档不会下次启动又冒出来
      if ((await store.getItem(adoptedKey)) !== null) return own;
      const old = await store.getItem(`nge:${options.adopt.key}`);
      if (old === null) return own;
      try {
        return { status: "ok", envelope: { v: options.adopt.version, at: 0, data: JSON.parse(old) } };
      } catch {
        return { status: "broken", reason: "旧存档不是合法的 JSON" };
      }
    } catch (e) {
      return { status: "broken", reason: `读不出来: ${e}` };
    }
  };
  const infoOf = (r: Read): SlotInfo<S> =>
    r.status === "ok" ? { status: "ok", savedAt: r.envelope.at, version: r.envelope.v, summary: r.envelope.summary as S | undefined } : r;
  const setInfo = (slot: number, info: SlotInfo<S>) => list.set(list().map((s, i) => (i === slot ? info : s)));

  const loaded = run(async () => {
    const all: Read[] = [];
    for (let i = 0; i < slots; i++) all.push(await read(i));
    list.set(all.map(infoOf));
    ready.set(true);
  });

  return {
    list: () => list(),
    ready: () => ready(),
    loaded,
    async load(slot) {
      check(slot);
      return run(async () => {
        const r = await read(slot);
        if (r.status !== "ok") return r;
        let data = r.envelope.data;
        for (let v = r.envelope.v; v < version; v++) {
          const up = migrations[v];
          if (up === undefined) return { status: "broken", reason: `没有从第 ${v} 版升级的函数` };
          try {
            data = up(data);
          } catch (e) {
            return { status: "broken", reason: `从第 ${v} 版升级失败: ${e}` };
          }
        }
        try {
          return { status: "ok", data: parse(data), savedAt: r.envelope.at };
        } catch (e) {
          return { status: "broken", reason: `数据不对: ${e}` };
        }
      });
    },
    async save(slot, data) {
      check(slot);
      return run(async () => {
        const at = now();
        const json = JSON.stringify({ v: version, at, summary: summary?.(data), data });
        // 先按读档的规矩检查一遍：data 是 undefined、时间不是有限数时 JSON 里会丢掉或变成 null，下次读就是坏档
        const back = envelopeOf(json ?? null, version);
        if (back.status !== "ok") throw new Error(`存档存不成：数据要能写成 JSON、时间要是有限数（data ${typeof data}，时间 ${at}）`);
        await store.setItem(slotKey(slot), json);
        // 列表按写进去的样子更新，和下次启动读出来的一致
        setInfo(slot, infoOf(back));
        if (slot === 0 && options.adopt !== undefined && !adopted) {
          // 槽里有档时本来就不接旧档；标记写不上也不算存档失败
          await store.setItem(adoptedKey, "1").then(
            () => void (adopted = true),
            (e) => console.warn(`写存档标记失败: ${e}`),
          );
        }
      });
    },
    async remove(slot) {
      check(slot);
      return run(async () => {
        // 先写「不再接旧档」的标记再删：标记写不上就不删，免得删了旧档下次又冒出来
        if (slot === 0 && options.adopt !== undefined && !adopted) {
          await store.setItem(adoptedKey, "1");
          adopted = true;
        }
        await store.removeItem(slotKey(slot));
        setInfo(slot, { status: "empty" });
      });
    },
    latest() {
      let best: number | null = null;
      list().forEach((s, i) => {
        if (s.status === "ok" && (best === null || s.savedAt > (list()[best] as { savedAt: number }).savedAt)) best = i;
      });
      return best;
    },
  };
}
