// 平台跳跃存档数据的升级和补齐。
import { expect, test } from "nsh/test";
import { saveSlots, type SaveStore } from "../../ts/saves";
import { NEW_PROGRESS } from "./levels";
import { NEW_SAVE, nextSlot, SAVE_MIGRATIONS, SAVE_VERSION, saveDataOf, saveSummary, SLOTS } from "./savefile";

test("以前 saved() 存的进度升上来，关卡从第一关开始", async () => {
  const items = new Map([["nge:platformer", JSON.stringify({ unlocked: 2, best: { "1": 12 } })]]);
  const store: SaveStore = { getItem: async (k) => items.get(k) ?? null, setItem: async (k, v) => void items.set(k, v), removeItem: async (k) => void items.delete(k) };
  const slots = saveSlots({ key: "platformer", slots: SLOTS, version: SAVE_VERSION, migrations: SAVE_MIGRATIONS, parse: saveDataOf, summary: saveSummary, adopt: { key: "platformer", version: 1 }, store });
  await slots.loaded;
  expect(slots.latest()).toBe(0);
  expect(await slots.load(0)).toEqual({ status: "ok", data: { progress: { unlocked: 2, best: { "1": 12 } }, level: 0 }, savedAt: 0 });
});

test("数据补齐：关卡号不合法或没解锁回第一关", () => {
  expect(saveDataOf(null)).toEqual(NEW_SAVE);
  expect(saveDataOf({ progress: { unlocked: 2 }, level: 1 })).toEqual({ progress: { unlocked: 2, best: {} }, level: 1 });
  expect(saveDataOf({ progress: { unlocked: 2 }, level: 2 }).level).toBe(0);
  expect(saveDataOf({ progress: { unlocked: 2 }, level: -1 }).level).toBe(0);
  expect(saveDataOf({ progress: { unlocked: 2 }, level: 0.5 }).level).toBe(0);
  expect(saveDataOf({ level: 1 }).progress).toEqual(NEW_PROGRESS);
  expect(saveSummary({ progress: { unlocked: 3, best: {} }, level: 0 })).toEqual({ unlocked: 3 });
});

test("S 键按顺序换到下一个存档位，最后一个回到第一个", () => {
  expect([0, 1, 2].map(nextSlot)).toEqual([1, 2, 0]);
});
