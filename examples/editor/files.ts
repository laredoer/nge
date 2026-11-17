// 关卡文件的存取（docs/editor.md）：文件名校验，读写走 `LevelStore`（界面用 fs，测试用内存）。
import { formatLevel, parseLevel, type LevelFile, type ParseResult } from "../../ts/level";

export interface LevelStore {
  /** 目录里的 .json 文件名，排好序。 */
  list(): Promise<string[]>;
  read(name: string): Promise<string>;
  write(name: string, text: string): Promise<void>;
}

const NAME = /^[\p{L}\p{N}_-]{1,48}$/u;

/** 用户输入的文件名：去掉空白、补上 .json；带路径、空的、太长的、有怪字符的返回 null。 */
export function fileName(input: string): string | null {
  const stem = input.trim().replace(/\.json$/i, "");
  return NAME.test(stem) ? `${stem}.json` : null;
}

/** 内存里的「目录」（测试用）。 */
export function memoryStore(files: Record<string, string> = {}): LevelStore & { files: Record<string, string> } {
  const store = {
    files: { ...files },
    list: async () => Object.keys(store.files).filter((n) => n.endsWith(".json")).sort(),
    read: async (name: string) => {
      const text = store.files[name];
      if (text === undefined) throw new Error(`没有 ${name}`);
      return text;
    },
    write: async (name: string, text: string) => {
      store.files[name] = text;
    },
  };
  return store;
}

/** 打开：读不出来、格式不对都返回错误，不抛异常。 */
export async function openFrom(store: LevelStore, name: string): Promise<ParseResult> {
  let text: string;
  try {
    text = await store.read(name);
  } catch (e) {
    return { ok: false, errors: [`读不了 ${name}：${e instanceof Error ? e.message : String(e)}`] };
  }
  return parseLevel(text);
}

export async function saveTo(store: LevelStore, name: string, level: LevelFile): Promise<void> {
  await store.write(name, formatLevel(level));
}
