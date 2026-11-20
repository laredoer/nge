// 多个瓦片层（docs/editor-more.md）：层的命令、某一帧的规则、视差层的拾取。都不改传进来的东西。
import type { LevelFile, TileLayer, TileSlope } from "../../ts/level";
import type { CameraView } from "../../ts/nge";
import type { Command } from "./history";

const mapLayer = (level: LevelFile, index: number, fn: (t: TileLayer) => TileLayer): LevelFile => ({ ...level, tiles: level.tiles.map((t, i) => (i === index ? fn(t) : t)) });

/** 新层的 id：`layer<n>`，不和已有的重。 */
export function freshLayerId(level: LevelFile): string {
  const used = new Set(level.tiles.map((t) => t.id));
  let n = level.tiles.length + 1;
  while (used.has(`layer${n}`)) n++;
  return `layer${n}`;
}

/** 在第 `index` 层后面加一层：和它一样大、一样的图集和规则，全空，没有视差。 */
export function addLayer(level: LevelFile, index: number): Command {
  const base = level.tiles[index];
  const id = freshLayerId(level);
  const at = index + 1;
  return {
    label: "加一层",
    apply: (l) => {
      if (base === undefined) return l;
      const { parallax: _p, hidden: _h, locked: _k, name: _n, z: _z, ...rest } = base;
      const layer: TileLayer = { ...rest, id, name: `第 ${l.tiles.length + 1} 层`, data: base.data.map((row) => row.map(() => -1)) };
      return { ...l, tiles: [...l.tiles.slice(0, at), layer, ...l.tiles.slice(at)] };
    },
    revert: (l) => ({ ...l, tiles: l.tiles.filter((t) => t.id !== id) }),
  };
}

/** 删掉第 `index` 层（最后一层不能删：没有层就没地方画了）；撤回时放回原位。 */
export function removeLayer(level: LevelFile, index: number): Command | null {
  const old = level.tiles[index];
  if (old === undefined || level.tiles.length <= 1) return null;
  return {
    label: "删一层",
    apply: (l) => ({ ...l, tiles: l.tiles.filter((t) => t.id !== old.id) }),
    revert: (l) => ({ ...l, tiles: [...l.tiles.slice(0, index), old, ...l.tiles.slice(index)] }),
  };
}

/** 把第 `from` 层挪到第 `to` 个；挪不动（到头了）返回 null。 */
export function moveLayer(level: LevelFile, from: number, to: number): Command | null {
  if (from === to || to < 0 || to >= level.tiles.length || level.tiles[from] === undefined) return null;
  const id = level.tiles[from].id;
  const move = (dest: number) => (l: LevelFile) => {
    const at = l.tiles.findIndex((t) => t.id === id);
    if (at < 0) return l;
    const rest = l.tiles.filter((t) => t.id !== id);
    return { ...l, tiles: [...rest.slice(0, dest), l.tiles[at], ...rest.slice(dest)] };
  };
  return { label: "挪层", apply: move(to), revert: move(from) };
}

/** 层的字段（名字、显隐、锁定、视差、z、规则）；值是 undefined 的去掉。撤回时还原原来的值。 */
export type LayerPatch = Partial<Omit<TileLayer, "id" | "data" | "cols" | "rows" | "texture" | "grid" | "tile">>;

export function updateLayer(level: LevelFile, index: number, patch: LayerPatch): Command {
  const old = level.tiles[index];
  const before = Object.fromEntries(Object.keys(patch).map((k) => [k, old?.[k as keyof TileLayer]])) as LayerPatch;
  const set = (p: LayerPatch) => (l: LevelFile) =>
    old === undefined
      ? l
      : mapLayer(l, index, (t) => {
          const next: Record<string, unknown> = { ...t, ...p };
          for (const k of Object.keys(p)) if (next[k] === undefined) delete next[k];
          return next as unknown as TileLayer;
        });
  return { label: "改层", apply: set(patch), revert: set(before) };
}

/** 一帧的规则：碰撞的种类、坡的两头、动画、材质。 */
export interface FrameRule {
  kind: "solid" | "oneWay" | "slope" | "none";
  slope?: { left: number; right: number };
  animation?: { frames: number[]; fps: number };
  friction?: number;
  restitution?: number;
}

/** 这一层上 `frame` 这一帧的规则（斜坡优先，然后单向，最后实心，和引擎一样）。 */
export function frameRule(t: TileLayer, frame: number): FrameRule {
  const slope = t.slopes?.find((s) => s.frame === frame);
  const kind = slope !== undefined ? "slope" : t.oneWay?.includes(frame) ? "oneWay" : t.solid.includes(frame) ? "solid" : "none";
  const rule: FrameRule = { kind };
  if (slope !== undefined) rule.slope = { left: slope.left, right: slope.right };
  const anim = t.animated?.find((a) => a.frame === frame);
  if (anim !== undefined) rule.animation = { frames: anim.frames, fps: anim.fps };
  const material = t.materials?.find((m) => m.frame === frame);
  if (material?.friction !== undefined) rule.friction = material.friction;
  if (material?.restitution !== undefined) rule.restitution = material.restitution;
  return rule;
}

/** 列表里去掉这一帧、按需要加上；空了的列表去掉（实心的列表一直留着）。 */
function withList<T extends { frame: number }>(list: T[] | undefined, frame: number, item: T | null): T[] | undefined {
  const rest = (list ?? []).filter((x) => x.frame !== frame);
  const next = item === null ? rest : [...rest, item].sort((a, b) => a.frame - b.frame);
  return next.length === 0 ? undefined : next;
}

/** 把这一帧的规则改成 `rule`：返回要改的层字段（给 `updateLayer`）。种类互斥：换种类时从别的列表里去掉。 */
export function rulePatch(t: TileLayer, frame: number, rule: FrameRule): LayerPatch {
  const without = (list: number[] | undefined) => (list ?? []).filter((f) => f !== frame);
  const solid = rule.kind === "solid" ? [...without(t.solid), frame].sort((a, b) => a - b) : without(t.solid);
  const oneWayList = rule.kind === "oneWay" ? [...without(t.oneWay), frame].sort((a, b) => a - b) : without(t.oneWay);
  const slope: TileSlope | null = rule.kind === "slope" ? { frame, left: rule.slope?.left ?? 0, right: rule.slope?.right ?? 1 } : null;
  const material = rule.friction === undefined && rule.restitution === undefined ? null : { frame, ...(rule.friction === undefined ? {} : { friction: rule.friction }), ...(rule.restitution === undefined ? {} : { restitution: rule.restitution }) };
  return {
    solid,
    oneWay: oneWayList.length === 0 ? undefined : oneWayList,
    slopes: withList(t.slopes, frame, slope),
    animated: withList(t.animated, frame, rule.animation === undefined ? null : { frame, ...rule.animation }),
    materials: withList(t.materials, frame, material),
  };
}

/** 有视差的层上，世界坐标 `w` 对应这一层上的位置：画的位置 = 声明的位置 + 相机 ×（1 − 视差）（docs/parallax.md）。 */
export function layerPoint(t: TileLayer, view: CameraView, [wx, wy]: [number, number]): [number, number] {
  const [px, py] = t.parallax ?? [1, 1];
  return [wx - view.x * (1 - px), wy - view.y * (1 - py)];
}
