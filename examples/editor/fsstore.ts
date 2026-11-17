// 关卡目录（docs/editor.md）：真机上读写 levels/ 目录；没有文件权限时（无界面的测试里）退到应用存储。
import { readdir, readFile, writeFile } from "fs/promises";
import { Storage, type StorageModule } from "nsh";
import type { LevelStore } from "./files";

const DIR = "${pluginDir}/levels";
const PREFIX = "editor:level:";

/** levels/ 目录。 */
export const dirStore: LevelStore = {
  list: async () => (await readdir(DIR)).filter((n) => n.endsWith(".json")).sort(),
  read: (name) => readFile(`${DIR}/${name}`, "utf8"),
  write: (name, text) => writeFile(`${DIR}/${name}`, text),
};

/** 应用存储里的「目录」：键是 `editor:level:<文件名>`。 */
export function storageStore(storage: Pick<StorageModule, "getItem" | "setItem" | "keys"> = Storage): LevelStore {
  return {
    list: async () => (await storage.keys()).filter((k) => k.startsWith(PREFIX)).map((k) => k.slice(PREFIX.length)).sort(),
    read: async (name) => {
      const text = await storage.getItem(PREFIX + name);
      if (text === null) throw new Error(`没有 ${name}`);
      return text;
    },
    write: (name, text) => storage.setItem(PREFIX + name, text),
  };
}

export interface Picked {
  store: LevelStore;
  /** 存到哪了，状态栏上说。 */
  where: string;
}

/** 先试 levels/ 目录，列不出来就用应用存储。 */
export async function pickStore(dir: LevelStore = dirStore, fallback: () => LevelStore = storageStore): Promise<Picked> {
  try {
    await dir.list();
    return { store: dir, where: "levels/ 目录" };
  } catch {
    return { store: fallback(), where: "应用存储（没有 levels/ 目录的权限）" };
  }
}
