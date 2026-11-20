// 画布上的指针（docs/editor.md、docs/editor-more.md）：按下 / 移动 / 松开 → 选择、移动、框选、平移、放置、瓦片刷、矩形、油漆桶、拖路线点。
// 纯函数，返回要改的东西。
import type { Entity, LevelFile } from "../../ts/level";
import { screenToWorld, type CameraView } from "../../ts/nge";
import { nextId } from "./clipboard";
import { addEntities, moveTo, paintTiles, updateEntity } from "./commands";
import { changesOf, floodFill, rectCells } from "./fill";
import { withDescendants } from "./hierarchy";
import type { Command } from "./history";
import { layerPoint } from "./layers";
import { movePoint, pointAt } from "./points";
import { boxSelect, hitTest, type BoundsOf } from "./select";
import { snapPoint } from "./snap";
import { cellAt, cellsBetween, layerWith, NEW_STROKE, strokeAdd, type Cell, type Stroke } from "./tiles";
import { pan } from "./view";

export type ToolName = "select" | "pan" | "place" | "brush" | "erase" | "rect" | "fill";

/** 拖不到这么远（屏幕 point）算点击。 */
export const CLICK_SLOP = 3;
/** 点选时最小的可点半径（屏幕 point）。 */
const PICK_SLOP = 4;
/** 路线点按得到的半径（屏幕 point）。 */
export const POINT_SLOP = 8;

export interface Ctx {
  level: LevelFile;
  selection: string[];
  view: CameraView;
  /** 游戏视图的宽高。 */
  size: [number, number];
  /** 网格边长，0 不吸附。 */
  grid: number;
  tool: ToolName;
  /** 放置工具放什么类型。 */
  placeType: string | null;
  /** 瓦片刷画哪一帧。 */
  frame: number;
  /** 画哪一层瓦片。 */
  layer: number;
  boundsOf: BoundsOf;
  /** 画的先后（点选时深的先命中）；不给按列表顺序。 */
  depthOf?: (e: Entity, index: number) => number;
  sizeOf(e: Entity): [number, number];
  /** 放到 (x, y)（站在地上的类型 y 固定）。 */
  place(e: Entity, x: number, y: number): Entity;
  make(type: string, id: string, x: number, y: number): Entity;
}

type Pos = { id: string; x: number; y: number };
export type Drag =
  | { kind: "idle" }
  | { kind: "press"; start: [number, number]; hit: string | null; tool: ToolName }
  | { kind: "move"; start: [number, number]; lead: string; origins: Pos[]; targets: Pos[] }
  | { kind: "box"; start: [number, number]; end: [number, number] }
  | { kind: "pan"; start: [number, number]; view: CameraView }
  | { kind: "paint"; last: Cell | null; stroke: Stroke }
  | { kind: "rect"; start: Cell; end: Cell }
  | { kind: "point"; id: string; index: number; start: [number, number]; origin: [number, number]; to: [number, number] };

export const IDLE: Drag = { kind: "idle" };

export interface Out {
  drag: Drag;
  selection?: string[];
  view?: CameraView;
  command?: Command;
  /** 状态栏要说的（画不上的原因）。 */
  note?: string;
}

const world = (c: Ctx, p: [number, number]) => screenToWorld(c.view, c.size, p);
const far = (a: [number, number], b: [number, number]) => Math.hypot(a[0] - b[0], a[1] - b[1]) >= CLICK_SLOP;
const frameOf = (c: Ctx) => (c.tool === "erase" ? -1 : c.frame);

/** 屏幕点 `p` 在当前层上的格子（视差层按画出来的位置算）。 */
function cellOf(c: Ctx, p: [number, number]): Cell | null {
  const layer = c.level.tiles[c.layer];
  return layer === undefined ? null : cellAt(layer, layerPoint(layer, c.view, world(c, p)));
}

/** 当前层画不上的原因（没有、锁着、藏着），能画是 null。 */
export function blockedLayer(c: Ctx): string | null {
  const layer = c.level.tiles[c.layer];
  if (layer === undefined) return "没有瓦片层";
  if (layer.locked === true) return `「${layer.name ?? layer.id}」锁着，画不上`;
  if (layer.hidden === true) return `「${layer.name ?? layer.id}」藏着，先显示出来再画`;
  return null;
}

/** 刷子在屏幕点 `p` 下的格子，接着上一格补齐。 */
function paint(c: Ctx, drag: Extract<Drag, { kind: "paint" }>, p: [number, number]): Drag {
  const layer = c.level.tiles[c.layer];
  const cell = cellOf(c, p);
  if (layer === undefined || cell === null) return { ...drag, last: null };
  const cells = drag.last === null ? [cell] : cellsBetween(drag.last, cell);
  return { kind: "paint", last: cell, stroke: strokeAdd(drag.stroke, layer, cells, frameOf(c)) };
}

export function pointerDown(c: Ctx, p: [number, number]): Out {
  switch (c.tool) {
    case "pan":
      return { drag: { kind: "pan", start: p, view: c.view } };
    case "brush":
    case "erase":
    case "rect":
    case "fill": {
      const why = blockedLayer(c);
      if (why !== null) return { drag: IDLE, note: why };
      if (c.tool === "fill") return { drag: { kind: "press", start: p, hit: null, tool: "fill" } };
      if (c.tool === "rect") {
        const cell = cellOf(c, p);
        return { drag: cell === null ? IDLE : { kind: "rect", start: cell, end: cell } };
      }
      return { drag: paint(c, { kind: "paint", last: null, stroke: NEW_STROKE }, p) };
    }
    case "place":
      return { drag: { kind: "press", start: p, hit: null, tool: "place" } };
    case "select": {
      // 选中的东西的路线点在上面：先看是不是按在点上
      const point = pointAt(c.level.entities, c.selection, world(c, p), POINT_SLOP / c.view.zoom);
      if (point !== null) {
        const origin = c.level.entities.find((e) => e.id === point.id)!.points![point.index];
        return { drag: { kind: "point", ...point, start: p, origin, to: origin } };
      }
      const hit = hitTest(c.level.entities, c.boundsOf, world(c, p), PICK_SLOP / c.view.zoom, c.depthOf);
      // 按在没选中的东西上：先只选中它（接着拖就是拖它）
      const selection = hit !== null && !c.selection.includes(hit) ? [hit] : undefined;
      return { drag: { kind: "press", start: p, hit, tool: "select" }, selection };
    }
  }
}

/** 移动：抓着的那个按网格吸附，别的挪一样多；站在地上的只改 x。 */
function moved(c: Ctx, drag: Extract<Drag, { kind: "move" }>, p: [number, number]): Drag {
  const [sx, sy] = world(c, drag.start);
  const [wx, wy] = world(c, p);
  const lead = drag.origins.find((o) => o.id === drag.lead);
  const leadEntity = c.level.entities.find((e) => e.id === drag.lead);
  // 拖着的东西没了（被删掉、撤销了）：拖动作废
  if (lead === undefined || leadEntity === undefined) return IDLE;
  const [tx, ty] = snapPoint([lead.x + wx - sx, lead.y + wy - sy], c.grid, c.sizeOf(leadEntity));
  const [dx, dy] = [tx - lead.x, ty - lead.y];
  const targets = drag.origins.flatMap((o) => {
    const e = c.level.entities.find((x) => x.id === o.id);
    if (e === undefined) return [];
    const placed = c.place(e, o.x + dx, o.y + dy);
    return [{ id: o.id, x: placed.x, y: placed.y }];
  });
  return { ...drag, targets };
}

export function pointerMove(c: Ctx, drag: Drag, p: [number, number]): Out {
  switch (drag.kind) {
    case "pan":
      return { drag, view: pan(drag.view, p[0] - drag.start[0], p[1] - drag.start[1]) };
    case "paint":
      return { drag: paint(c, drag, p) };
    case "box":
      return { drag: { ...drag, end: world(c, p) } };
    case "move":
      return { drag: moved(c, drag, p) };
    case "rect": {
      const cell = cellOf(c, p);
      return { drag: cell === null ? drag : { ...drag, end: cell } };
    }
    case "point": {
      const entity = c.level.entities.find((e) => e.id === drag.id);
      // 点所在的实体没了、点被删了：拖动作废
      if (entity?.points?.[drag.index] === undefined) return { drag: IDLE };
      const [sx, sy] = world(c, drag.start);
      const [wx, wy] = world(c, p);
      // 路线点按实体自己的大小吸附：和实体放在同一种格子位置上
      const to = snapPoint([drag.origin[0] + wx - sx, drag.origin[1] + wy - sy], c.grid, c.sizeOf(entity));
      return { drag: { ...drag, to } };
    }
    case "press": {
      if (drag.tool !== "select" || !far(drag.start, p)) return { drag };
      if (drag.hit === null) return { drag: { kind: "box", start: world(c, drag.start), end: world(c, p) } };
      // 拖父节点时子孙跟着走
      const ids = withDescendants(c.level, c.selection.includes(drag.hit) ? c.selection : [drag.hit]);
      const origins = c.level.entities.filter((e) => ids.includes(e.id)).map((e) => ({ id: e.id, x: e.x, y: e.y }));
      return { drag: moved(c, { kind: "move", start: drag.start, lead: drag.hit, origins, targets: origins }, p) };
    }
    case "idle":
      return { drag };
  }
}

export function pointerUp(c: Ctx, drag: Drag, p: [number, number]): Out {
  const last = pointerMove(c, drag, p).drag;
  switch (last.kind) {
    case "pan":
      return { drag: IDLE, view: pan(last.view, p[0] - last.start[0], p[1] - last.start[1]) };
    case "paint":
      return { drag: IDLE, command: last.stroke.changes.length === 0 ? undefined : paintTiles(c.layer, last.stroke.changes) };
    case "rect": {
      const layer = c.level.tiles[c.layer];
      const changes = layer === undefined ? [] : changesOf(layer, rectCells(last.start, last.end), c.frame);
      return { drag: IDLE, command: changes.length === 0 ? undefined : paintTiles(c.layer, changes) };
    }
    case "point": {
      const e = c.level.entities.find((x) => x.id === last.id);
      const same = last.to[0] === last.origin[0] && last.to[1] === last.origin[1];
      return { drag: IDLE, command: e === undefined || same ? undefined : updateEntity(c.level, e.id, { points: movePoint(e, last.index, last.to[0], last.to[1]) }) };
    }
    case "box":
      return { drag: IDLE, selection: boxSelect(c.level.entities, c.boundsOf, last.start, last.end) };
    case "move": {
      const changed = last.targets.some((t) => {
        const o = last.origins.find((x) => x.id === t.id);
        return o === undefined || t.x !== o.x || t.y !== o.y;
      });
      return { drag: IDLE, command: changed ? moveTo(c.level, last.targets) : undefined };
    }
    case "press": {
      if (last.tool === "fill") {
        const layer = c.level.tiles[c.layer];
        const cell = cellOf(c, p);
        const changes = layer === undefined || cell === null ? [] : floodFill(layer, cell, c.frame);
        return { drag: IDLE, command: changes.length === 0 ? undefined : paintTiles(c.layer, changes) };
      }
      if (last.tool === "place") {
        if (c.placeType === null) return { drag: IDLE };
        const id = nextId(c.level);
        const draft = c.make(c.placeType, id, 0, 0);
        const [x, y] = snapPoint(world(c, p), c.grid, c.sizeOf(draft));
        return { drag: IDLE, command: addEntities([c.place(draft, x, y)]), selection: [id] };
      }
      // 点击：点中的只留它，点空白清空
      return { drag: IDLE, selection: last.hit === null ? [] : [last.hit] };
    }
    case "idle":
      return { drag: IDLE };
  }
}

/** 指针在 `p`（没按着）时下面的实体：选择工具才有。 */
export function hoverOf(c: Ctx, p: [number, number]): string | null {
  return c.tool === "select" ? hitTest(c.level.entities, c.boundsOf, world(c, p), PICK_SLOP / c.view.zoom, c.depthOf) : null;
}

/** 拖动中画布上显示的关卡：移动中的实体在新位置，正在画的瓦片画上去。 */
export function previewOf(c: Ctx, drag: Drag): LevelFile {
  if (drag.kind === "move") return moveTo(c.level, drag.targets).apply(c.level);
  if (drag.kind === "point") {
    const e = c.level.entities.find((x) => x.id === drag.id);
    return e?.points?.[drag.index] === undefined ? c.level : updateEntity(c.level, e.id, { points: movePoint(e, drag.index, drag.to[0], drag.to[1]) }).apply(c.level);
  }
  if (drag.kind === "rect" && c.level.tiles[c.layer] !== undefined) {
    const changes = changesOf(c.level.tiles[c.layer], rectCells(drag.start, drag.end), c.frame);
    return { ...c.level, tiles: c.level.tiles.map((t, i) => (i === c.layer ? layerWith(t, { changes }) : t)) };
  }
  if (drag.kind === "paint" && c.level.tiles[c.layer] !== undefined) {
    return { ...c.level, tiles: c.level.tiles.map((t, i) => (i === c.layer ? layerWith(t, drag.stroke) : t)) };
  }
  return c.level;
}
