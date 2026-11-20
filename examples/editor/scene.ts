// 编辑时游戏视图里的场景（docs/editor.md、docs/editor-more.md）：实体按游戏的样子画，加上网格、选中框、悬停框、路线点、框选框；
// 不动、没有刚体。藏起来的瓦片层不画，预制体实例照游戏里一样展开。
import { toTilemap, type LevelFile, type TileLayer } from "../../ts/level";
import type { CameraView, LightNode, ModelNode, PrefabInstance, Scene, SpriteNode, TilemapNode } from "../../ts/nge";
import { boundsOf, PREFAB_BOXES, propOf } from "../platformer/catalog";
import { night, SHADERS } from "../platformer/effects";
import { PREFAB_REFS } from "../platformer/levels";
import { PREFABS } from "../platformer/prefabs";
import { previewAt, previewId } from "../platformer/preview";

/** 网格线一个方向最多画几条：再多就太密，不画。 */
export const MAX_GRID_LINES = 160;
const SELECT_COLOR = "#4da3ff";
const HOVER_COLOR = "#ffffffa0";
const POINT_COLOR = "#ffb0b0";
const SUN: LightNode = { id: "sun", kind: "directional", direction: [-1, -2, -1.5], shadows: true };

export interface SceneInput {
  level: LevelFile;
  selection: string[];
  view: CameraView;
  size: [number, number];
  grid: number;
  /** 框选框的两个角（世界坐标）。 */
  box: [[number, number], [number, number]] | null;
  /** 指针下面的实体（悬停高亮）。 */
  hover?: string | null;
}

/** 一个矩形的四条边（`t` 是线宽）。 */
function outline(id: string, l: number, r: number, b: number, top: number, t: number, color: string): SpriteNode[] {
  const [cx, cy, w, h] = [(l + r) / 2, (b + top) / 2, r - l, top - b];
  const line = (part: string, x: number, y: number, lw: number, lh: number): SpriteNode => ({ id: `${id}:${part}`, x, y, w: lw, h: lh, z: 95, color, unlit: true });
  return [line("t", cx, top, w + t, t), line("b", cx, b, w + t, t), line("l", l, cy, t, h + t), line("r", r, cy, t, h + t)];
}

/** 视野里的网格线。 */
export function gridLines(view: CameraView, size: [number, number], grid: number): SpriteNode[] {
  if (!(grid > 0)) return [];
  const [hw, hh] = [size[0] / (2 * view.zoom), size[1] / (2 * view.zoom)];
  const [x0, x1] = [Math.ceil((view.x - hw) / grid), Math.floor((view.x + hw) / grid)];
  const [y0, y1] = [Math.ceil((view.y - hh) / grid), Math.floor((view.y + hh) / grid)];
  if (x1 - x0 + 1 > MAX_GRID_LINES || y1 - y0 + 1 > MAX_GRID_LINES) return [];
  const t = 1 / view.zoom;
  const color = "#ffffff1c";
  const out: SpriteNode[] = [];
  for (let i = x0; i <= x1; i++) out.push({ id: `grid:v${i}`, x: i * grid, y: view.y, w: t, h: 2 * hh, z: 90, color, unlit: true });
  for (let j = y0; j <= y1; j++) out.push({ id: `grid:h${j}`, x: view.x, y: j * grid, w: 2 * hw, h: t, z: 90, color, unlit: true });
  return out;
}

/** 瓦片层 → 引擎的瓦片地图：按层对象缓存，拖动实体时瓦片层没变，不用每次都摊平一遍。 */
const tilemaps = new WeakMap<TileLayer, TilemapNode[]>();
function tilemapOf(t: TileLayer, index: number): TilemapNode {
  const cached = tilemaps.get(t) ?? [];
  let node = cached[index];
  if (node === undefined) {
    // 瓦片不要碰撞体：编辑时用不上（z 按层的顺序，所以按下标缓存）
    node = { ...toTilemap(t, index), solid: [], oneWay: [], slopes: [] };
    cached[index] = node;
    tilemaps.set(t, cached);
  }
  return node;
}

/** 预制体实体 → 场景的实例（和游戏里一样：实体的 (x, y) 是包围盒中心）。 */
function instanceOf(e: LevelFile["entities"][number]): PrefabInstance {
  const name = propOf<string>(e, "prefab");
  const [cx, cy] = PREFAB_BOXES[name]?.center ?? [0, 0];
  return { id: previewId(e, "inst"), prefab: PREFAB_REFS[name] ?? name, x: e.x - cx, y: e.y - cy, ...(e.set === undefined ? {} : { set: e.set }) };
}

export function editorScene(s: SceneInput): Scene {
  const sprites: SpriteNode[] = [];
  const models: ModelNode[] = [];
  const lights: LightNode[] = [SUN];
  for (const [i, e] of s.level.entities.entries()) {
    const p = previewAt(e, i);
    sprites.push(...p.sprites);
    models.push(...p.models);
    lights.push(...p.lights);
  }
  const t = 2 / s.view.zoom;
  const selected = new Set(s.selection);
  for (const e of s.level.entities.filter((e) => selected.has(e.id))) {
    const b = boundsOf(e);
    sprites.push(...outline(`sel:${e.id}`, b.left, b.right, b.bottom, b.top, t, SELECT_COLOR));
    // 选中的东西的路线点：能拖的小方块
    (e.points ?? []).forEach(([x, y], i) => sprites.push({ id: `pt:${e.id}:${i}`, x, y, w: 8 / s.view.zoom, h: 8 / s.view.zoom, z: 96, color: POINT_COLOR, unlit: true }));
  }
  const hovered = s.hover === undefined || s.hover === null || selected.has(s.hover) ? undefined : s.level.entities.find((e) => e.id === s.hover);
  if (hovered !== undefined) {
    const b = boundsOf(hovered);
    sprites.push(...outline(`hov:${hovered.id}`, b.left, b.right, b.bottom, b.top, 1 / s.view.zoom, HOVER_COLOR));
  }
  if (s.box !== null) {
    const [[ax, ay], [bx, by]] = s.box;
    const [l, r, b, top] = [Math.min(ax, bx), Math.max(ax, bx), Math.min(ay, by), Math.max(ay, by)];
    sprites.push({ id: "box", x: (l + r) / 2, y: (b + top) / 2, w: r - l, h: top - b, z: 94, color: `${SELECT_COLOR}22`, unlit: true });
    sprites.push(...outline("box", l, r, b, top, 1 / s.view.zoom, SELECT_COLOR));
  }
  sprites.push(...gridLines(s.view, s.size, s.grid));
  // 夜里：和游戏一样的环境光、终点的灯和暗角；跟着角色的灯跟着出生点
  const goal = s.level.entities.find((e) => e.type === "goal");
  const spawn = s.level.entities.find((e) => e.type === "spawn");
  const dark = s.level.night && goal !== undefined ? night({ id: "goal", x: goal.x, y: goal.y }) : null;
  const lighting = dark === null ? undefined : { ...dark.lighting, lights: (dark.lighting.lights ?? []).flatMap((l) => (l.follow !== "player" ? [l] : spawn === undefined ? [] : [{ ...l, follow: previewId(spawn) }])) };
  return {
    level: "edit",
    background: s.level.night ? "#0b0f1e" : "#1b2233",
    // 编辑时什么都不动：物理、补间、动画都停着
    timeScale: 0,
    shaders: SHADERS,
    camera: { x: s.view.x, y: s.view.y, zoom: s.view.zoom },
    camera3d: { match2d: true, interleave: true },
    lights,
    ambient: { brightness: 0.35 },
    // 藏起来的层不画（下标按原来的算，z 不跟着变）
    tilemaps: s.level.tiles.flatMap((t, i) => (t.hidden === true ? [] : [tilemapOf(t, i)])),
    prefabs: PREFABS,
    instances: s.level.entities.filter((e) => e.type === "prefab").map(instanceOf),
    sprites,
    models,
    ...(lighting === undefined ? {} : { lighting, post: dark!.post }),
  };
}
