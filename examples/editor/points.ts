// 路线点（docs/editor-more.md）：走地怪巡逻的点，世界坐标。加、删、挪都返回新的点列表。
import type { Entity } from "../../ts/level";
import { MAX_POINTS } from "../../ts/level";

/** 加一个点：在最后一个点右边一格（没有点时在实体右边一格）；满了不加。 */
export function addPoint(e: Entity, step: number): [number, number][] {
  const points = e.points ?? [];
  if (points.length >= MAX_POINTS) return points;
  const [x, y] = points.at(-1) ?? [e.x, e.y];
  return [...points, [x + step, y]];
}

/** 删掉第 `i` 个；没有点了是 undefined。 */
export function removePoint(e: Entity, i: number): [number, number][] | undefined {
  const points = (e.points ?? []).filter((_, k) => k !== i);
  return points.length === 0 ? undefined : points;
}

/** 第 `i` 个挪到 (x, y)。 */
export function movePoint(e: Entity, i: number, x: number, y: number): [number, number][] {
  return (e.points ?? []).map((p, k) => (k === i ? [x, y] : p));
}

/** 世界坐标 `w` 点中了哪个路线点（`radius` 以内最近的）：只找 `ids` 这些实体的。 */
export function pointAt(entities: Entity[], ids: string[], [wx, wy]: [number, number], radius: number): { id: string; index: number } | null {
  let best: { id: string; index: number; d: number } | null = null;
  for (const e of entities) {
    if (!ids.includes(e.id)) continue;
    (e.points ?? []).forEach(([x, y], index) => {
      const d = Math.hypot(x - wx, y - wy);
      if (d <= radius && (best === null || d < best.d)) best = { id: e.id, index, d };
    });
  }
  return best === null ? null : { id: (best as { id: string }).id, index: (best as { index: number }).index };
}
