// 三段战场：物件摆在哪、每波敌人从哪来。坐标是带子坐标（见 belt.ts）。

export type EnemyKind = "soldier" | "archer" | "boss";

export interface Spawn {
  kind: EnemyKind;
  /** 从锁定区的哪一边进来。 */
  side: "left" | "right";
  /** 纵深。 */
  d: number;
  /** 这一波开始后几秒出来。 */
  delay: number;
}

export interface Wave {
  /** 玩家走到这个 x 时开打，镜头锁住。 */
  at: number;
  spawns: Spawn[];
}

export type PropKind = "wall" | "cart" | "fire" | "banner" | "barricade";

export interface Prop {
  kind: PropKind;
  x: number;
  d: number;
}

export interface Stage {
  /** 文案键里的编号（stage1…）。 */
  from: number;
  waves: Wave[];
  props: Prop[];
}

/** 世界左右边（玩家走不出去）。 */
export const WORLD = { left: 0, right: 4900 };
/** 长坂桥的左端：桥面从这里往右 600，地面铺到这里为止。 */
export const BRIDGE_X = 4300;
export const BRIDGE_LEN = 600;
/** 一段地面模型的长度（tools/make_models.py 的 GROUND_LEN）。 */
export const GROUND_LEN = 400;

const R = "right";
const L = "left";
const s = (kind: EnemyKind, side: "left" | "right", d: number, delay: number): Spawn => ({ kind, side, d, delay });

export const STAGES: Stage[] = [
  {
    // 当阳郊野：塌了的城墙、路边烧着的粮车
    from: 0,
    waves: [
      { at: 250, spawns: [s("soldier", R, 30, 0), s("soldier", R, 70, 0.5), s("soldier", R, 110, 1)] },
      { at: 950, spawns: [s("soldier", R, 40, 0), s("archer", R, 100, 0.4), s("soldier", R, 80, 1.2), s("soldier", L, 60, 2)] },
    ],
    props: [
      { kind: "wall", x: -120, d: 150 },
      { kind: "banner", x: 230, d: 140 },
      { kind: "wall", x: 420, d: 152 },
      { kind: "fire", x: 560, d: 104 },
      { kind: "banner", x: 760, d: 138 },
      { kind: "barricade", x: 870, d: 18 },
      { kind: "wall", x: 1050, d: 150 },
      { kind: "cart", x: 1260, d: 16 },
      { kind: "banner", x: 1420, d: 142 },
    ],
  },
  {
    // 粮道：一路的粮车，不少已经着了
    from: 1600,
    waves: [
      { at: 1900, spawns: [s("soldier", R, 30, 0), s("archer", R, 110, 0.3), s("soldier", R, 90, 0.8), s("archer", L, 20, 1.6)] },
      { at: 2450, spawns: [s("soldier", R, 50, 0), s("soldier", R, 100, 0.4), s("soldier", L, 20, 1), s("archer", R, 120, 1.4)] },
      { at: 2950, spawns: [s("soldier", L, 40, 0), s("soldier", R, 70, 0), s("archer", R, 20, 0.8), s("archer", R, 120, 1.2), s("soldier", R, 100, 2)] },
    ],
    props: [
      { kind: "fire", x: 1700, d: 110 },
      { kind: "wall", x: 1800, d: 156 },
      { kind: "banner", x: 2040, d: 140 },
      { kind: "fire", x: 2160, d: 22 },
      { kind: "cart", x: 2380, d: 112 },
      { kind: "wall", x: 2600, d: 150 },
      { kind: "fire", x: 2700, d: 96 },
      { kind: "banner", x: 2880, d: 138 },
      { kind: "cart", x: 3080, d: 30 },
    ],
  },
  {
    // 长坂桥：拒马挡道，桥头有骑将
    from: 3200,
    waves: [
      { at: 3550, spawns: [s("soldier", R, 30, 0), s("soldier", R, 110, 0.4), s("archer", R, 70, 0.8), s("soldier", L, 60, 1.6)] },
      { at: 4250, spawns: [s("boss", R, 65, 0), s("soldier", R, 20, 1.2), s("soldier", R, 115, 1.8)] },
    ],
    props: [
      { kind: "banner", x: 3300, d: 140 },
      { kind: "wall", x: 3400, d: 154 },
      { kind: "barricade", x: 3760, d: 112 },
      { kind: "barricade", x: 3800, d: 20 },
      { kind: "fire", x: 3950, d: 108 },
      { kind: "banner", x: 4180, d: 142 },
      { kind: "banner", x: BRIDGE_X + 40, d: 150 },
      { kind: "banner", x: BRIDGE_X + 520, d: 150 },
    ],
  },
];

export const WAVES: Wave[] = STAGES.flatMap((st) => st.waves);
export const PROPS: Prop[] = STAGES.flatMap((st) => st.props);

/** 第几段（从 0 数）：玩家走到哪段的 from 就算哪段。 */
export function stageAt(x: number): number {
  let i = 0;
  STAGES.forEach((st, k) => {
    if (x >= st.from) i = k;
  });
  return i;
}

/** 物件在地上占的一块（带子坐标），人走不进去。城墙在带子后面、旌旗很细，不挡人。 */
export interface Footprint {
  x0: number;
  x1: number;
  d0: number;
  d1: number;
}

/** 和 tools/make_models.py 的模型尺寸一致：粮车 x 从 -38 到车辕尖 74、纵深 ±24（轮子）；拒马 x ±21、纵深 ±22。 */
export function footprintOf(p: Prop): Footprint | null {
  if (p.kind === "cart" || p.kind === "fire") return { x0: p.x - 40, x1: p.x + 74, d0: p.d - 24, d1: p.d + 24 };
  if (p.kind === "barricade") return { x0: p.x - 21, x1: p.x + 21, d0: p.d - 22, d1: p.d + 22 };
  return null;
}

export const FOOTPRINTS: Footprint[] = PROPS.map(footprintOf).filter((f): f is Footprint => f !== null);

/** 人（左右半宽 12、纵深半厚 5）站在 (x, d) 会不会踩进物件里。 */
export function blocked(x: number, d: number, prints: Footprint[] = FOOTPRINTS): boolean {
  return prints.some((f) => x + 12 > f.x0 && x - 12 < f.x1 && d + 5 > f.d0 && d - 5 < f.d1);
}
