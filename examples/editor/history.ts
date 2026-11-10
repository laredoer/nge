// 撤销 / 重做：命令模式（docs/editor.md）。命令和历史都不改传进来的东西，返回新的。
import type { LevelFile } from "../../ts/level";

/** 一步编辑：`apply` 做，`revert` 撤回，都返回新的关卡。`merge` 一样的相邻命令（同一批微移）可以合成一步。 */
export interface Command {
  label: string;
  apply(level: LevelFile): LevelFile;
  revert(level: LevelFile): LevelFile;
  merge?: string;
}

/** 历史里的一步：`serial` 每次执行（含合并）都是新的，保存时记下栈顶的，判断有没有改过。 */
export interface Step {
  cmd: Command;
  serial: number;
  at: number;
}

export interface History {
  done: Step[];
  undone: Step[];
  /** 保存时栈顶的 serial（空栈是 0）；那一步被丢掉后再也回不去，是 -1。 */
  savedAt: number;
  next: number;
  /** 超过上限丢掉的最后一步的 serial：撤销到底时的状态是它之后的。 */
  base: number;
}

export interface Editor {
  level: LevelFile;
  history: History;
}

export const HISTORY_LIMIT = 200;
/** 相邻两次同类命令隔多久以内合成一步（毫秒）。 */
export const MERGE_MS = 600;

export function newEditor(level: LevelFile): Editor {
  return { level, history: { done: [], undone: [], savedAt: 0, next: 1, base: 0 } };
}

const top = (h: History) => h.done.at(-1)?.serial ?? h.base;

/** 依次做完一组命令的合成命令，撤回时倒着撤。 */
export function chain(label: string, cmds: Command[]): Command {
  return {
    label,
    apply: (level) => cmds.reduce((l, c) => c.apply(l), level),
    revert: (level) => cmds.reduceRight((l, c) => c.revert(l), level),
  };
}

export function execute(ed: Editor, cmd: Command, now = Date.now()): Editor {
  const h = ed.history;
  const last = h.done.at(-1);
  const level = cmd.apply(ed.level);
  const merging = last !== undefined && cmd.merge !== undefined && last.cmd.merge === cmd.merge && now - last.at <= MERGE_MS;
  const step: Step = merging ? { cmd: { ...chain(last.cmd.label, [last.cmd, cmd]), merge: cmd.merge }, serial: h.next, at: now } : { cmd, serial: h.next, at: now };
  const kept = merging ? h.done.slice(0, -1) : h.done;
  const all = [...kept, step];
  const done = all.slice(-HISTORY_LIMIT);
  const dropped = all.length - done.length;
  const base = dropped > 0 ? all[dropped - 1].serial : h.base;
  // 保存的那一步在丢掉的重做栈里（或被合并掉了）：再也回不去
  const lost = h.undone.some((s) => s.serial === h.savedAt) || (merging && last!.serial === h.savedAt);
  return { level, history: { done, undone: [], savedAt: lost ? -1 : h.savedAt, next: h.next + 1, base } };
}

export function undo(ed: Editor): Editor {
  const h = ed.history;
  const last = h.done.at(-1);
  if (last === undefined) return ed;
  return { level: last.cmd.revert(ed.level), history: { ...h, done: h.done.slice(0, -1), undone: [...h.undone, last] } };
}

export function redo(ed: Editor): Editor {
  const h = ed.history;
  const next = h.undone.at(-1);
  if (next === undefined) return ed;
  return { level: next.cmd.apply(ed.level), history: { ...h, done: [...h.done, next], undone: h.undone.slice(0, -1) } };
}

export function markSaved(ed: Editor): Editor {
  return { ...ed, history: { ...ed.history, savedAt: top(ed.history) } };
}

export function isDirty(ed: Editor): boolean {
  return top(ed.history) !== ed.history.savedAt;
}
