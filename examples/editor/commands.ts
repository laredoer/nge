// 编辑命令（docs/editor.md）：构造时记下撤回要用的旧值，`apply` / `revert` 都返回新的关卡。
import type { Entity, LevelFile, TileLayer } from "../../ts/level";
import { chain, type Command } from "./history";

const without = (e: Entity, patch: Partial<Entity>): Entity => {
  const next: Record<string, unknown> = { ...e, ...patch };
  for (const k of Object.keys(patch)) if (next[k] === undefined) delete next[k];
  return next as unknown as Entity;
};
const mapEntities = (level: LevelFile, fn: (e: Entity) => Entity): LevelFile => ({ ...level, entities: level.entities.map(fn) });
const mapLayer = (level: LevelFile, index: number, fn: (t: TileLayer) => TileLayer): LevelFile => ({ ...level, tiles: level.tiles.map((t, i) => (i === index ? fn(t) : t)) });

/** 加一组实体，插在第 `index` 个前面（默认最后）。 */
export function addEntities(entities: Entity[], index?: number): Command {
  const ids = new Set(entities.map((e) => e.id));
  return {
    label: entities.length === 1 ? "添加" : `添加 ${entities.length} 个`,
    apply: (level) => {
      const at = index === undefined ? level.entities.length : Math.max(0, Math.min(index, level.entities.length));
      return { ...level, entities: [...level.entities.slice(0, at), ...entities, ...level.entities.slice(at)] };
    },
    revert: (level) => ({ ...level, entities: level.entities.filter((e) => !ids.has(e.id)) }),
  };
}

/** 删掉一组实体；撤回时放回原来的位置。 */
export function removeEntities(level: LevelFile, ids: string[]): Command {
  const wanted = new Set(ids);
  const removed = level.entities.map((e, i) => ({ e, i })).filter(({ e }) => wanted.has(e.id));
  return {
    label: removed.length === 1 ? "删除" : `删除 ${removed.length} 个`,
    apply: (l) => ({ ...l, entities: l.entities.filter((e) => !wanted.has(e.id)) }),
    revert: (l) => {
      const entities = [...l.entities];
      // 按原来的下标从小到大插回去，前面的先就位，后面的下标才对
      for (const { e, i } of removed) entities.splice(Math.min(i, entities.length), 0, e);
      return { ...l, entities };
    },
  };
}

/** 把一组实体放到新位置（路线点一起平移）。`merge` 给了时相邻的同类移动合成一步（方向键微移）。 */
export function moveTo(level: LevelFile, targets: { id: string; x: number; y: number }[], merge?: string): Command {
  const to = new Map(targets.map((t) => [t.id, { x: t.x, y: t.y }]));
  const from = new Map(level.entities.filter((e) => to.has(e.id)).map((e) => [e.id, { x: e.x, y: e.y }]));
  return moves(from, to, merge);
}

type Positions = Map<string, { x: number; y: number }>;
const MOVES = Symbol("moves");
type MoveCommand = Command & { [MOVES]: { from: Positions; to: Positions } };

/** 移动命令：`from` 是撤回去的位置，`to` 是挪到的位置；合并时 from 留最早的、to 用最新的。 */
function moves(from: Positions, to: Positions, merge?: string): MoveCommand {
  const place = (pos: Positions) => (l: LevelFile) =>
    mapEntities(l, (e) => {
      const p = pos.get(e.id);
      if (p === undefined || !from.has(e.id)) return e;
      // 路线点跟着实体平移（撤回时按反方向挪回来）
      const [dx, dy] = [p.x - e.x, p.y - e.y];
      return e.points === undefined ? { ...e, x: p.x, y: p.y } : { ...e, x: p.x, y: p.y, points: e.points.map(([x, y]) => [x + dx, y + dy]) };
    });
  return {
    label: "移动",
    apply: place(to),
    revert: place(from),
    merge,
    [MOVES]: { from, to },
    combine: (next) => {
      const n = (next as Partial<MoveCommand>)[MOVES];
      if (n === undefined) return undefined;
      const f = new Map(from);
      for (const [id, p] of n.from) if (!f.has(id)) f.set(id, p);
      return moves(f, new Map([...to, ...n.to]), merge);
    },
  };
}

/** 改一个实体的字段（名字、尺寸、属性……）；值是 undefined 的字段去掉。 */
export function updateEntity(level: LevelFile, id: string, patch: Partial<Omit<Entity, "id">>): Command {
  const old = level.entities.find((e) => e.id === id);
  const before = Object.fromEntries(Object.keys(patch).map((k) => [k, old?.[k as keyof Entity]])) as Partial<Entity>;
  const set = (p: Partial<Entity>) => (l: LevelFile) => (old === undefined ? l : mapEntities(l, (e) => (e.id === id ? without(e, p) : e)));
  return { label: Object.keys(patch).length === 1 && "name" in patch ? "重命名" : "修改", apply: set(patch), revert: set(before) };
}

/** 把实体移到第 `index` 个（超出的按两头算）。 */
export function reorder(level: LevelFile, id: string, index: number): Command {
  const from = level.entities.findIndex((e) => e.id === id);
  const move = (to: number) => (l: LevelFile) => {
    const at = l.entities.findIndex((e) => e.id === id);
    if (at < 0) return l;
    const rest = l.entities.filter((e) => e.id !== id);
    const i = Math.max(0, Math.min(to, rest.length));
    return { ...l, entities: [...rest.slice(0, i), l.entities[at], ...rest.slice(i)] };
  };
  return { label: "排序", apply: move(index), revert: move(from) };
}

export interface TileChange {
  r: number;
  c: number;
  before: number;
  after: number;
}

/** 改第 `layer` 层的一组格子。 */
export function paintTiles(layer: number, changes: TileChange[]): Command {
  const write = (pick: (c: TileChange) => number) => (l: LevelFile) =>
    mapLayer(l, layer, (t) => {
      const data = t.data.map((row) => [...row]);
      for (const c of changes) if (data[c.r]?.[c.c] !== undefined) data[c.r][c.c] = pick(c);
      return { ...t, data };
    });
  return { label: "画瓦片", apply: write((c) => c.after), revert: write((c) => c.before) };
}

/** 改瓦片层的列数、行数：底边和左边不动，多出来的是空格，切掉的丢掉（撤回时整层还原）。 */
export function resizeLayer(level: LevelFile, layer: number, cols: number, rows: number): Command {
  const old = level.tiles[layer];
  return {
    label: "改大小",
    apply: (l) =>
      mapLayer(l, layer, (t) => {
        const bottom = t.data.slice(Math.max(0, t.rows - rows));
        const pad = Array.from({ length: Math.max(0, rows - t.rows) }, () => Array(cols).fill(-1));
        const data = [...pad, ...bottom.map((row) => [...row.slice(0, cols), ...Array(Math.max(0, cols - t.cols)).fill(-1)])];
        return { ...t, cols, rows, data, y: t.y + (rows - t.rows) * t.tile };
      }),
    revert: (l) => (old === undefined ? l : mapLayer(l, layer, () => old)),
  };
}

/** 改关卡设置（名字、夜里）。 */
export function setLevel(level: LevelFile, patch: Partial<Pick<LevelFile, "name" | "night">>): Command {
  const before = Object.fromEntries(Object.keys(patch).map((k) => [k, level[k as "name" | "night"]]));
  return { label: "关卡设置", apply: (l) => ({ ...l, ...patch }), revert: (l) => ({ ...l, ...before }) };
}

/** 几个命令合成一步撤销。 */
export function compose(label: string, cmds: Command[]): Command {
  return chain(label, cmds);
}
