// 网格吸附（docs/editor.md）：比格子大的边对齐格线，比格子小的放在格子中间。
export function snapValue(v: number, grid: number, size: number): number {
  if (!(grid > 0)) return v;
  if (size >= grid) return Math.round((v - size / 2) / grid) * grid + size / 2;
  return Math.round((v - grid / 2) / grid) * grid + grid / 2;
}

export function snapPoint([x, y]: [number, number], grid: number, [w, h]: [number, number]): [number, number] {
  return [snapValue(x, grid, w), snapValue(y, grid, h)];
}
