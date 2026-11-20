// 大地图（按 B 进出，桌面上）：2000 × 80 格的起伏地形，只画视野里的块；点地面挖一格、U 填回去，
// 只重建那一块的碰撞体；左上角开着性能面板和曲线（O 开关曲线）。见 docs/big-scene-perf.md。
import type { Scene, SpriteNode, Tap, TilemapNode } from "../../ts/nge";
import { TILESET } from "./catalog";
import { TILE } from "./levels";

/** 场景的 `level`：进出大地图都整个重建。 */
export const BIG_LEVEL = "bigmap";
export const BIG_MAP = "big";
export const BIG_COLS = 2000;
export const BIG_ROWS = 80;
/** 块的边长（格）。 */
export const BIG_CHUNK = 32;

const [STONE, GRASS, ONE_WAY, WATER] = [0, 1, 3, 10];

/** 第 c 列地面有几格高：几段正弦叠起来，左右两头是墙。 */
export function groundRows(c: number): number {
  if (c === 0 || c === BIG_COLS - 1) return BIG_ROWS;
  return Math.round(24 + 8 * Math.sin(c / 37) + 4 * Math.sin(c / 11) + 2 * Math.sin(c / 5));
}

/** 每 150 列一个两格宽的水坑（只是画面，掉进去站在坑底）。 */
const pit = (c: number) => c > 20 && c % 150 < 2;

function frameAt(c: number, r: number): number {
  const top = BIG_ROWS - groundRows(c);
  if (pit(c)) return r === BIG_ROWS - 1 ? STONE : r >= top ? WATER : -1;
  if (r === top) return GRASS;
  if (r > top) return STONE;
  // 每 40 列一块离地四格的单向板
  return c % 40 >= 10 && c % 40 < 14 && r === top - 4 ? ONE_WAY : -1;
}

let cached: number[] | undefined;
/** 整张地图的格子：只算一次，每次声明都是同一份。 */
export function bigData(): number[] {
  cached ??= Array.from({ length: BIG_COLS * BIG_ROWS }, (_, i) => frameAt(i % BIG_COLS, Math.floor(i / BIG_COLS)));
  return cached;
}

export const BIG_TILEMAP = (): TilemapNode => ({
  id: BIG_MAP,
  texture: TILESET.texture,
  grid: TILESET.grid,
  tile: TILE,
  cols: BIG_COLS,
  x: 0,
  y: (BIG_ROWS - 1) * TILE,
  data: bigData(),
  solid: TILESET.solid,
  oneWay: TILESET.oneWay,
  animated: TILESET.animated,
  chunk: BIG_CHUNK,
  tap: true,
  friction: 0,
});

/** 出生点：第 5 列地面上面。 */
export const BIG_SPAWN: [number, number] = [5 * TILE, groundRows(5) * TILE + 16];

/** 大地图的场景：地图、角色、跟着角色的相机、性能面板和曲线。 */
export function bigScene(player: SpriteNode, zoom: number): Scene {
  return {
    level: BIG_LEVEL,
    background: "#1b2233",
    physics: { gravity: [0, -1800] },
    camera: { follow: player.id, y: 60, smooth: 0.15, zoom, bounds: { left: 0, right: BIG_COLS * TILE, bottom: -160, top: BIG_ROWS * TILE } },
    perf: { panel: true, graph: true },
    tilemaps: [BIG_TILEMAP()],
    sprites: [{ ...player, x: BIG_SPAWN[0], y: BIG_SPAWN[1] }],
  };
}

/** 点中大地图的一格：有东西就挖掉，返回 `[列, 行, 原来的帧]`（填回去用）；点空格、点别的不算。 */
export function digOf(tap: Tap): [number, number, number] | null {
  if (tap.id !== BIG_MAP || tap.cell === undefined || tap.frame === undefined || tap.frame < 0) return null;
  return [tap.cell[0], tap.cell[1], tap.frame];
}
