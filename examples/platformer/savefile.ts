// 平台跳跃的存档（见 docs/saves.md）：三个存档位，各存进度和打到第几关。
import { NEW_PROGRESS, progressOf, type Progress } from "./levels";

export const SLOTS = 3;
/** 第 1 版是以前 `saved("platformer")` 存的进度，第 2 版加上打到第几关。 */
export const SAVE_VERSION = 2;

export interface SaveData {
  progress: Progress;
  /** 打到第几关（从 0 数）。 */
  level: number;
}

export const NEW_SAVE: SaveData = { progress: NEW_PROGRESS, level: 0 };

export const SAVE_MIGRATIONS: Record<number, (data: unknown) => unknown> = {
  1: (old) => ({ progress: old, level: 0 }),
};

/** 读出来的数据补齐：关卡号不合法或还没解锁时回到第一关。 */
export function saveDataOf(data: unknown): SaveData {
  const o = (typeof data === "object" && data !== null ? data : {}) as Partial<SaveData>;
  const progress = progressOf(o.progress);
  const level = Number.isInteger(o.level) && (o.level as number) >= 0 && (o.level as number) < progress.unlocked ? (o.level as number) : 0;
  return { progress, level };
}

/** 列表里显示的概要：解锁了几关。 */
export function saveSummary(data: SaveData): { unlocked: number } {
  return { unlocked: data.progress.unlocked };
}

export function nextSlot(slot: number): number {
  return (slot + 1) % SLOTS;
}
