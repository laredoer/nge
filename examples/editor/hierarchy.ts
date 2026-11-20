// 父子层级（docs/editor-more.md）：父节点只是层级，坐标都是世界坐标。场景树的行、子孙、能不能当父节点、拖到哪一行。
import type { Entity, LevelFile } from "../../ts/level";
import type { Command } from "./history";

/** 直接的孩子，按数组里的顺序。 */
export function childrenOf(level: LevelFile, id: string | undefined): Entity[] {
  const ids = new Set(level.entities.map((e) => e.id));
  // 父节点不在了的（不该有，保险）当最外层
  return level.entities.filter((e) => (e.parent !== undefined && ids.has(e.parent) ? e.parent : undefined) === id);
}

/** 所有子孙的 id（不含自己），一层层往下。 */
export function descendantsOf(level: LevelFile, id: string): string[] {
  const out: string[] = [];
  const seen = new Set([id]);
  const queue = [id];
  while (queue.length > 0) {
    const at = queue.shift()!;
    for (const e of level.entities) {
      if (e.parent === at && !seen.has(e.id)) {
        seen.add(e.id);
        out.push(e.id);
        queue.push(e.id);
      }
    }
  }
  return out;
}

/** 选中的加上它们的子孙（移动、删除、复制都带着），按数组里的顺序。 */
export function withDescendants(level: LevelFile, ids: string[]): string[] {
  const all = new Set(ids);
  for (const id of ids) for (const d of descendantsOf(level, id)) all.add(d);
  return level.entities.filter((e) => all.has(e.id)).map((e) => e.id);
}

/** `parent` 能不能当 `id` 的父节点：不能是自己、不能是自己的子孙；`undefined` 是放回最外层，总是行。 */
export function canParent(level: LevelFile, id: string, parent: string | undefined): boolean {
  if (parent === undefined) return true;
  if (parent === id || !level.entities.some((e) => e.id === parent)) return false;
  return !descendantsOf(level, id).includes(parent);
}

export interface TreeRow {
  id: string;
  depth: number;
  /** 有没有孩子（显示 ▸ / ▾）。 */
  parent: boolean;
  collapsed: boolean;
}

/** 场景树的行：最外层按数组顺序，每个后面跟着它的子孙（折起来的不展开）。 */
export function treeRows(level: LevelFile, collapsed: ReadonlySet<string>): TreeRow[] {
  const rows: TreeRow[] = [];
  const walk = (id: string | undefined, depth: number) => {
    for (const e of childrenOf(level, id)) {
      const kids = childrenOf(level, e.id).length > 0;
      const folded = kids && collapsed.has(e.id);
      rows.push({ id: e.id, depth, parent: kids, collapsed: folded });
      if (kids && !folded) walk(e.id, depth + 1);
    }
  };
  walk(undefined, 0);
  return rows;
}

/** 在场景树里按着第 `from` 行拖到 y（从列表顶上算）松开：放进哪一行（它的 id），拖到最后一行下面是 null（放回最外层），没挪出这一行是 undefined（不算拖）。 */
export function dropTarget(rows: TreeRow[], from: number, y: number, rowHeight: number): string | null | undefined {
  const at = Math.floor(y / rowHeight);
  if (at === from) return undefined;
  if (at >= rows.length) return null;
  return at < 0 ? undefined : rows[at].id;
}

/** 一组实体换父节点（`undefined` 回到最外层）：不合规的（会绕成圈）跳过；都没变返回 null。撤回还原。 */
export function setParent(level: LevelFile, ids: string[], parent: string | undefined): Command | null {
  // 选中的里面有父子关系时只挪最上面的那个，孩子跟着走
  const chosen = new Set(ids);
  const tops = ids.filter((id) => !ids.some((other) => other !== id && descendantsOf(level, other).includes(id)));
  const moving = level.entities.filter((e) => tops.includes(e.id) && chosen.has(e.id) && e.parent !== parent && canParent(level, e.id, parent));
  if (moving.length === 0) return null;
  const before = new Map(moving.map((e) => [e.id, e.parent]));
  const put = (pick: (id: string) => string | undefined) => (l: LevelFile) => ({
    ...l,
    entities: l.entities.map((e) => {
      if (!before.has(e.id)) return e;
      const p = pick(e.id);
      const { parent: _old, ...rest } = e;
      return p === undefined ? rest : { ...rest, parent: p };
    }),
  });
  return { label: parent === undefined ? "移出分组" : "放进分组", apply: put(() => parent), revert: put((id) => before.get(id)) };
}
