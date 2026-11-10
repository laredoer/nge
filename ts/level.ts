// 关卡文件：带版本号的 JSON（瓦片层 + 实体列表），读入时校验、保存时格式化（见 docs/editor.md）。
// 这里不知道有哪些实体类型：类型和属性由游戏的类型表给（`checkTypes`）。
import type { TilemapNode } from "./nge";

export const LEVEL_FORMAT = "nge-level";
export const LEVEL_VERSION = 1;
/** 坐标、尺寸的绝对值上限。 */
export const MAX_COORD = 1e6;
export const MAX_COLS = 2000;
export const MAX_ROWS = 500;
export const MAX_ENTITIES = 5000;
const MAX_TEXT = 256;
const MAX_ID = 64;
const MAX_ERRORS = 20;

export type PropValue = number | boolean | string;

/** 瓦片层：左上角在 (x, y)（y 朝上），`data` 有 `rows` 行、每行 `cols` 个帧号，-1 是空格。 */
export interface TileLayer {
  id: string;
  texture: string;
  grid: [number, number];
  tile: number;
  cols: number;
  rows: number;
  x: number;
  y: number;
  solid: number[];
  data: number[][];
  friction?: number;
  z?: number;
}

/** 实体：(x, y) 是中心的世界坐标；`w` / `h` 不写用类型的默认尺寸。 */
export interface Entity {
  id: string;
  type: string;
  name?: string;
  x: number;
  y: number;
  w?: number;
  h?: number;
  props?: Record<string, PropValue>;
}

export interface LevelFile {
  format: typeof LEVEL_FORMAT;
  version: number;
  name: string;
  /** 夜里（游戏自己决定怎么画）。 */
  night: boolean;
  tiles: TileLayer[];
  /** 按场景树的顺序：越往后画在越上面。 */
  entities: Entity[];
}

export type ParseResult = { ok: true; level: LevelFile } | { ok: false; errors: string[] };

/** 一个属性：数（范围）、布尔、几个选项之一、文字。 */
export type PropSpec =
  | { key: string; label: string; kind: "number"; min: number; max: number; default: number; step?: number }
  | { key: string; label: string; kind: "bool"; default: boolean }
  | { key: string; label: string; kind: "choice"; options: string[]; default: string }
  | { key: string; label: string; kind: "text"; default: string; pattern?: RegExp; hint?: string };

/** 一种实体：名字、属性，关卡里最少 / 最多几个。 */
export interface TypeSpec {
  label?: string;
  props: PropSpec[];
  min?: number;
  max?: number;
}
export type TypeSpecs = Record<string, TypeSpec>;

class Errors {
  list: string[] = [];
  add(message: string) {
    if (this.list.length < MAX_ERRORS) this.list.push(message);
  }
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const finite = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

function text(v: unknown, path: string, errors: Errors, { empty = true, max = MAX_TEXT } = {}): string {
  if (typeof v !== "string") {
    errors.add(`${path} 要是字符串`);
    return "";
  }
  // 控制字符（换行、响铃……）不让进
  if (v.length > max || (!empty && v.length === 0) || /[\u0000-\u001f\u007f]/.test(v)) {
    errors.add(`${path} 要是${empty ? "" : "非空、"}不超过 ${max} 个字、没有控制字符的字符串`);
  }
  return v;
}

function coord(v: unknown, path: string, errors: Errors): number {
  if (!finite(v) || Math.abs(v) > MAX_COORD) {
    errors.add(`${path} 要是有限的数，绝对值不超过 ${MAX_COORD}`);
    return 0;
  }
  return v;
}

function positive(v: unknown, path: string, errors: Errors): number {
  if (!finite(v) || v <= 0 || v > MAX_COORD) {
    errors.add(`${path} 要是正数，不超过 ${MAX_COORD}`);
    return 1;
  }
  return v;
}

function int(v: unknown, path: string, errors: Errors, min: number, max: number): number {
  if (!Number.isInteger(v) || (v as number) < min || (v as number) > max) {
    errors.add(`${path} 要是 ${min} 到 ${max} 的整数`);
    return min;
  }
  return v as number;
}

function list(v: unknown, path: string, errors: Errors): unknown[] {
  if (!Array.isArray(v)) {
    errors.add(`${path} 要是数组`);
    return [];
  }
  return v;
}

function layerOf(raw: unknown, path: string, errors: Errors): TileLayer {
  const o = isObject(raw) ? raw : (errors.add(`${path} 要是对象`), {});
  const gridRaw = list(o.grid, `${path}.grid`, errors);
  const grid: [number, number] = [int(gridRaw[0], `${path}.grid[0]`, errors, 1, 256), int(gridRaw[1], `${path}.grid[1]`, errors, 1, 256)];
  const frames = grid[0] * grid[1];
  const cols = int(o.cols, `${path}.cols`, errors, 1, MAX_COLS);
  const rows = int(o.rows, `${path}.rows`, errors, 1, MAX_ROWS);
  const rowsRaw = list(o.data, `${path}.data`, errors);
  if (rowsRaw.length !== rows) errors.add(`${path}.data 要有 ${rows} 行`);
  const data = rowsRaw.map((row, r) => {
    const cells = list(row, `${path}.data[${r}]`, errors);
    if (cells.length !== cols) errors.add(`${path}.data[${r}] 要有 ${cols} 格`);
    return cells.map((c, i) => int(c, `${path}.data[${r}][${i}]`, errors, -1, frames - 1));
  });
  const layer: TileLayer = {
    id: text(o.id, `${path}.id`, errors, { empty: false, max: MAX_ID }),
    texture: text(o.texture, `${path}.texture`, errors, { empty: false }),
    grid,
    tile: positive(o.tile, `${path}.tile`, errors),
    cols,
    rows,
    x: coord(o.x, `${path}.x`, errors),
    y: coord(o.y, `${path}.y`, errors),
    solid: list(o.solid, `${path}.solid`, errors).map((f, i) => int(f, `${path}.solid[${i}]`, errors, 0, frames - 1)),
    data,
  };
  if (o.friction !== undefined) layer.friction = positiveOrZero(o.friction, `${path}.friction`, errors);
  if (o.z !== undefined) layer.z = coord(o.z, `${path}.z`, errors);
  return layer;
}

function flag(v: unknown, path: string, errors: Errors): boolean {
  if (v === undefined) return false;
  if (typeof v !== "boolean") errors.add(`${path} 要是布尔`);
  return v === true;
}

function positiveOrZero(v: unknown, path: string, errors: Errors): number {
  if (!finite(v) || v < 0) {
    errors.add(`${path} 要是不小于 0 的数`);
    return 0;
  }
  return v;
}

function entityOf(raw: unknown, path: string, errors: Errors): Entity {
  const o = isObject(raw) ? raw : (errors.add(`${path} 要是对象`), {});
  const entity: Entity = {
    id: text(o.id, `${path}.id`, errors, { empty: false, max: MAX_ID }),
    type: text(o.type, `${path}.type`, errors, { empty: false, max: MAX_ID }),
    x: coord(o.x, `${path}.x`, errors),
    y: coord(o.y, `${path}.y`, errors),
  };
  if (o.name !== undefined) entity.name = text(o.name, `${path}.name`, errors);
  if (o.w !== undefined) entity.w = positive(o.w, `${path}.w`, errors);
  if (o.h !== undefined) entity.h = positive(o.h, `${path}.h`, errors);
  if (o.props !== undefined) {
    if (!isObject(o.props)) errors.add(`${path}.props 要是对象`);
    else {
      const props: Record<string, PropValue> = {};
      for (const [key, v] of Object.entries(o.props)) {
        const at = `${path}.props.${key}`;
        if (typeof v === "boolean") props[key] = v;
        else if (typeof v === "string") props[key] = text(v, at, errors);
        else if (finite(v) && Math.abs(v) <= MAX_COORD) props[key] = v;
        else errors.add(`${at} 只能是有限的数、布尔或字符串`);
      }
      entity.props = props;
    }
  }
  return entity;
}

/** 读关卡文件：`input` 是文件内容或解析过的值。坏数据不抛异常，返回带路径的错误（最多 20 条）。 */
export function parseLevel(input: unknown): ParseResult {
  let raw = input;
  if (typeof input === "string") {
    try {
      raw = JSON.parse(input);
    } catch (e) {
      return { ok: false, errors: [`不是合法的 JSON：${e instanceof Error ? e.message : String(e)}`] };
    }
  }
  if (!isObject(raw)) return { ok: false, errors: ["关卡文件不是对象"] };
  if (raw.format !== LEVEL_FORMAT) return { ok: false, errors: [`format 要是 "${LEVEL_FORMAT}"`] };
  if (!Number.isInteger(raw.version) || (raw.version as number) < 1) return { ok: false, errors: ["version 要是正整数"] };
  if ((raw.version as number) > LEVEL_VERSION) return { ok: false, errors: [`version ${raw.version} 是更新的编辑器存的，这里只认到 ${LEVEL_VERSION}`] };
  const errors = new Errors();
  const tiles = list(raw.tiles, "tiles", errors).map((t, i) => layerOf(t, `tiles[${i}]`, errors));
  const entitiesRaw = list(raw.entities, "entities", errors);
  if (entitiesRaw.length > MAX_ENTITIES) return { ok: false, errors: [`entities 最多 ${MAX_ENTITIES} 个，这里有 ${entitiesRaw.length} 个`] };
  const entities = entitiesRaw.map((e, i) => entityOf(e, `entities[${i}]`, errors));
  const seen = new Map<string, number>();
  entities.forEach((e, i) => {
    const first = seen.get(e.id);
    if (first !== undefined) errors.add(`entities[${i}].id 和 entities[${first}] 重复：${e.id}`);
    else seen.set(e.id, i);
  });
  const level: LevelFile = {
    format: LEVEL_FORMAT,
    version: LEVEL_VERSION,
    name: raw.name === undefined ? "" : text(raw.name, "name", errors),
    night: flag(raw.night, "night", errors),
    tiles,
    entities,
  };
  return errors.list.length > 0 ? { ok: false, errors: errors.list } : { ok: true, level };
}

/** 一个属性值合不合规：合规返回 null，不然返回原因。 */
export function checkProp(spec: PropSpec, value: unknown): string | null {
  switch (spec.kind) {
    case "number":
      if (!finite(value)) return "要是数";
      return value < spec.min || value > spec.max ? `要在 ${spec.min} 到 ${spec.max} 之间` : null;
    case "bool":
      return typeof value === "boolean" ? null : "要是布尔";
    case "choice":
      return typeof value === "string" && spec.options.includes(value) ? null : `只能是 ${spec.options.join("、")}`;
    case "text":
      if (typeof value !== "string") return "要是文字";
      return spec.pattern === undefined || spec.pattern.test(value) ? null : `要是${spec.hint ?? "合规的文字"}`;
  }
}

/** 按游戏的类型表查实体：类型、属性、每种的数量。返回错误列表（空的是没问题）。 */
export function checkTypes(level: LevelFile, types: TypeSpecs): string[] {
  const errors = new Errors();
  const counts = new Map<string, number>();
  for (const e of level.entities) {
    const spec = types[e.type];
    if (spec === undefined) {
      errors.add(`${e.id}：不认识的类型 ${e.type}`);
      continue;
    }
    counts.set(e.type, (counts.get(e.type) ?? 0) + 1);
    for (const [key, value] of Object.entries(e.props ?? {})) {
      const prop = spec.props.find((p) => p.key === key);
      if (prop === undefined) {
        errors.add(`${e.id}：不认识的属性 ${key}`);
        continue;
      }
      const why = checkProp(prop, value);
      if (why !== null) errors.add(`${e.id} 的 ${prop.label}${why}`);
    }
  }
  for (const [type, spec] of Object.entries(types)) {
    const n = counts.get(type) ?? 0;
    const label = spec.label ?? type;
    if (spec.min !== undefined && spec.min === spec.max && n !== spec.min) errors.add(`${label}要有 ${spec.min} 个，现在 ${n} 个`);
    else if (spec.min !== undefined && n < spec.min) errors.add(`${label}至少要 ${spec.min} 个，现在 ${n} 个`);
    else if (spec.max !== undefined && n > spec.max) errors.add(`${label}最多 ${spec.max} 个，现在 ${n} 个`);
  }
  return errors.list;
}

/** 一行写完的 JSON 值：逗号、冒号后面空一格，字符串照 JSON 转义。 */
function inline(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(inline).join(", ")}]`;
  if (isObject(v)) {
    const fields = Object.entries(v).map(([k, x]) => `${JSON.stringify(k)}: ${inline(x)}`);
    return fields.length === 0 ? "{}" : `{ ${fields.join(", ")} }`;
  }
  return JSON.stringify(v);
}

/** 保存用的文本：两个空格缩进，瓦片一行一行，实体一个一行，结尾换行。 */
export function formatLevel(level: LevelFile): string {
  const layer = (t: TileLayer) => {
    const { data, ...rest } = t;
    const head = Object.entries(rest).map(([k, v]) => `      ${JSON.stringify(k)}: ${inline(v)}`);
    const rows = data.map((row) => `        [${row.join(", ")}]`).join(",\n");
    return `    {\n${head.join(",\n")},\n      "data": [\n${rows}\n      ]\n    }`;
  };
  const block = (items: string[]) => (items.length === 0 ? "[]" : `[\n${items.join(",\n")}\n  ]`);
  return [
    "{",
    `  "format": ${JSON.stringify(level.format)},`,
    `  "version": ${level.version},`,
    `  "name": ${JSON.stringify(level.name)},`,
    `  "night": ${level.night},`,
    `  "tiles": ${block(level.tiles.map(layer))},`,
    `  "entities": ${block(level.entities.map((e) => `    ${inline(e)}`))}`,
    "}",
    "",
  ].join("\n");
}

/** 新建关卡：一层全空的瓦片，底行的底边在 y = 0 往下一格（底行的顶边在 y = 0）。 */
export function emptyLevel(o: { name: string; cols: number; rows: number; tile: number; texture: string; grid: [number, number]; solid: number[] }): LevelFile {
  return {
    format: LEVEL_FORMAT,
    version: LEVEL_VERSION,
    name: o.name,
    night: false,
    tiles: [{ id: "level", texture: o.texture, grid: o.grid, tile: o.tile, cols: o.cols, rows: o.rows, x: 0, y: (o.rows - 1) * o.tile, solid: o.solid, data: Array.from({ length: o.rows }, () => Array(o.cols).fill(-1)) }],
    entities: [],
  };
}

/** 瓦片层 → 引擎的瓦片地图。 */
export function toTilemap(t: TileLayer): TilemapNode {
  const { data, rows: _rows, ...rest } = t;
  return { ...rest, data: data.flat() };
}
