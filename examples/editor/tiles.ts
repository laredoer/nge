// 瓦片刷（docs/editor.md）：世界坐标 → 格子，拖动时补齐两点之间的格子，一笔里的改动。
import type { TileLayer } from "../../ts/level";
import type { TileChange } from "./commands";

export interface Cell {
  r: number;
  c: number;
}

/** 世界坐标落在哪一格；地图外是 null。左上角在 (x, y)，y 朝上，行往下数。 */
export function cellAt(t: TileLayer, [wx, wy]: [number, number]): Cell | null {
  const c = Math.floor((wx - t.x) / t.tile);
  const r = Math.floor((t.y - wy) / t.tile);
  return r >= 0 && r < t.rows && c >= 0 && c < t.cols ? { r, c } : null;
}

/** 两格之间按直线经过的格子（含两头，Bresenham）。 */
export function cellsBetween(a: Cell, b: Cell): Cell[] {
  const out: Cell[] = [];
  const [dc, dr] = [Math.abs(b.c - a.c), -Math.abs(b.r - a.r)];
  const [sc, sr] = [a.c < b.c ? 1 : -1, a.r < b.r ? 1 : -1];
  let [c, r, err] = [a.c, a.r, dc + dr];
  for (;;) {
    out.push({ r, c });
    if (c === b.c && r === b.r) return out;
    const e2 = 2 * err;
    if (e2 >= dr) {
      err += dr;
      c += sc;
    }
    if (e2 <= dc) {
      err += dc;
      r += sr;
    }
  }
}

/** 一笔里的改动：每格只记一次，旧值是第一次碰到时的。 */
export interface Stroke {
  changes: TileChange[];
}

export const NEW_STROKE: Stroke = { changes: [] };

/** 往一笔里加格子：画成 `frame`（-1 是擦掉）；已经记过的、本来就是这个帧号的不记。 */
export function strokeAdd(stroke: Stroke, t: TileLayer, cells: Cell[], frame: number): Stroke {
  const seen = new Set(stroke.changes.map((c) => `${c.r},${c.c}`));
  const added: TileChange[] = [];
  for (const { r, c } of cells) {
    const key = `${r},${c}`;
    const before = t.data[r]?.[c];
    if (before === undefined || seen.has(key) || before === frame) continue;
    seen.add(key);
    added.push({ r, c, before, after: frame });
  }
  return added.length === 0 ? stroke : { changes: [...stroke.changes, ...added] };
}

/** 预览：画上这一笔的瓦片层（不改原来的）。 */
export function layerWith(t: TileLayer, stroke: Stroke): TileLayer {
  if (stroke.changes.length === 0) return t;
  const data = t.data.map((row) => [...row]);
  for (const ch of stroke.changes) data[ch.r][ch.c] = ch.after;
  return { ...t, data };
}
