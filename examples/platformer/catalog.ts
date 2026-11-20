// 平台跳跃关卡文件里的实体类型（见 docs/editor.md）：名字、默认尺寸、属性。只有数据，编辑器和 `fromFile` 都用它。
import { checkProp, type Entity, type PropSpec, type PropValue, type TypeSpec } from "../../ts/level";

export interface EntityType extends TypeSpec {
  label: string;
  /** 默认尺寸（包围盒，中心在实体的 (x, y)）。 */
  size: [number, number];
  /** 物件库和场景树里的色块。 */
  color: string;
  /** 站在地上的东西：y 固定在这里（包围盒的中心），拖动只改 x。 */
  fixedY?: number;
  /** 能在属性面板里改的尺寸：`"w"` 只能改宽（移动平台的高在游戏里固定是 16），`"wh"` 宽高都能改。 */
  resizable?: "w" | "wh";
  /** 能转（`rotation`，度）：装饰、模型。 */
  rotatable?: true;
  /** 能单独设 z（`z`，画的先后）：装饰。 */
  layered?: true;
  /** 有路线点（`points`）：走地怪。 */
  route?: true;
}

const MODELS = ["crate.glb", "flag.glb", "jelly.glb"];
/** 物件库里能放的预制体：`prefabs.ts` 的名字或文件（见 docs/scene-flow.md）。 */
export const PREFAB_CHOICES = ["grove", "tree"];
/** 预制体的包围盒：大小、实例原点到包围盒中心的偏移（实体的 (x, y) 是包围盒中心，实例放在 (x, y) 减偏移）。 */
export const PREFAB_BOXES: Record<string, { size: [number, number]; center: [number, number] }> = {
  // 两棵树：树干底在原点，第二棵在右边 127、树冠高 14
  grove: { size: [192, 140], center: [64, 68] },
  tree: { size: [64, 124], center: [0, 60] },
};

export const TYPES: Record<string, EntityType> = {
  spawn: { label: "出生点", size: [24, 32], color: "#ff7a59", props: [], max: 1 },
  coin: { label: "金币", size: [14, 14], color: "#ffd93d", props: [] },
  enemy: { label: "敌人", size: [24, 24], color: "#9b59b6", props: [{ key: "patrol", label: "巡逻格数", kind: "number", min: 1, max: 20, default: 3 }] },
  ghost: { label: "幽灵", size: [22, 22], color: "#c8d2ff", props: [] },
  walker: {
    label: "走地怪",
    size: [24, 24],
    color: "#d35f5f",
    route: true,
    props: [
      { key: "patrol", label: "巡逻格数", kind: "number", min: 1, max: 20, default: 3 },
      { key: "speed", label: "速度", kind: "number", min: 40, max: 400, default: 110 },
      { key: "jump", label: "起跳速度", kind: "number", min: 200, max: 1200, default: 720 },
      { key: "sight", label: "视野格数", kind: "number", min: 1, max: 30, default: 8 },
      { key: "rest", label: "两头停几秒", kind: "number", min: 0, max: 5, default: 0.6 },
    ],
  },
  lift: {
    label: "移动平台",
    size: [64, 16],
    color: "#c08552",
    resizable: "w",
    props: [
      { key: "dx", label: "横向距离", kind: "number", min: -2000, max: 2000, default: 128 },
      { key: "dy", label: "纵向距离", kind: "number", min: -2000, max: 2000, default: 0 },
      { key: "duration", label: "一趟秒数", kind: "number", min: 0.2, max: 30, default: 2 },
    ],
  },
  crate: { label: "3D 箱子", size: [40, 40], color: "#a0703c", props: [], min: 1, max: 1 },
  swing: { label: "铁链", size: [20, 92], color: "#8a8f9c", props: [], max: 1 },
  goal: { label: "终点", size: [16, 80], color: "#6bcb77", props: [], min: 1, max: 1 },
  ramp: { label: "斜坡", size: [96, 32], color: "#8a6f4e", resizable: "wh", props: [{ key: "down", label: "往右下降", kind: "bool", default: false }] },
  tower: { label: "箱子堆", size: [48, 44], color: "#d9a066", fixedY: 22, props: [], max: 1 },
  jelly: { label: "果冻", size: [48, 30], color: "#7fd1c7", fixedY: 15, props: [], max: 1 },
  light: {
    label: "灯",
    size: [16, 16],
    color: "#fff2a8",
    props: [
      { key: "kind", label: "种类", kind: "choice", options: ["point", "spot"], default: "point" },
      { key: "color", label: "颜色", kind: "text", default: "#ffffff", pattern: /^#([0-9a-f]{6}|[0-9a-f]{8})$/i, hint: " #rrggbb 或 #rrggbbaa" },
      { key: "intensity", label: "强度", kind: "number", min: 0, max: 1e6, default: 20000 },
      { key: "range", label: "范围", kind: "number", min: 1, max: 10000, default: 400 },
      { key: "z", label: "离关卡平面", kind: "number", min: -2000, max: 2000, default: 100 },
      { key: "angle", label: "聚光角", kind: "number", min: 1, max: 89, default: 30 },
    ],
  },
  model: {
    label: "模型",
    size: [40, 40],
    color: "#88a0c8",
    rotatable: true,
    props: [
      { key: "model", label: "模型", kind: "choice", options: MODELS, default: "crate.glb" },
      { key: "scale", label: "缩放", kind: "number", min: 1, max: 500, default: 40 },
      { key: "z", label: "离关卡平面", kind: "number", min: -500, max: 500, default: 0 },
      { key: "spin", label: "每秒转", kind: "number", min: -720, max: 720, default: 0 },
    ],
  },
  group: { label: "分组", size: [16, 16], color: "#8899aa", props: [] },
  deco: {
    label: "装饰",
    size: [32, 32],
    color: "#556677",
    resizable: "wh",
    rotatable: true,
    layered: true,
    props: [{ key: "color", label: "颜色", kind: "text", default: "#556677", pattern: /^#([0-9a-f]{6}|[0-9a-f]{8})$/i, hint: " #rrggbb 或 #rrggbbaa" }],
  },
  prefab: { label: "预制体", size: [160, 140], color: "#2f3d5c", props: [{ key: "prefab", label: "预制体", kind: "choice", options: PREFAB_CHOICES, default: "grove" }] },
};

/** 物件库里的顺序。 */
export const TYPE_ORDER = Object.keys(TYPES);

/** 瓦片图集：和 levels.ts 的 TILE、tiles.png（tools/make_tiles.py）一致。 */
export const TILESET = {
  texture: "asset:/@platformer/tiles.png",
  grid: [16, 1] as [number, number],
  tile: 32,
  solid: [0, 1, 2],
  /** 单向木板：从下面跳得上去，按下加跳穿下去。 */
  oneWay: [3],
  /** 45° 上坡、下坡，22.5° 上坡（两格一组）、下坡（见 docs/gameplay2d.md）。 */
  slopes: [
    { frame: 4, left: 0, right: 1 },
    { frame: 5, left: 1, right: 0 },
    { frame: 6, left: 0, right: 0.5 },
    { frame: 7, left: 0.5, right: 1 },
    { frame: 8, left: 1, right: 0.5 },
    { frame: 9, left: 0.5, right: 0 },
  ],
  /** 水（10 → 11 → 12）、火把（13 ↔ 14）：只是画面，不挡人。 */
  animated: [
    { frame: 10, frames: [10, 11, 12], fps: 4 },
    { frame: 13, frames: [13, 14], fps: 6 },
  ],
  frames: [
    { frame: 0, label: "石块", color: "#7d7f86" },
    { frame: 1, label: "草地", color: "#5fae4f" },
    { frame: 2, label: "木板", color: "#b0804a" },
    { frame: 3, label: "单向板", color: "#c89a5c" },
    { frame: 4, label: "上坡", color: "#7a5638" },
    { frame: 5, label: "下坡", color: "#7a5638" },
    { frame: 6, label: "缓坡上1", color: "#7a5638" },
    { frame: 7, label: "缓坡上2", color: "#7a5638" },
    { frame: 8, label: "缓坡下1", color: "#7a5638" },
    { frame: 9, label: "缓坡下2", color: "#7a5638" },
    { frame: 10, label: "水", color: "#3c82d2" },
    { frame: 13, label: "火把", color: "#f07828" },
  ],
};

/** 实体的宽高：能改的用自己写的，不然用类型的默认值（不认识的类型按 16 × 16）。和游戏里一样：不能改的写了也不算。 */
export function sizeOf(e: Entity): [number, number] {
  if (e.type === "prefab") return PREFAB_BOXES[propOf<string>(e, "prefab")]?.size ?? [16, 16];
  const spec = TYPES[e.type];
  const [w, h] = spec?.size ?? [16, 16];
  const r = spec?.resizable;
  return [r !== undefined ? (e.w ?? w) : w, r === "wh" ? (e.h ?? h) : h];
}

/** 转了多少度：能转的类型才算。 */
export function rotationOf(e: Entity): number {
  return TYPES[e.type]?.rotatable === true ? (e.rotation ?? 0) : 0;
}

/** 包围盒：转过的取四个角的外接矩形。 */
export function boundsOf(e: Entity): { left: number; right: number; bottom: number; top: number } {
  const [w, h] = sizeOf(e);
  const a = (rotationOf(e) * Math.PI) / 180;
  const [c, s] = [Math.abs(Math.cos(a)), Math.abs(Math.sin(a))];
  const [hw, hh] = [(w * c + h * s) / 2, (w * s + h * c) / 2];
  return { left: e.x - hw, right: e.x + hw, bottom: e.y - hh, top: e.y + hh };
}

/** 属性的说明（找不到是 undefined）。 */
export function propSpec(type: string, key: string): PropSpec | undefined {
  return TYPES[type]?.props.find((p) => p.key === key);
}

/** 属性值：没写或不合规（类型不对、超范围、不在选项里）用默认值。 */
export function propOf<T extends PropValue>(e: Entity, key: string): T {
  const spec = propSpec(e.type, key);
  const v = e.props?.[key];
  return (v !== undefined && (spec === undefined || checkProp(spec, v) === null) ? v : spec?.default) as T;
}

/** 放到 (x, y)：站在地上的类型 y 固定。 */
export function placed(e: Entity, x: number, y: number): Entity {
  const fixedY = TYPES[e.type]?.fixedY;
  return { ...e, x, y: fixedY ?? y };
}

/** 新的实体：属性不写，用默认值。 */
export function newEntity(type: string, id: string, x: number, y: number): Entity {
  return placed({ id, type, x, y }, x, y);
}

/** 场景树上显示的名字：自己的名字，没有用类型名。 */
export function labelOf(e: Entity): string {
  return e.name !== undefined && e.name !== "" ? e.name : (TYPES[e.type]?.label ?? e.type);
}
