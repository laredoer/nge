// 平台跳跃的关卡数据，以及按数据生成地图、金币、移动平台、敌人。
import type { ModelNode, SpriteNode, TilemapNode, Tween } from "../../ts/nge";

export const TILE = 32;
export const ROWS = 15;
/** 图集：0 石块，1 长草的地面，2 木平台。 */
const [STONE, GRASS, PLANK] = [0, 1, 2];

export interface Level {
  name: string;
  cols: number;
  /** 坑：[起始列, 结束列]。 */
  pits: number[][];
  /** 平台：[行, 起始列, 结束列]，行 r 的顶边在 y = (14 - r) × 32。坑边上面留空，免得从地面起跳撞头。 */
  platforms: number[][];
  /** 台阶：[列, 最高的行]。 */
  stairs: number[][];
  /** 地面上放金币的列。 */
  groundCoins: number[];
  /** 移动平台：起点列、y，往哪走（列或 y）、来回一趟的秒数。 */
  lifts: { col: number; y: number; to: { col?: number; y?: number }; duration: number }[];
  /** 别的金币（台阶顶上、只有坐移动平台才够得着的）：列、y。 */
  extraCoins: number[][];
  /** 敌人：在地面上从这一列往右来回 3 格。巡逻线上方的平台要够高，踩它时不撞头。 */
  enemies: number[];
  /** 终点前转着的 3D 箱子在哪一列。 */
  crate: number;
}

export const LEVELS: Level[] = [
  {
    name: "草原",
    cols: 80,
    pits: [[18, 20], [41, 43], [60, 65]],
    platforms: [[11, 10, 13], [9, 16, 19], [11, 24, 27], [8, 30, 33], [10, 36, 39], [10, 56, 59]],
    stairs: [[48, 13], [49, 13], [50, 12], [51, 12], [52, 11], [53, 11]],
    groundCoins: [6, 23, 35, 46, 66, 70],
    // 横的那块在第三个坑上面来回；竖的那块在第二个坑里上下，把角色送到高处的金币（底下是坑，不会把人压进地里）
    lifts: [
      { col: 61, y: -8, to: { col: 65 }, duration: 2 },
      { col: 42.5, y: -8, to: { y: 152 }, duration: 2.5 },
    ],
    extraCoins: [[53.5, 124], [42.5, 200]],
    enemies: [14, 31, 69],
    crate: 73.5,
  },
  {
    name: "石阶",
    cols: 72,
    pits: [[10, 12], [26, 31], [44, 46], [55, 58]],
    platforms: [[11, 5, 7], [9, 14, 17], [8, 20, 23], [11, 33, 36], [9, 38, 41], [9, 48, 51], [10, 60, 62]],
    stairs: [[22, 13], [23, 12], [24, 11], [25, 11], [52, 13], [53, 12], [54, 12]],
    groundCoins: [4, 9, 19, 35, 43, 64, 67],
    // 第二个坑太宽，靠横着的平台过去；最后一个坑上面有个竖着的
    lifts: [
      { col: 27, y: -8, to: { col: 31 }, duration: 2.2 },
      { col: 56.5, y: -8, to: { y: 120 }, duration: 2 },
    ],
    extraCoins: [[29, 120], [56.5, 170]],
    enemies: [15, 37, 47],
    crate: 66.5,
  },
];

export interface Built {
  tilemap: TilemapNode;
  coins: { id: string; x: number; y: number }[];
  lifts: { node: SpriteNode; to: Tween["to"]; duration: number }[];
  enemies: { id: string; x: number; to: { x: number } }[];
  crate: ModelNode;
  goal: SpriteNode;
  /** 相机的范围右边。 */
  right: number;
}

/** 按关卡数据生成场景里这一关的东西。地面顶边在 y = 0，地图左边在 x = 0。 */
export function build(level: Level): Built {
  const { cols } = level;
  const tileAt = (c: number, r: number): number => {
    if (c === 0 || c === cols - 1) return STONE;
    if (r === ROWS - 1) return level.pits.some(([a, b]) => c >= a && c <= b) ? -1 : GRASS;
    if (level.stairs.some(([col, top]) => c === col && r >= top)) return STONE;
    return level.platforms.some(([row, a, b]) => r === row && c >= a && c <= b) ? PLANK : -1;
  };
  /** 某一列上方站着的高度（平台、台阶、地面里最高的那个顶边）。 */
  const standY = (c: number): number => {
    for (let r = 0; r < ROWS; r++) if (tileAt(c, r) >= 0) return (ROWS - 1 - r) * TILE;
    return 0;
  };
  const tilemap: TilemapNode = {
    id: "level",
    texture: "asset:/@platformer/tiles.png",
    grid: [3, 1],
    tile: TILE,
    cols,
    x: 0,
    y: (ROWS - 1) * TILE,
    data: Array.from({ length: cols * ROWS }, (_, i) => tileAt(i % cols, Math.floor(i / cols))),
    solid: [STONE, GRASS, PLANK],
    // 角色的速度归 Rust 管，地面不要摩擦
    friction: 0,
  };
  // 金币：平台上面一个，地面上和别的地方各几个
  const coins = [
    ...level.platforms.map(([, a, b], i) => ({ id: `coin-p${i}`, x: ((a + b + 1) / 2) * TILE, y: standY(Math.floor((a + b) / 2)) + 28 })),
    ...level.groundCoins.map((c, i) => ({ id: `coin-g${i}`, x: (c + 0.5) * TILE, y: standY(c) + 28 })),
    ...level.extraCoins.map(([c, y], i) => ({ id: `coin-x${i}`, x: c * TILE, y })),
  ];
  const lifts = level.lifts.map((l, i) => ({
    node: { id: `lift-${i}`, x: l.col * TILE, y: l.y, w: 64, h: 16 } as SpriteNode,
    to: l.to.col === undefined ? { y: l.to.y } : { x: l.to.col * TILE },
    duration: l.duration,
  }));
  // id 以 enemy- 开头（Rust 按这个认），kinematic
  const enemies = level.enemies.map((c, i) => ({ id: `enemy-${i}`, x: (c + 0.5) * TILE, to: { x: (c + 3.5) * TILE } }));
  // 终点前转着的 3D 箱子（2.5D）：3D 相机跟着 2D 相机（`match2d`），模型的 (x, y) 就是关卡里的位置
  const crate: ModelNode = { id: "crate", model: "asset:/@platformer/crate.glb", x: level.crate * TILE, y: 56, scale: 40, rotation: [20, 0, 0], spin: 60 };
  const goal: SpriteNode = { id: "goal", x: (cols - 4) * TILE, y: 40, w: 16, h: 80, color: "#6bcb77", body: { kind: "fixed", sensor: true, collide: true } };
  return { tilemap, coins, lifts, enemies, crate, goal, right: cols * TILE };
}

/** 存档：解锁到第几关（从 1 数），每关最好的用时（秒，键是关卡号）。 */
export interface Progress {
  unlocked: number;
  best: Record<string, number>;
}

export const NEW_PROGRESS: Progress = { unlocked: 1, best: {} };

/** 存档读出来的东西补齐成 `Progress`：以前的版本存的、结构不对的，缺什么按新存档算。 */
export function progressOf(stored: unknown): Progress {
  const s = (typeof stored === "object" && stored !== null ? stored : {}) as Partial<Progress>;
  const unlocked = Number.isInteger(s.unlocked) ? Math.min(LEVELS.length, Math.max(1, s.unlocked as number)) : 1;
  const best = typeof s.best === "object" && s.best !== null ? Object.fromEntries(Object.entries(s.best).filter(([, v]) => typeof v === "number")) : {};
  return { unlocked, best };
}

/** 第 `index` 关（从 0 数）用 `seconds` 秒通关后的存档：解锁下一关，用时更短时记下来。 */
export function finish(progress: Progress, index: number, seconds: number): Progress {
  const key = String(index + 1);
  const best = progress.best[key];
  return {
    unlocked: Math.min(LEVELS.length, Math.max(progress.unlocked, index + 2)),
    best: best !== undefined && best <= seconds ? progress.best : { ...progress.best, [key]: seconds },
  };
}
