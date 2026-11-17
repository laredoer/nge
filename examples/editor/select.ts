// 选择：点选、框选、切换（docs/editor.md）。包围盒由游戏的类型表给。
import type { Entity, LevelFile } from "../../ts/level";

export interface Box {
  left: number;
  right: number;
  bottom: number;
  top: number;
}
export type BoundsOf = (e: Entity) => Box;

/** 点中的实体：包围盒含这个点的里面画在最上面的（`depth` 最大的，一样大时列表靠后的）；`minHalf` 是最小可点的半宽（太小的东西也点得到）。 */
export function hitTest(entities: Entity[], boundsOf: BoundsOf, [x, y]: [number, number], minHalf = 0, depth: (e: Entity, index: number) => number = (_, i) => i): string | null {
  let best: { id: string; d: number } | null = null;
  for (let i = entities.length - 1; i >= 0; i--) {
    const b = boundsOf(entities[i]);
    const [cx, cy] = [(b.left + b.right) / 2, (b.bottom + b.top) / 2];
    const hw = Math.max((b.right - b.left) / 2, minHalf);
    const hh = Math.max((b.top - b.bottom) / 2, minHalf);
    if (Math.abs(x - cx) > hw || Math.abs(y - cy) > hh) continue;
    // 从后往前找：一样深时先找到的（列表里靠后的）留着
    const d = depth(entities[i], i);
    if (best === null || d > best.d) best = { id: entities[i].id, d };
  }
  return best?.id ?? null;
}

/** 框选：包围盒和 a、b 两个角围成的框相交的，按列表顺序。 */
export function boxSelect(entities: Entity[], boundsOf: BoundsOf, a: [number, number], b: [number, number]): string[] {
  const [left, right] = [Math.min(a[0], b[0]), Math.max(a[0], b[0])];
  const [bottom, top] = [Math.min(a[1], b[1]), Math.max(a[1], b[1])];
  return entities.filter((e) => {
    const r = boundsOf(e);
    return r.right >= left && r.left <= right && r.top >= bottom && r.bottom <= top;
  }).map((e) => e.id);
}

/** 选上或取消一个。 */
export function toggle(selection: string[], id: string): string[] {
  return selection.includes(id) ? selection.filter((s) => s !== id) : [...selection, id];
}

/** 去掉关卡里已经没有的（撤销了添加、删掉了）。 */
export function prune(selection: string[], level: LevelFile): string[] {
  const ids = new Set(level.entities.map((e) => e.id));
  return selection.filter((s) => ids.has(s));
}
