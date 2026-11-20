// 瓦片的矩形填充和油漆桶（docs/editor-more.md）：只算出要改的格子，提交用 `paintTiles`。
import type { TileLayer } from "../../ts/level";
import type { TileChange } from "./commands";
import type { Cell } from "./tiles";

/** 两格围成的矩形里的格子（含两头），一行一行。 */
export function rectCells(a: Cell, b: Cell): Cell[] {
  const out: Cell[] = [];
  for (let r = Math.min(a.r, b.r); r <= Math.max(a.r, b.r); r++) for (let c = Math.min(a.c, b.c); c <= Math.max(a.c, b.c); c++) out.push({ r, c });
  return out;
}

/** 把一组格子画成 `frame`：已经是这一帧的、在层外面的不记。 */
export function changesOf(t: TileLayer, cells: Cell[], frame: number): TileChange[] {
  return cells.flatMap(({ r, c }) => {
    const before = t.data[r]?.[c];
    return before === undefined || before === frame ? [] : [{ r, c, before, after: frame }];
  });
}

/** 油漆桶：从 `start` 开始四连通、和起点帧号一样的格子都换成 `frame`。用显式栈（格子的下标），大地图不爆栈。 */
export function floodFill(t: TileLayer, start: Cell, frame: number): TileChange[] {
  const from = t.data[start.r]?.[start.c];
  if (from === undefined || from === frame) return [];
  const { rows, cols } = t;
  const seen = new Uint8Array(rows * cols);
  const out: TileChange[] = [];
  const stack = [start.r * cols + start.c];
  seen[stack[0]] = 1;
  const visit = (r: number, c: number) => {
    const k = r * cols + c;
    if (seen[k] === 0 && t.data[r][c] === from) {
      seen[k] = 1;
      stack.push(k);
    }
  };
  while (stack.length > 0) {
    const k = stack.pop()!;
    const [r, c] = [Math.floor(k / cols), k % cols];
    out.push({ r, c, before: from, after: frame });
    if (r > 0) visit(r - 1, c);
    if (r < rows - 1) visit(r + 1, c);
    if (c > 0) visit(r, c - 1);
    if (c < cols - 1) visit(r, c + 1);
  }
  return out;
}
