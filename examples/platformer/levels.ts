// 平台跳跃的关卡数据，以及按数据生成地图、金币、移动平台、敌人。
import { bt, type AnimGraphDef, type BehaviorNode, type Emitter3dNode, type JointNode, type LightNode, type ModelNode, type PrefabInstance, type SpriteNode, type TilemapNode, type TrailNode, type Tween } from "../../ts/nge";
import { checkTypes, LEVEL_FORMAT, LEVEL_VERSION, parseLevel, toTilemap, type Entity, type LevelFile } from "../../ts/level";
import { tiled, type TiledMap } from "../../ts/tiled";
import { PREFAB_BOXES, propOf, rotationOf, sizeOf, TILESET, TYPES } from "./catalog";
import { TREE } from "./prefabs";
import heroAnim from "./hero-anim.json";
import level3 from "./maps/level3.json";
// 第二关在编辑器的目录里：编辑器只能读写自己目录下的 levels/（见 docs/editor.md）
import stairs from "../editor/levels/stairs.json";

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
  /** 幽灵：在空中的列和 y，看见角色就绕开平台追过来。 */
  ghosts?: number[][];
  /** 一摞 3D 箱子摆在第几列。 */
  tower?: number;
  /** 幽灵果冻蹲在第几列的地面上。 */
  jelly?: number;
  /** 斜坡：两头的 [x, y]（顶面），角色控制器能走上去（见 docs/character.md）。 */
  ramps?: number[][][];
}

/** 用代码描述的关卡（第二关「石阶」导出成了关卡文件，见 docs/editor.md）。 */
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
    enemies: [14, 44, 69],
    crate: 73.5,
    swing: [7.5, 150],
    // 从地面走上第三块平台的右边
    ramps: [[[28 * TILE, 3 * TILE], [28 * TILE + 3 * TILE * Math.sqrt(3), 0]]],
    // 第二组平台上方：从平台缝里看见地面上的角色
    ghosts: [[29, 240]],
    // 第一个坑后面的地面上：一摞 3D 箱子，角色跑过去撞散
    tower: 23,
    // 第三块平台右边、第二个坑前面的地面上
    jelly: 38,
  },
];

export interface Built {
  tilemaps: TilemapNode[];
  coins: { id: string; x: number; y: number }[];
  lifts: { node: SpriteNode; to: Tween["to"]; duration: number }[];
  enemies: { id: string; x: number; y: number; to: { x: number } }[];
  /** 幽灵：id 以 enemy- 开头（踩它、碰它和别的敌人一样），会飞，追角色。 */
  ghosts: { id: string; x: number; y: number }[];
  /** 走地面的敌人：行为树巡逻，看见角色就沿地面追（跳上平台、落下来），追丢了回去巡逻（见 docs/gameplay2d.md）。 */
  walkers: Walker[];
  /** 斜坡：转过角度的固定刚体。 */
  ramps: SpriteNode[];
  /** 一摞 3D 箱子摆在哪（x），没有是 null；见 `towerAt`。 */
  tower: number | null;
  /** 幽灵果冻在哪（x），没有是 null；见 `jellyAt`。 */
  jelly: number | null;
  /** 3D 箱子：模型跟着一个看不见的 2D 刚体走（`attach`），角色能推着它走、跳上去。 */
  crate: { body: SpriteNode; model: ModelNode };
  /** 吊着的铁链：几节链子和一个铁球，用转动关节一节节连起来，挂在世界上的支点。 */
  swing: { sprites: SpriteNode[]; joints: JointNode[] };
  goal: SpriteNode;
  /** 相机的范围右边、上边。 */
  right: number;
  top: number;
  /** 角色的起点。 */
  spawn: [number, number];
  /** 额外的 3D 灯（关卡文件里摆的）。 */
  lights: LightNode[];
  /** 摆着看的 3D 模型（关卡文件里摆的）。 */
  models: ModelNode[];
  /** 装饰：没有刚体的色块（关卡文件第 2 版的 `deco`）。 */
  decos: SpriteNode[];
  /** 预制体实例（关卡文件第 2 版的 `prefab`）。 */
  instances: PrefabInstance[];
}

/** 走地怪：出生点、往右巡逻几格；关卡文件里改过的才有路线点（有了就不看 `patrol`）、速度、跳跃、视野（格）、两头停几秒。 */
export interface Walker {
  id: string;
  x: number;
  y: number;
  patrol: number;
  route?: [number, number][];
  speed?: number;
  jump?: number;
  sight?: number;
  rest?: number;
}

/** 角色默认的起点。 */
export const SPAWN: [number, number] = [3 * TILE, 16];
/** 代码描述的关卡和 Tiled 关卡没有的东西。 */
const EXTRAS = { spawn: SPAWN, lights: [], models: [], decos: [], instances: [] };

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
    grid: [16, 1],
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
  const enemies = level.enemies.map((c, i) => ({ id: `enemy-${i}`, x: (c + 0.5) * TILE, y: ENEMY_Y, to: { x: (c + 3.5) * TILE } }));
  // 终点前转着的 3D 箱子（2.5D）：3D 相机跟着 2D 相机（`match2d`），模型的 (x, y) 就是关卡里的位置
  const swing = level.swing === undefined ? NO_SWING : swingAt(level.swing[0] * TILE, level.swing[1]);
  const ghosts = (level.ghosts ?? []).map(([c, y], i) => ({ id: `enemy-ghost-${i}`, x: (c + 0.5) * TILE, y }));
  const ramps = (level.ramps ?? []).map(([a, b], i) => rampOf(`ramp-${i}`, a, b));
  const tower = level.tower === undefined ? null : (level.tower + 0.5) * TILE;
  const jelly = level.jelly === undefined ? null : (level.jelly + 0.5) * TILE;
  return { tilemaps: [tilemap], coins, lifts, enemies, ghosts, walkers: [], ramps, tower, jelly, crate: crateAt(level.crate * TILE, 56), swing, goal: goalAt((cols - 4) * TILE, 40), right: cols * TILE, top: (ROWS - 1) * TILE, ...EXTRAS };
}

/** 装饰默认的 z：在瓦片（0）上面、金币（2）下面。 */
const DECO_Z = 0.5;
/** 物件库里预制体的名字 → 场景里的预制体（名字或文件）。 */
export const PREFAB_REFS: Record<string, string> = { grove: "grove", tree: TREE };

/** 地面上的敌人的 y（中心）。 */
const ENEMY_Y = 12;

/** 走地面的敌人：比角色慢，跳得比角色高一点（够得着三格高的平台），看得见 8 格。 */
export const WALKER = { speed: 110, jump: 720, sight: 8 * TILE };

/**
 * 走地面的敌人的行为树：看见角色（没墙挡着）就追，一次看见只发一次 `spotted`（黑板上记着，3 秒最多一次）；
 * 看不见了清掉记号，回到出生点，在出生点和右边 `patrol` 格之间来回走，两头各停一下。
 */
export function walkerBehavior(x: number, y: number, patrol: number, o: Omit<Walker, "id" | "x" | "y" | "patrol"> = {}): BehaviorNode {
  const rest = o.rest ?? 0.6;
  // 有路线点：出生点 → 每个点，走到最后一个再从头（每个点停一下）
  const ends: [number, number][] = o.route !== undefined && o.route.length > 0 ? [[x, y], ...o.route] : [[x, y], [x + patrol * TILE, y]];
  // 来回走：去的时候经过每个点，回来时原路返回（两头不重复）
  const stops = [...ends, ...ends.slice(1, -1).reverse()];
  return {
    speed: o.speed ?? WALKER.speed,
    jump: o.jump ?? WALKER.jump,
    tree: bt.selector(
      [
        bt.sequence([bt.see("player", o.sight === undefined ? WALKER.sight : o.sight * TILE), bt.selector([bt.check(SPOTTED), bt.sequence([bt.set(SPOTTED, true), bt.invert(bt.cooldown(3, bt.invert(bt.emit("spotted"))))])]), bt.chase("player")], true),
        bt.sequence([bt.set(SPOTTED, null), bt.repeat(bt.sequence(stops.flatMap(([px, py]) => [bt.goto(px, py), bt.wait(rest)])))]),
      ],
      true,
    ),
  };
}

/** 黑板上记「这次看见已经喊过了」的键。 */
const SPOTTED = "spotted";

/**
 * 走地面的敌人的精灵。id 带着第几轮（`round`，按 R 重开加一）：换了 id 引擎就重建，回到出生点、行为树从头跑。
 * `stomped` 里有的（踩到的，完整 id）变透明。
 */
export function walkerSprites(walkers: Built["walkers"], round: number, stomped: string[]): SpriteNode[] {
  return walkers.map(({ id, x, y, patrol, ...o }): SpriteNode => {
    const full = `${id}@${round}`;
    return stomped.includes(full)
      ? { id: full, x, y, w: 24, h: 24, z: 4, color: "#00000000" }
      : { id: full, x, y, w: 24, h: 24, z: 4, color: "#d35f5f", body: { kind: "character", friction: 0 }, behavior: walkerBehavior(x, y, patrol, o) };
  });
}

/** 厚 16 的斜坡，顶面从 `a` 到 `b`。 */
export function rampOf(id: string, [ax, ay]: number[], [bx, by]: number[]): SpriteNode {
  const [dx, dy] = [bx - ax, by - ay];
  const length = Math.hypot(dx, dy);
  // 顶面的中点往下（沿顶面的法线）挪半个厚度
  const [nx, ny] = [-dy / length, dx / length].map((v) => (dx >= 0 ? v : -v));
  return {
    id,
    x: (ax + bx) / 2 - nx * 8,
    y: (ay + by) / 2 - ny * 8,
    w: length,
    h: 16,
    z: 1,
    rotation: (Math.atan2(dy, dx) * 180) / Math.PI,
    color: "#8a6f4e",
    body: { kind: "fixed", friction: 0 },
  };
}

/** 移动平台：宽 `w`、高 16。 */
export function lift(id: string, x: number, y: number, to: Tween["to"], duration: number, w = 64) {
  return { node: { id, x, y, w, h: 16 } as SpriteNode, to, duration };
}
/** 箱子边长 40（模型是边长 1 的立方体）。刚体不画（全透明），模型挂在它上面，往后仰 20° 看得出是 3D 的。 */
export function crateAt(x: number, y: number) {
  return {
    body: { id: "crate-body", x, y, w: 40, h: 40, z: 3, color: "#00000000", body: { density: 0.3, friction: 0.6 } } as SpriteNode,
    model: { id: "crate", model: "asset:/@platformer/crate.glb", attach: "crate-body", scale: 40, rotation: [20, 0, 0] } as ModelNode,
  };
}

/** 一摞 3D 箱子的边长、离关卡平面多远（往后）。 */
export const TOWER_BOX = 14;
const TOWER_Z = -20;
/** 飞出去的箱子拖的尾巴：静止时相邻的点挨得近，尾巴缩回去。 */
const BOX_TRAIL: TrailNode = { lifetime: 0.35, width: [8, 0], color: ["#fff2a8cc", "#ffb34700"], minDistance: 2, blend: "add" };

/** 篝火的火苗序列帧：4 × 4 格（tools/make_flame.py 生成）。 */
export const FLAME_SHEET = "asset:/@platformer/flame.png";

/**
 * 塔右边一堆 3D 篝火，和箱子在同一个深度，箱子飞到前面时挡住它（3D 粒子补齐，见 docs/particles3d-more.md）：
 * - 火苗：叠加混合的序列帧，按寿命播完、随机起始帧、相邻帧混合，正着放（转角 0），先变大再收小；
 * - 火星：沿速度拉长，落到地面（y = 0）就死，落地处溅起一小撮更细的火花（碰撞触发的子发射器）。
 */
export function campfireAt(x: number): Emitter3dNode[] {
  const at = { x: x + 6 * TOWER_BOX, y: 4, z: TOWER_Z };
  const flames: Emitter3dNode = {
    id: "campfire",
    ...at,
    rate: 40,
    lifetime: [0.6, 1.0],
    speed: [12, 24],
    direction: [0, 1, 0],
    spread: 15,
    gravity: [0, 30, 0],
    angle: [0, 0],
    sizeCurve: [[0, 8], [0.3, 15], [1, 5]],
    color: ["#ffffffff", "#ff6a2000"],
    texture: FLAME_SHEET,
    sheet: { grid: [4, 4], randomStart: true, blend: true },
    blend: "add",
    max: 120,
  };
  const sparks: Emitter3dNode = {
    id: "campfire-sparks",
    ...at,
    y: at.y + 6,
    rate: 14,
    lifetime: [1.0, 1.6],
    speed: [60, 110],
    spread: 25,
    gravity: [0, -260, 0],
    drag: 0.4,
    size: [1.6, 1],
    color: ["#ffe08aff", "#ff602000"],
    facing: "velocity",
    stretch: 0.04,
    blend: "add",
    collision: { ground: 0, kill: true, radius: 0.5 },
    subEmitters: [
      {
        on: "collision",
        count: 5,
        emitter: { lifetime: [0.2, 0.4], speed: [20, 45], spread: 60, gravity: [0, -200, 0], size: [1.2, 0], color: ["#fff2b0ff", "#ff802000"], facing: "velocity", stretch: 0.03, blend: "add" },
      },
    ],
    max: 80,
  };
  return [flames, sparks];
}

/** 碰到它就把箱子撞散的 sensor 的 id。 */
export const TOWER_TRIGGER = "tower-trigger";

/**
 * 地面上 x 处一摞六个 3D 箱子（3D 物理，见 docs/physics3d.md），底下三个、中间两个、顶上一个，摆在关卡平面后面一点。
 * 3D 地面是一块看不见的固定刚体（模型缩到看不见，刚体大小单独写）。左边一点有一个看不见的 2D sensor，角色碰到它时游戏把箱子推散。
 * 箱子的 id 带着摞的次数（`round`）：换了就重建，回到原来摞着的样子。
 */
export function towerAt(x: number, round: number): { trigger: SpriteNode; models: ModelNode[] } {
  const s = TOWER_BOX;
  const boxes = [[-1, 0], [0, 0], [1, 0], [-0.5, 1], [0.5, 1], [0, 2]].map(([i, row], n): ModelNode => ({
    id: `tower-${round}-${n}`,
    model: "asset:/@platformer/crate.glb",
    // 箱子之间留 1 的缝，摆上去不互相挤
    x: x + i * (s + 1),
    y: s / 2 + row * s,
    z: TOWER_Z,
    scale: s,
    body: { density: 0.5 },
    trail: BOX_TRAIL,
  }));
  const floor: ModelNode = {
    id: "tower-floor",
    model: "asset:/@platformer/crate.glb",
    x,
    y: -10,
    z: TOWER_Z,
    scale: 0.001,
    body: { kind: "fixed", size: [12 * TILE, 20, 6 * TILE] },
  };
  return {
    trigger: { id: TOWER_TRIGGER, x: x - 3 * s, y: 32, w: 8, h: 64, color: "#00000000", body: { kind: "fixed", sensor: true } },
    models: [floor, ...boxes],
  };
}

/** 照着箱子堆的聚光灯：在左前上方，暖色，开影子（箱子在彼此身上投影）。亮度按距离平方衰减，所以按距离配强度。 */
export function towerSpot(x: number): LightNode {
  const from = [x - 4 * TILE, 7 * TILE, 140];
  const to = [x, TOWER_BOX * 1.5, TOWER_Z];
  const direction: [number, number, number] = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
  const d = Math.hypot(...direction);
  return { id: "tower-spot", kind: "spot", x: from[0], y: from[1], z: from[2], direction, angle: 18, color: "#ffd9a8", intensity: 2.5 * d * d, range: 2 * d, shadows: true };
}

/** 果冻感应区的 id：角色进去时果冻压一压，出来时停。 */
export const JELLY_ZONE = "jelly-zone";
/** 果冻在 idle 和 squish 之间淡多久（秒）。 */
export const JELLY_FADE = 0.25;

/** 果冻的动画状态机（见 docs/anim-graph.md）：`near` 时从 idle 淡到 squish，离开淡回去。 */
export const JELLY_ANIM: AnimGraphDef = {
  params: { near: false },
  start: "idle",
  states: {
    idle: { clip: "idle", to: [{ to: "squish", when: "near", fade: JELLY_FADE }] },
    squish: { clip: "squish", to: [{ to: "idle", when: "!near", fade: JELLY_FADE }] },
  },
};

/**
 * 主角的样子：精灵图集 hero.png（tools/make_hero.py 生成，8 × 2 格）和动画状态机 hero-anim.json（Rust 的角色控制也读这一份，设 speed、vy、grounded、jump）。
 * 走、跑是按 speed 的一维混合，脚步对得上；走跑落脚、落地时发 step / land 事件，Rust 放脚步声。
 */
export const HERO: Pick<SpriteNode, "texture" | "grid" | "animGraph"> = {
  texture: "asset:/@platformer/hero.png",
  grid: [8, 2],
  animGraph: heroAnim as AnimGraphDef,
};

/** 果冻离关卡平面多远：在角色（z = 5）和其他 z ≥ 0 的精灵后面。 */
export const JELLY_Z = 0;

/**
 * 地面上 x 处的幽灵果冻：半透明圆顶（宽 48、高约 29），在角色后面（`JELLY_Z`），透出后面的背景。
 * 半透明网格不写深度，混排时 z ≥ 0 的精灵在它后面画、会整只盖上去，所以放在这些精灵后面，盖上去也是对的。
 * 状态机 `JELLY_ANIM` 按 `near` 在 idle 和 squish 之间按 `JELLY_FADE` 淡过去。左右各 2 格的看不见的 2D sensor 管「靠近」。
 */
export function jellyAt(x: number, near: boolean): { zone: SpriteNode; model: ModelNode } {
  return {
    zone: { id: JELLY_ZONE, x, y: 32, w: 4 * TILE, h: 64, color: "#00000000", body: { kind: "fixed", sensor: true } },
    model: { id: "jelly", model: "asset:/@platformer/jelly.glb", x, y: 0, z: JELLY_Z, scale: 48, animGraph: JELLY_ANIM, animParams: { near } },
  };
}

/** 链子几节、每节多长。 */
export const LINKS = 4;
const LINK = 18;
const NO_SWING = { sprites: [], joints: [] };

/** 支点在 (x, y) 的铁链：链子一节挨一节往下挂，最下面是半径 10 的铁球，一开始往右甩着。 */
export function swingAt(x: number, y: number): Built["swing"] {
  const links = Array.from({ length: LINKS }, (_, i): SpriteNode => ({
    id: `swing-${i}`, x, y: y - LINK * (i + 0.5), w: 6, h: LINK, z: 1, color: "#8a8f9c", body: { density: 0.5 },
  }));
  const end = y - LINK * LINKS;
  const ball: SpriteNode = { id: "swing-ball", x, y: end - 10, w: 20, z: 1, color: "#4a4f5c", body: { shape: "circle", density: 2, velocity: [150, 0] } };
  // 每个连接点一个转动关节：第一节挂在世界上，后面的挂在上一节
  const joints = [...links, ball].map((s, i): JointNode => ({ id: `swing-j${i}`, a: i === 0 ? undefined : `swing-${i - 1}`, b: s.id, kind: "revolute", anchorA: [x, y - LINK * i] }));
  return { sprites: [...links, ball], joints };
}
export function goalAt(x: number, y: number): SpriteNode {
  return { id: "goal", x, y, w: 16, h: 80, color: "#6bcb77", body: { kind: "fixed", sensor: true, collide: true } };
}

/**
 * 终点旗杆顶上挥动的 3D 旗子：带骨骼动画的 glTF（布宽 1、高 0.6，从原点往右下挂），放大 40 倍挂在旗杆右上角。
 * 布摆起来前后各约 20，放在 z = 30 整块都在角色（z = 5）前面：和 `camera3d.interleave` 一起，角色跳过终点时从布后面经过。
 */
export const flagOn = (goal: SpriteNode): ModelNode => ({
  id: "flag",
  model: "asset:/@platformer/flag.glb",
  x: (goal.x ?? 0) + (goal.w ?? 0) / 2,
  y: (goal.y ?? 0) + (goal.h ?? 0) / 2 - 2,
  z: 30,
  scale: 40,
  animation: { clip: "wave" },
});

/**
 * 用 Tiled 画的关卡（见 docs/tiled.md）。图块集 `tiles` 和 tiles.png 一样；对象层里的对象按类型生成：
 * `coin`、`enemy`（属性 `patrol`：往右巡逻几格）、`ghost`（会飞、追角色的幽灵）、`lift`（属性 `dx` / `dy`：往哪走，像素，Tiled 里 y 朝下；`duration`）、`crate`、`swing`（铁链的支点）、`goal`。
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
    enemies: of("enemy").map((o, i) => ({ id: `enemy-${i}`, x: o.x, y: ENEMY_Y, to: { x: o.x + num(o, "patrol", 3) * TILE } })),
    ghosts: of("ghost").map((o, i) => ({ id: `enemy-ghost-${i}`, x: o.x, y: o.y })),
    walkers: of("walker").map((o, i) => ({ id: `enemy-walker-${i}`, x: o.x, y: o.y, patrol: num(o, "patrol", 3) })),
    ramps: [],
    tower: null,
    jelly: null,
    crate: crateAt(crate.x, crate.y),
    swing: swing === undefined ? NO_SWING : swingAt(swing.x, swing.y),
    goal: goalAt(goal.x, goal.y),
    right: width,
    top: (map.height - 1) * map.tileheight,
    ...EXTRAS,
  };
}

/**
 * 关卡文件里的东西：游戏里的 id 按编辑器里实体的 id 生成（删掉、重排前面的不影响后面的，拿过的金币、踩过的敌人还记在对的上面）。
 * 用 `:` 隔开（实体 id 里没有 `:`），不同类型的不会撞；敌人以 enemy- 开头，Rust 按这个认。
 */
function entitiesOf(entities: Entity[]) {
  const of = (type: string) => entities.filter((e) => e.type === type);
  const num = (e: Entity, key: string) => propOf<number>(e, key);
  const lifts = of("lift").map((e) => {
    const [dx, dy] = [num(e, "dx"), num(e, "dy")];
    return lift(`lift:${e.id}`, e.x, e.y, dy === 0 ? { x: e.x + dx } : { y: e.y + dy }, num(e, "duration"), sizeOf(e)[0]);
  });
  const ramps = of("ramp").map((e) => {
    const [w, h] = sizeOf(e);
    const down = propOf<boolean>(e, "down");
    return rampOf(`ramp:${e.id}`, [e.x - w / 2, e.y + (down ? h : -h) / 2], [e.x + w / 2, e.y + (down ? -h : h) / 2]);
  });
  const lights = of("light").map((e): LightNode => {
    const spot = propOf<string>(e, "kind") === "spot";
    const base: LightNode = { id: `light:${e.id}`, kind: spot ? "spot" : "point", x: e.x, y: e.y, z: num(e, "z"), color: propOf<string>(e, "color"), intensity: num(e, "intensity"), range: num(e, "range") };
    // 聚光灯朝关卡平面往下照
    return spot ? { ...base, angle: num(e, "angle"), direction: [0, -0.5, -1] } : base;
  });
  const models = of("model").map((e): ModelNode => {
    const model: ModelNode = { id: `model:${e.id}`, model: `asset:/@platformer/${propOf<string>(e, "model")}`, x: e.x, y: e.y, z: num(e, "z"), scale: num(e, "scale"), spin: num(e, "spin") };
    return rotationOf(e) === 0 ? model : { ...model, rotation: [0, 0, rotationOf(e)] };
  });
  const decos = of("deco").map((e): SpriteNode => {
    const [w, h] = sizeOf(e);
    return { id: `deco:${e.id}`, x: e.x, y: e.y, w, h, z: e.z ?? DECO_Z, rotation: rotationOf(e), color: propOf<string>(e, "color") };
  });
  // 实体的 (x, y) 是包围盒中心，实例的原点要减去偏移
  const instances = of("prefab").map((e): PrefabInstance => {
    const name = propOf<string>(e, "prefab");
    const [cx, cy] = PREFAB_BOXES[name]?.center ?? [0, 0];
    const instance: PrefabInstance = { id: `prefab:${e.id}`, prefab: PREFAB_REFS[name] ?? name, x: e.x - cx, y: e.y - cy };
    return e.set === undefined ? instance : { ...instance, set: e.set };
  });
  const walkers = of("walker").map((e): Walker => {
    const walker: Walker = { id: `enemy-walker:${e.id}`, x: e.x, y: e.y, patrol: num(e, "patrol") };
    if (e.points !== undefined && e.points.length > 0) walker.route = e.points;
    // 只带关卡里写了的：没改的和以前一样
    for (const key of ["speed", "jump", "sight", "rest"] as const) if (e.props?.[key] !== undefined) walker[key] = num(e, key);
    return walker;
  });
  return { of, lifts, ramps, lights, models, decos, instances, walkers };
}

/** 关卡文件 → 游戏用的关卡。先查格式和类型（缺终点、属性超范围……），不合规的不转，返回错误。 */
export function fromFile(file: unknown): { ok: true; built: Built; name: string; night: boolean } | { ok: false; errors: string[] } {
  const parsed = parseLevel(file);
  if (!parsed.ok) return parsed;
  const level = parsed.level;
  const errors = checkTypes(level, TYPES);
  for (const e of level.entities.filter((e) => e.type === "lift" && propOf<number>(e, "dx") !== 0 && propOf<number>(e, "dy") !== 0)) {
    errors.push(`${e.id}：移动平台只能往一个方向走（横向、纵向距离只给一个）`);
  }
  if (errors.length > 0) return { ok: false, errors };
  const { of, lifts, ramps, lights, models, decos, instances, walkers } = entitiesOf(level.entities);
  // 有视差的层只是画面：碰撞体不跟着视差挪（docs/parallax.md），不挡人，也不算进相机的范围
  const moving = (t: { parallax?: [number, number] }) => t.parallax !== undefined && (t.parallax[0] !== 1 || t.parallax[1] !== 1);
  const ground = level.tiles.filter((t) => !moving(t));
  const one = (type: string) => of(type)[0];
  const spawn = one("spawn");
  const swing = one("swing");
  const tower = one("tower");
  const jelly = one("jelly");
  const crate = one("crate");
  const goal = one("goal");
  const built: Built = {
    // 角色的速度归 Rust 管，地面默认不要摩擦
    tilemaps: level.tiles.map((t, i) => {
      const map: TilemapNode = { ...toTilemap(t, i), friction: t.friction ?? 0 };
      if (!moving(t)) return map;
      const { oneWay: _oneWay, slopes: _slopes, ...rest } = map;
      return { ...rest, solid: [] };
    }),
    coins: of("coin").map((e) => ({ id: `coin:${e.id}`, x: e.x, y: e.y })),
    lifts,
    enemies: of("enemy").map((e) => ({ id: `enemy-patrol:${e.id}`, x: e.x, y: e.y, to: { x: e.x + propOf<number>(e, "patrol") * TILE } })),
    ghosts: of("ghost").map((e) => ({ id: `enemy-ghost:${e.id}`, x: e.x, y: e.y })),
    walkers,
    ramps,
    tower: tower === undefined ? null : tower.x,
    jelly: jelly === undefined ? null : jelly.x,
    crate: crateAt(crate.x, crate.y),
    // 铁链的支点在包围盒顶边的中点
    swing: swing === undefined ? NO_SWING : swingAt(swing.x, swing.y + sizeOf(swing)[1] / 2),
    goal: goalAt(goal.x, goal.y),
    right: Math.max(0, ...ground.map((t) => t.x + t.cols * t.tile)),
    top: Math.max(0, ...ground.map((t) => t.y)),
    spawn: spawn === undefined ? SPAWN : [spawn.x, spawn.y],
    lights,
    models,
    decos,
    instances,
  };
  return { ok: true, built, name: level.name, night: level.night };
}

/** 代码描述的关卡 → 关卡文件（导出用）：位置都取 `build` 生成的，转回来和原来一样。 */
export function toFile(level: Level): LevelFile {
  const b = build(level);
  const map = b.tilemaps[0];
  let n = 0;
  const at = (type: string, x: number, y: number, rest: Partial<Entity> = {}): Entity => ({ id: `e${++n}`, type, x, y, ...rest });
  const swingSize = sizeOf({ id: "", type: "swing", x: 0, y: 0 });
  const entities: Entity[] = [
    at("spawn", ...SPAWN),
    ...b.coins.map((c) => at("coin", c.x, c.y)),
    ...b.lifts.map(({ node, to, duration }) => {
      const props = to.x === undefined ? { dx: 0, dy: to.y! - node.y! } : { dx: to.x - node.x!, dy: 0 };
      return at("lift", node.x!, node.y!, { props: { ...props, duration } });
    }),
    ...b.enemies.map((e) => at("enemy", e.x, e.y, { props: { patrol: (e.to.x - e.x) / TILE } })),
    ...b.ghosts.map((g) => at("ghost", g.x, g.y)),
    ...b.walkers.map((w) => at("walker", w.x, w.y, { props: { patrol: w.patrol } })),
    ...(level.ramps ?? []).map(([[ax, ay], [bx, by]]) => {
      const [left, right] = ax <= bx ? [[ax, ay], [bx, by]] : [[bx, by], [ax, ay]];
      return at("ramp", (ax + bx) / 2, (ay + by) / 2, { w: right[0] - left[0], h: Math.abs(by - ay), props: { down: right[1] < left[1] } });
    }),
    ...(b.tower === null ? [] : [at("tower", b.tower, TYPES.tower.fixedY!)]),
    ...(b.jelly === null ? [] : [at("jelly", b.jelly, TYPES.jelly.fixedY!)]),
    at("crate", b.crate.body.x!, b.crate.body.y!),
    ...(level.swing === undefined ? [] : [at("swing", level.swing[0] * TILE, level.swing[1] - swingSize[1] / 2)]),
    at("goal", b.goal.x!, b.goal.y!),
  ];
  const data = Array.from({ length: ROWS }, (_, r) => map.data.slice(r * map.cols, (r + 1) * map.cols));
  return {
    format: LEVEL_FORMAT,
    version: LEVEL_VERSION,
    name: level.name,
    night: false,
    tiles: [{ id: map.id, texture: map.texture, grid: map.grid!, tile: map.tile, cols: map.cols, rows: ROWS, x: map.x!, y: map.y!, solid: map.solid!, data, friction: 0 }],
    entities,
  };
}

/** 读坏了的关卡文件时顶上的：一段平地加终点，照样能玩。 */
function fallbackLevel(): Built {
  const cols = 20;
  const data = Array.from({ length: ROWS * cols }, (_, i) => (i >= (ROWS - 1) * cols ? TILESET.frames[1].frame : -1));
  const tilemaps: TilemapNode[] = [{ id: "level", texture: TILESET.texture, grid: TILESET.grid, tile: TILE, cols, x: 0, y: (ROWS - 1) * TILE, data, solid: TILESET.solid, friction: 0 }];
  return { tilemaps, coins: [], lifts: [], enemies: [], ghosts: [], walkers: [], ramps: [], tower: null, jelly: null, crate: crateAt(10 * TILE, 56), swing: NO_SWING, goal: goalAt((cols - 4) * TILE, 40), right: cols * TILE, top: (ROWS - 1) * TILE, ...EXTRAS };
}

/** 游戏里的一关：关卡文件读不了时（编辑器存了个没做完的）不崩，换成一段平地并报错。 */
export function fileLevel(raw: unknown, fallbackName: string, report: (message: string) => void = console.error): { name: string; built: Built; night?: boolean } {
  const r = fromFile(raw);
  if (r.ok) return { name: r.name || fallbackName, built: r.built, night: r.night };
  report(`关卡文件「${fallbackName}」读不了，先用一段平地顶上：${r.errors.join("；")}`);
  return { name: fallbackName, built: fallbackLevel() };
}

/** 第一关（导出、测试用）。 */
export const MEADOW = DESCRIBED[0];

/** `night`：夜里，开 2D 光照（见 effects.ts）。 */
export const LEVELS: { name: string; built: Built; night?: boolean }[] = [
  { name: MEADOW.name, built: build(MEADOW) },
  fileLevel(stairs, "石阶"),
  { name: "云端", built: fromTiled(level3), night: true },
];

/** 拿到了几个金币：只数关卡里还有的（热重载删了金币以后不会「得到」比总数多）。 */
export function coinsGot(coins: { id: string }[], taken: string[]): number {
  return coins.filter((c) => taken.includes(c.id)).length;
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
