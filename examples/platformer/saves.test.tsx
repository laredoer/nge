// 存档槽（见 docs/saves.md）：用内存里的存储测，不碰设备上的存档。
import { expect, test } from "nsh/test";
import { saveSlots, type SaveStore } from "../../ts/saves";

/** 内存里的存储；`full` 时写入失败。 */
function memory(init: Record<string, string> = {}) {
  const items = new Map(Object.entries(init));
  const store: SaveStore & { full: boolean; items: Map<string, string> } = {
    full: false,
    items,
    getItem: async (k) => items.get(k) ?? null,
    setItem: async (k, v) => {
      if (store.full) throw new Error("存储满了");
      items.set(k, v);
    },
    removeItem: async (k) => {
      items.delete(k);
    },
  };
  return store;
}

interface Data {
  coins: number;
  name: string;
}

const parse = (d: unknown): Data => {
  const o = d as Partial<Data>;
  if (typeof o?.coins !== "number") throw new Error("coins 不是数");
  return { coins: o.coins, name: typeof o.name === "string" ? o.name : "无名" };
};

const env = (v: number, at: number, data: unknown, summary?: unknown) => JSON.stringify({ v, at, summary, data });

function make(store: SaveStore, extra: object = {}) {
  let clock = 1000;
  return saveSlots<Data, string>({
    key: "g",
    slots: 3,
    version: 2,
    migrations: { 1: (old) => ({ coins: (old as { gold: number }).gold, name: "升级来的" }) },
    parse,
    summary: (d) => `${d.coins} 金币`,
    store,
    now: () => (clock += 10),
    ...extra,
  });
}

/** 等列表读完。 */
async function ready(slots: { ready: () => boolean }) {
  for (let i = 0; i < 100 && !slots.ready(); i++) await Promise.resolve();
  expect(slots.ready()).toBe(true);
}

test("列出空的、有档的、坏的槽；读完之前 ready 是假", async () => {
  const store = memory({ "nge:save:g:0": env(2, 500, { coins: 3 }, "3 金币"), "nge:save:g:1": "{坏", "nge:save:g:2": env(3, 9, {}) });
  const slots = make(store);
  expect(slots.ready()).toBe(false);
  expect(slots.list()).toEqual([]);
  await slots.loaded;
  expect(slots.ready()).toBe(true);
  const [a, b, c] = slots.list();
  expect(a).toEqual({ status: "ok", savedAt: 500, version: 2, summary: "3 金币" });
  expect(b.status).toBe("broken");
  expect(c.status).toBe("broken");
  expect(c.status === "broken" && c.reason.includes("新")).toBe(true);
  const empty = make(memory({ "nge:save:g:0": JSON.stringify({ at: 1 }) }));
  await ready(empty);
  expect(empty.list().map((s) => s.status)).toEqual(["broken", "empty", "empty"]);
});

test("存了再读，列表跟着变，latest 找最近存的，删了变空", async () => {
  const store = memory();
  const slots = make(store);
  await ready(slots);
  expect(slots.latest()).toBe(null);
  await slots.save(1, { coins: 5, name: "a" });
  await slots.save(2, { coins: 7, name: "b" });
  expect(slots.list()[1]).toEqual({ status: "ok", savedAt: 1010, version: 2, summary: "5 金币" });
  expect(slots.latest()).toBe(2);
  await slots.save(1, { coins: 6, name: "a" });
  expect(slots.latest()).toBe(1);
  expect(await slots.load(1)).toEqual({ status: "ok", data: { coins: 6, name: "a" }, savedAt: 1030 });
  expect(JSON.parse(store.items.get("nge:save:g:1")!).v).toBe(2);
  expect(await slots.load(0)).toEqual({ status: "empty" });
  await slots.remove(1);
  expect(slots.list()[1]).toEqual({ status: "empty" });
  expect(await slots.load(1)).toEqual({ status: "empty" });
  expect(slots.latest()).toBe(2);
});

test("旧版本一级一级升上来；缺升级函数、数据不对、版本太新都是坏档", async () => {
  const store = memory({
    "nge:save:g:0": env(1, 5, { gold: 4 }),
    "nge:save:g:1": env(0, 5, { gold: 4 }),
    "nge:save:g:2": env(2, 5, { coins: "很多" }),
  });
  const slots = make(store);
  await ready(slots);
  expect(await slots.load(0)).toEqual({ status: "ok", data: { coins: 4, name: "升级来的" }, savedAt: 5 });
  const missing = await slots.load(1);
  expect(missing.status === "broken" && missing.reason.includes("0")).toBe(true);
  const bad = await slots.load(2);
  expect(bad.status === "broken" && bad.reason.includes("coins 不是数")).toBe(true);
  store.items.set("nge:save:g:2", env(5, 5, {}));
  expect((await slots.load(2)).status).toBe("broken");
  store.items.set("nge:save:g:2", "不是 JSON");
  expect((await slots.load(2)).status).toBe("broken");
  expect(store.items.get("nge:save:g:1")).toBe(env(0, 5, { gold: 4 }));
});

test("写失败时 reject，列表和原来的档都不变", async () => {
  const store = memory();
  const slots = make(store);
  await ready(slots);
  await slots.save(0, { coins: 1, name: "a" });
  store.full = true;
  let failed = false;
  await slots.save(0, { coins: 2, name: "a" }).catch(() => (failed = true));
  expect(failed).toBe(true);
  const first = slots.list()[0];
  expect(first.status === "ok" && first.summary).toBe("1 金币");
  expect(await slots.load(0)).toEqual({ status: "ok", data: { coins: 1, name: "a" }, savedAt: 1010 });
});

test("槽号不合法直接 reject", async () => {
  const slots = make(memory());
  await ready(slots);
  for (const slot of [-1, 3, 1.5, NaN]) {
    let failed = 0;
    await slots.load(slot).catch(() => failed++);
    await slots.save(slot, { coins: 1, name: "" }).catch(() => failed++);
    await slots.remove(slot).catch(() => failed++);
    expect(failed).toBe(3);
  }
});

test("adopt：第一个槽空着时接过 saved() 的旧存档，有档时不接", async () => {
  const old = { "nge:g": JSON.stringify({ gold: 9 }) };
  const store = memory(old);
  const slots = make(store, { adopt: { key: "g", version: 1 } });
  await ready(slots);
  expect(slots.list()[0]).toEqual({ status: "ok", savedAt: 0, version: 1, summary: undefined });
  expect(slots.latest()).toBe(0);
  expect(await slots.load(0)).toEqual({ status: "ok", data: { coins: 9, name: "升级来的" }, savedAt: 0 });
  await slots.save(0, { coins: 10, name: "x" });
  expect(store.items.has("nge:save:g:0")).toBe(true);
  expect(store.items.get("nge:g")).toBe(old["nge:g"]);
  expect(await slots.load(0)).toEqual({ status: "ok", data: { coins: 10, name: "x" }, savedAt: 1010 });

  await slots.remove(0);
  const again = make(store, { adopt: { key: "g", version: 1 } });
  await ready(again);
  expect(again.list()[0]).toEqual({ status: "empty" });

  const taken = make(memory({ ...old, "nge:save:g:0": env(2, 3, { coins: 1 }, "1 金币") }), { adopt: { key: "g", version: 1 } });
  await ready(taken);
  expect(await taken.load(0)).toEqual({ status: "ok", data: { coins: 1, name: "无名" }, savedAt: 3 });
  const broken = make(memory({ "nge:g": "{坏" }), { adopt: { key: "g", version: 1 } });
  await ready(broken);
  expect(broken.list()[0].status).toBe("broken");
});

test("列表读完之前存：排在读列表后面，列表是存进去的那份", async () => {
  const store = memory({ "nge:g": JSON.stringify({ gold: 9 }) });
  const slots = make(store, { adopt: { key: "g", version: 1 } });
  const saving = slots.save(0, { coins: 2, name: "新" });
  await slots.loaded;
  await saving;
  expect(slots.list()[0]).toEqual({ status: "ok", savedAt: 1010, version: 2, summary: "2 金币" });
  expect(await slots.load(0)).toEqual({ status: "ok", data: { coins: 2, name: "新" }, savedAt: 1010 });
});

test("存不成 JSON 的数据、不是有限数的时间直接 reject，不写坏档", async () => {
  const store = memory();
  // 不带 summary：summary 读 undefined 的字段会先抛，测不到这里
  const slots = make(store, { summary: undefined });
  await slots.loaded;
  let failed = 0;
  await slots.save(0, undefined as unknown as Data).catch(() => failed++);
  expect(failed).toBe(1);
  const nan = make(store, { now: () => NaN });
  await nan.loaded;
  await nan.save(1, { coins: 1, name: "" }).catch(() => failed++);
  expect(failed).toBe(2);
  expect(store.items.size).toBe(0);
  expect(slots.list()[0]).toEqual({ status: "empty" });
});

test("版本太新的坏档带 newer；存储读出错算坏档；参数不对直接抛", async () => {
  const store = memory({ "nge:save:g:0": env(3, 1, {}), "nge:save:g:1": "{坏" });
  const slots = make(store);
  await slots.loaded;
  const [a, b] = slots.list();
  expect(a.status === "broken" && a.newer).toBe(true);
  expect(b.status === "broken" && b.newer).toBe(undefined);
  const load = await slots.load(0);
  expect(load.status === "broken" && load.newer).toBe(true);
  store.getItem = async () => {
    throw new Error("坏了");
  };
  const failing = await slots.load(1);
  expect(failing.status === "broken" && failing.reason.includes("读不出来")).toBe(true);
  for (const bad of [{ slots: 0 }, { slots: 1.5 }, { version: 0 }, { adopt: { key: "g", version: 3 } }]) {
    let threw = false;
    try {
      make(memory(), bad);
    } catch {
      threw = true;
    }
    expect(threw).toBe(true);
  }
});

test("删第一个槽时标记写不上就不删；存档时标记写不上不算失败", async () => {
  const store = memory({ "nge:g": JSON.stringify({ gold: 9 }) });
  const slots = make(store, { adopt: { key: "g", version: 1 } });
  await slots.loaded;
  store.full = true;
  let failed = false;
  await slots.remove(0).catch(() => (failed = true));
  expect(failed).toBe(true);
  expect(slots.list()[0].status).toBe("ok");

  const other = memory();
  const saves = make(other, { adopt: { key: "g", version: 1 } });
  await saves.loaded;
  const setItem = other.setItem;
  other.setItem = async (k, v) => {
    if (k.endsWith(":adopted")) throw new Error("满了");
    return setItem(k, v);
  };
  await saves.save(0, { coins: 3, name: "" });
  expect(saves.list()[0].status).toBe("ok");
});
