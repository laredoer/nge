// 平台跳跃的关卡数据，以及按数据生成地图、金币、移动平台、敌人。
import type { JointNode, ModelNode, SpriteNode, TilemapNode, Tween } from "../../ts/nge";
import { tiled, type TiledMap } from "../../ts/tiled";
import level3 from "./maps/level3.json";

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
  /** 终点前的 3D 箱子在哪一列，能推着走。 */
  crate: number;
  /** 吊着的铁链：支点的列和 y。 */
  swing?: number[];
}

/** 用代码描述的关卡。 */
const DESCRIBED: Level[] = [
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
    swing: [7.5, 150],
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
    // 第一个坑上面
    swing: [11.5, 150],
  },
];

export interface Built {
  tilemaps: TilemapNode[];
  coins: { id: string; x: number; y: number }[];
  lifts: { node: SpriteNode; to: Tween["to"]; duration: number }[];
  enemies: { id: string; x: number; to: { x: number } }[];
  /** 3D 箱子：模型跟着一个看不见的 2D 刚体走（`attach`），角色能推着它走、跳上去。 */
  crate: { body: SpriteNode; model: ModelNode };
  /** 吊着的铁链：几节链子和一个铁球，用转动关节一节节连起来，挂在世界上的支点。 */
  swing: { sprites: SpriteNode[]; joints: JointNode[] };
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
  const lifts = level.lifts.map((l, i) => lift(`lift-${i}`, l.col * TILE, l.y, l.to.col === undefined ? { y: l.to.y } : { x: l.to.col * TILE }, l.duration));
  // id 以 enemy- 开头（Rust 按这个认），kinematic
  const enemies = level.enemies.map((c, i) => ({ id: `enemy-${i}`, x: (c + 0.5) * TILE, to: { x: (c + 3.5) * TILE } }));
  // 终点前转着的 3D 箱子（2.5D）：3D 相机跟着 2D 相机（`match2d`），模型的 (x, y) 就是关卡里的位置
  const swing = level.swing === undefined ? NO_SWING : swingAt(level.swing[0] * TILE, level.swing[1]);
  return { tilemaps: [tilemap], coins, lifts, enemies, crate: crateAt(level.crate * TILE, 56), swing, goal: goalAt((cols - 4) * TILE, 40), right: cols * TILE };
}

const lift = (id: string, x: number, y: number, to: Tween["to"], duration: number) => ({ node: { id, x, y, w: 64, h: 16 } as SpriteNode, to, duration });
/** 箱子边长 40（模型是边长 1 的立方体）。刚体不画（全透明），模型挂在它上面，往后仰 20° 看得出是 3D 的。 */
const crateAt = (x: number, y: number) => ({
  body: { id: "crate-body", x, y, w: 40, h: 40, z: 3, color: "#00000000", body: { density: 0.3, friction: 0.6 } } as SpriteNode,
  model: { id: "crate", model: "asset:/@platformer/crate.glb", attach: "crate-body", scale: 40, rotation: [20, 0, 0] } as ModelNode,
});

/** 链子几节、每节多长。 */
export const LINKS = 4;
const LINK = 18;
const NO_SWING = { sprites: [], joints: [] };

/** 支点在 (x, y) 的铁链：链子一节挨一节往下挂，最下面是半径 10 的铁球，一开始往右甩着。 */
function swingAt(x: number, y: number): Built["swing"] {
  const links = Array.from({ length: LINKS }, (_, i): SpriteNode => ({
    id: `swing-${i}`, x, y: y - LINK * (i + 0.5), w: 6, h: LINK, z: 1, color: "#8a8f9c", body: { density: 0.5 },
  }));
  const end = y - LINK * LINKS;
  const ball: SpriteNode = { id: "swing-ball", x, y: end - 10, w: 20, z: 1, color: "#4a4f5c", body: { shape: "circle", density: 2, velocity: [150, 0] } };
  // 每个连接点一个转动关节：第一节挂在世界上，后面的挂在上一节
  const joints = [...links, ball].map((s, i): JointNode => ({ id: `swing-j${i}`, a: i === 0 ? undefined : `swing-${i - 1}`, b: s.id, kind: "revolute", anchorA: [x, y - LINK * i] }));
  return { sprites: [...links, ball], joints };
}
const goalAt = (x: number, y: number): SpriteNode => ({ id: "goal", x, y, w: 16, h: 80, color: "#6bcb77", body: { kind: "fixed", sensor: true, collide: true } });

/**
 * 用 Tiled 画的关卡（见 docs/tiled.md）。图块集 `tiles` 和 tiles.png 一样；对象层里的对象按类型生成：
 * `coin`、`enemy`（属性 `patrol`：往右巡逻几格）、`lift`（属性 `dx` / `dy`：往哪走，像素，Tiled 里 y 朝下；`duration`）、`crate`、`swing`（铁链的支点）、`goal`。
 */
export function fromTiled(map: TiledMap): Built {
  // 地图底行的顶边在 y = 0，和代码描述的关卡一样
  const { tilemaps, objects, width } = tiled(map, { textures: { tiles: "asset:/@platformer/tiles.png" }, y: (map.height - 1) * map.tileheight });
  const of = (type: string) => objects.filter((o) => o.type === type);
  const num = (o: { properties: Record<string, unknown> }, name: string, fallback: number) => {
    const v = o.properties[name];
    return typeof v === "number" ? v : fallback;
  };
  const one = (type: string) => {
    const found = of(type)[0];
    if (found === undefined) throw new Error(`Tiled 关卡里没有 ${type}`);
    return found;
  };
  const crate = one("crate");
  const goal = one("goal");
  const swing = of("swing")[0];
  return {
    tilemaps,
    coins: of("coin").map((o, i) => ({ id: `coin-t${i}`, x: o.x, y: o.y })),
    lifts: of("lift").map((o, i) => {
      const [dx, dy] = [num(o, "dx", 0), num(o, "dy", 0)];
      if (dx !== 0 && dy !== 0) throw new Error(`Tiled 关卡的移动平台 ${o.id} 只能往一个方向走（dx 和 dy 只给一个）`);
      return lift(`lift-${i}`, o.x, o.y, dy === 0 ? { x: o.x + dx } : { y: o.y - dy }, num(o, "duration", 2));
    }),
    enemies: of("enemy").map((o, i) => ({ id: `enemy-${i}`, x: o.x, to: { x: o.x + num(o, "patrol", 3) * TILE } })),
    crate: crateAt(crate.x, crate.y),
    swing: swing === undefined ? NO_SWING : swingAt(swing.x, swing.y),
    goal: goalAt(goal.x, goal.y),
    right: width,
  };
}

export const LEVELS: { name: string; built: Built }[] = [
  ...DESCRIBED.map((level) => ({ name: level.name, built: build(level) })),
  { name: "云端", built: fromTiled(level3) },
];

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
