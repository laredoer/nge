// 关卡目录（fsstore.ts）：列不出目录就退到应用存储。
import { expect, test } from "nsh/test";
import { memoryStore } from "./files";
import { pickStore, storageStore } from "./fsstore";

const fakeStorage = () => {
  const data = new Map<string, string>([["other", "x"]]);
  return {
    getItem: async (k: string) => data.get(k) ?? null,
    setItem: async (k: string, v: string) => void data.set(k, v),
    keys: async () => [...data.keys()],
  };
};

test("应用存储：只列关卡的键，读写对得上，没有的读不出来", async () => {
  const store = storageStore(fakeStorage());
  await store.write("b.json", "B");
  await store.write("a.json", "A");
  expect(await store.list()).toEqual(["a.json", "b.json"]);
  expect(await store.read("a.json")).toBe("A");
  let failed = false;
  await store.read("c.json").catch(() => (failed = true));
  expect(failed).toBe(true);
});

test("目录能列就用目录，列不出来用应用存储", async () => {
  const dir = memoryStore();
  expect((await pickStore(dir)).store).toBe(dir);
  const broken = { ...dir, list: async () => Promise.reject(new Error("ERR_ACCESS_DENIED")) };
  const picked = await pickStore(broken, () => storageStore(fakeStorage()));
  expect(picked.store === broken).toBe(false);
  expect(picked.where.startsWith("应用存储")).toBe(true);
});
