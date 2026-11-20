// 关卡文件：带版本号的 JSON（瓦片层 + 实体列表），读入时迁移到现在的版本、校验，保存时格式化（见 docs/editor.md、docs/editor-more.md）。
// 这里不知道有哪些实体类型：类型和属性由游戏的类型表给（`checkTypes`）。
import type { TilemapNode } from "./nge";

export const LEVEL_FORMAT = "nge-level";
export const LEVEL_VERSION = 2;
/** 坐标、尺寸的绝对值上限。 */
export const MAX_COORD = 1e6;
export const MAX_COLS = 2000;
export const MAX_ROWS = 500;
export const MAX_ENTITIES = 5000;
export const MAX_LAYERS = 16;
/** 一个实体最多几个路线点。 */
export const MAX_POINTS = 64;
/** 预制体覆盖最多几个子节点。 */
export const MAX_OVERRIDES = 32;
/** 视差的上限（0 是不动的远景，1 是跟着关卡走）。 */
export const MAX_PARALLAX = 4;
/** 旋转（度）的绝对值上限。 */
export const MAX_ROTATION = 3600;
/** 实体 id：字母、数字、- 和 _（编辑器的预览节点用 `ed:<id>:<部件>`，id 里不能有冒号）。 */
const ID = /^[A-Za-z0-9_-]{1,64}$/;
/** 属性名：字母开头的字母、数字、下划线（`__proto__` 这种进不来）。 */
const PROP_KEY = /^[A-Za-z][A-Za-z0-9_]{0,31}$/;
/** 预制体里子节点的路径：id 用 / 连起来（嵌套的预制体），最多 8 段；不能有 `__proto__` 这一段（写进对象会被吞掉）。 */
export const CHILD_PATH = /^(?!(?:[^/]*\/)*__proto__(?:\/|$))[A-Za-z0-9_-]{1,64}(\/[A-Za-z0-9_-]{1,64}){0,7}$/;
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
  /** 单向平台、斜坡、动画瓦片、每种瓦片的材质的帧号（同 `TilemapNode`，见 docs/gameplay2d.md）。 */
  oneWay?: number[];
  slopes?: TileSlope[];
  animated?: TileAnimation[];
  materials?: TileMaterial[];
  /** 第 2 版：层面板上的名字、编辑器里隐藏 / 锁定（游戏不管）、视差 `[x, y]`。 */
  name?: string;
  hidden?: boolean;
  locked?: boolean;
  parallax?: [number, number];
}

export interface TileSlope {
  frame: number;
  left: number;
  right: number;
}

export interface TileAnimation {
  frame: number;
  frames: number[];
  fps: number;
}

export interface TileMaterial {
  frame: number;
  friction?: number;
  restitution?: number;
}

/** 瓦片层里按帧号的规则（新建关卡时从图集抄过来）。 */
export type TileRules = Pick<TileLayer, "oneWay" | "slopes" | "animated" | "materials">;

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
  /** 第 2 版：旋转（度）、z、父节点（只是层级，坐标还是世界坐标）、路线点、预制体按子节点的覆盖。 */
  rotation?: number;
  z?: number;
  parent?: string;
  points?: [number, number][];
  set?: Record<string, Record<string, PropValue>>;
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
  // 行数、格数对不上就不往下查每一格（坏文件可能很大）
  const shapeOk = rowsRaw.length === rows;
  if (!shapeOk) errors.add(`${path}.data 要有 ${rows} 行`);
  const data = !shapeOk
    ? []
    : rowsRaw.map((row, r) => {
        const cells = list(row, `${path}.data[${r}]`, errors);
        if (cells.length !== cols) {
          errors.add(`${path}.data[${r}] 要有 ${cols} 格`);
          return [];
        }
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
  Object.assign(layer, rulesOf(o, path, frames, errors));
  if (o.name !== undefined) layer.name = text(o.name, `${path}.name`, errors, { max: MAX_ID });
  if (o.hidden !== undefined) layer.hidden = flag(o.hidden, `${path}.hidden`, errors);
  if (o.locked !== undefined) layer.locked = flag(o.locked, `${path}.locked`, errors);
  if (o.parallax !== undefined) {
    const raw = list(o.parallax, `${path}.parallax`, errors);
    if (raw.length !== 2) errors.add(`${path}.parallax 要是两个数 [x, y]`);
    const axis = (v: unknown, i: number) => {
      if (!finite(v) || v < 0 || v > MAX_PARALLAX) errors.add(`${path}.parallax[${i}] 要是 0 到 ${MAX_PARALLAX} 的数`);
      return finite(v) ? v : 1;
    };
    layer.parallax = [axis(raw[0], 0), axis(raw[1], 1)];
  }
  return layer;
}

/** 单向平台、斜坡、动画瓦片、材质：可选，写了就逐项查。 */
function rulesOf(o: Record<string, unknown>, path: string, frames: number, errors: Errors): TileRules {
  const frame = (v: unknown, p: string) => int(v, p, errors, 0, frames - 1);
  const items = (key: string) => list(o[key], `${path}.${key}`, errors).map((raw, i) => [isObject(raw) ? raw : (errors.add(`${path}.${key}[${i}] 要是对象`), {}), `${path}.${key}[${i}]`] as const);
  const height = (v: unknown, p: string) => {
    if (!finite(v) || v < 0 || v > 1) errors.add(`${p} 要是 0 到 1 的数`);
    return finite(v) ? v : 0;
  };
  const rules: TileRules = {};
  if (o.oneWay !== undefined) rules.oneWay = list(o.oneWay, `${path}.oneWay`, errors).map((f, i) => frame(f, `${path}.oneWay[${i}]`));
  if (o.slopes !== undefined) {
    rules.slopes = items("slopes").map(([s, p]) => {
      const slope = { frame: frame(s.frame, `${p}.frame`), left: height(s.left, `${p}.left`), right: height(s.right, `${p}.right`) };
      if (slope.left === 0 && slope.right === 0) errors.add(`${p} 的 left、right 不能都是 0`);
      return slope;
    });
  }
  if (o.animated !== undefined) {
    rules.animated = items("animated").map(([a, p]) => {
      const seq = list(a.frames, `${p}.frames`, errors).map((f, i) => frame(f, `${p}.frames[${i}]`));
      if (seq.length === 0) errors.add(`${p}.frames 不能是空的`);
      return { frame: frame(a.frame, `${p}.frame`), frames: seq, fps: positive(a.fps, `${p}.fps`, errors) };
    });
  }
  if (o.materials !== undefined) {
    rules.materials = items("materials").map(([m, p]) => {
      const material: TileMaterial = { frame: frame(m.frame, `${p}.frame`) };
      if (m.friction !== undefined) material.friction = positiveOrZero(m.friction, `${p}.friction`, errors);
      if (m.restitution !== undefined) material.restitution = positiveOrZero(m.restitution, `${p}.restitution`, errors);
      return material;
    });
  }
  return rules;
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

function idOf(v: unknown, path: string, errors: Errors): string {
  if (typeof v !== "string" || !ID.test(v)) {
    errors.add(`${path} 要是 1 到 ${MAX_ID} 个字母、数字、- 或 _`);
    return typeof v === "string" ? v : "";
  }
  return v;
}

function entityOf(raw: unknown, path: string, errors: Errors): Entity {
  const o = isObject(raw) ? raw : (errors.add(`${path} 要是对象`), {});
  const entity: Entity = {
    id: idOf(o.id, `${path}.id`, errors),
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
        if (!PROP_KEY.test(key)) errors.add(`${path}.props 的属性名 ${JSON.stringify(key.slice(0, 40))} 要是字母开头的字母、数字、下划线，不超过 32 个字`);
        else if (typeof v === "boolean") props[key] = v;
        else if (typeof v === "string") props[key] = text(v, at, errors);
        else if (finite(v) && Math.abs(v) <= MAX_COORD) props[key] = v;
        else errors.add(`${at} 只能是有限的数、布尔或字符串`);
      }
      entity.props = props;
    }
  }
  if (o.rotation !== undefined) {
    if (!finite(o.rotation) || Math.abs(o.rotation) > MAX_ROTATION) errors.add(`${path}.rotation 要是有限的数，绝对值不超过 ${MAX_ROTATION}`);
    entity.rotation = finite(o.rotation) ? o.rotation : 0;
  }
  if (o.z !== undefined) entity.z = coord(o.z, `${path}.z`, errors);
  if (o.parent !== undefined) entity.parent = idOf(o.parent, `${path}.parent`, errors);
  if (o.points !== undefined) {
    const raw = list(o.points, `${path}.points`, errors);
    if (raw.length > MAX_POINTS) errors.add(`${path}.points 最多 ${MAX_POINTS} 个`);
    entity.points = raw.slice(0, MAX_POINTS).map((p, i) => {
      const at = `${path}.points[${i}]`;
      if (!Array.isArray(p) || p.length !== 2) {
        errors.add(`${at} 要是 [x, y]`);
        return [0, 0];
      }
      return [coord(p[0], `${at}[0]`, errors), coord(p[1], `${at}[1]`, errors)];
    });
  }
  if (o.set !== undefined) entity.set = overridesOf(o.set, `${path}.set`, errors);
  return entity;
}

/** 预制体覆盖：`{ 子节点路径: { 字段: 值 } }`，字段名、值和属性一样的规则。 */
function overridesOf(raw: unknown, path: string, errors: Errors): Record<string, Record<string, PropValue>> {
  if (!isObject(raw)) {
    errors.add(`${path} 要是对象`);
    return {};
  }
  const out: Record<string, Record<string, PropValue>> = {};
  const entries = Object.entries(raw);
  if (entries.length > MAX_OVERRIDES) errors.add(`${path} 最多 ${MAX_OVERRIDES} 个子节点`);
  for (const [child, fields] of entries.slice(0, MAX_OVERRIDES)) {
    if (!CHILD_PATH.test(child)) {
      errors.add(`${path} 的子节点 ${JSON.stringify(child.slice(0, 40))} 要是 id 用 / 连起来（最多 8 段，不能有 __proto__ 这一段）`);
      continue;
    }
    const at = `${path}.${child}`;
    if (!isObject(fields)) {
      errors.add(`${at} 要是对象`);
      continue;
    }
    const values: Record<string, PropValue> = {};
    for (const [key, v] of Object.entries(fields)) {
      if (!PROP_KEY.test(key)) errors.add(`${at} 的字段名 ${JSON.stringify(key.slice(0, 40))} 要是字母开头的字母、数字、下划线`);
      else if (typeof v === "boolean") values[key] = v;
      else if (typeof v === "string") values[key] = text(v, `${at}.${key}`, errors);
      else if (finite(v) && Math.abs(v) <= MAX_COORD) values[key] = v;
      else errors.add(`${at}.${key} 只能是有限的数、布尔或字符串`);
    }
    out[child] = values;
  }
  return out;
}

/** 父节点：要指向关卡里有的、不是自己、不绕成圈。 */
function checkParents(entities: Entity[], errors: Errors) {
  const byId = new Map(entities.map((e) => [e.id, e]));
  entities.forEach((e, i) => {
    if (e.parent === undefined) return;
    if (e.parent === e.id) return errors.add(`entities[${i}].parent 不能是自己`);
    if (!byId.has(e.parent)) return errors.add(`entities[${i}].parent 指向的 ${e.parent} 不在关卡里`);
    // 往上走：走的步数超过实体数就是绕成了圈
    let at: Entity | undefined = e;
    for (let steps = 0; at?.parent !== undefined; steps++) {
      if (steps > entities.length) return errors.add(`entities[${i}].parent 绕成了圈（${e.id}）`);
      at = byId.get(at.parent);
    }
  });
}

/** 第 1 版的读法认识的字段：迁到第 2 版时别的都丢掉（第 1 版读的时候本来就不算它们）。 */
const V1_LAYER = ["id", "texture", "grid", "tile", "cols", "rows", "x", "y", "solid", "data", "friction", "z", "oneWay", "slopes", "animated", "materials"];
const V1_ENTITY = ["id", "type", "name", "x", "y", "w", "h", "props"];
const pick = (v: unknown, keys: string[]) => (isObject(v) ? Object.fromEntries(Object.entries(v).filter(([k]) => keys.includes(k))) : v);

/** `LEVEL_MIGRATIONS[n]`：第 n 版的原始对象 → 第 n + 1 版。不改传进来的对象。 */
export const LEVEL_MIGRATIONS: Record<number, (raw: Record<string, unknown>) => Record<string, unknown>> = {
  1: (raw) => ({
    ...raw,
    version: 2,
    tiles: Array.isArray(raw.tiles) ? raw.tiles.map((t) => pick(t, V1_LAYER)) : raw.tiles,
    entities: Array.isArray(raw.entities) ? raw.entities.map((e) => pick(e, V1_ENTITY)) : raw.entities,
  }),
};

/** 从文件的版本一步步迁到现在的版本（版本号不对的原样返回，交给 `parseLevel` 报）。 */
export function migrateLevel(raw: Record<string, unknown>): Record<string, unknown> {
  let out = raw;
  while (Number.isInteger(out.version) && (out.version as number) < LEVEL_VERSION) {
    const step = LEVEL_MIGRATIONS[out.version as number];
    if (step === undefined) break;
    out = step(out);
  }
  return out;
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
  const migrated = migrateLevel(raw);
  const errors = new Errors();
  const tilesRaw = list(migrated.tiles, "tiles", errors);
  if (tilesRaw.length > MAX_LAYERS) return { ok: false, errors: [`tiles 最多 ${MAX_LAYERS} 层，这里有 ${tilesRaw.length} 层`] };
  const tiles = tilesRaw.map((t, i) => layerOf(t, `tiles[${i}]`, errors));
  const layerIds = new Map<string, number>();
  tiles.forEach((t, i) => {
    const first = layerIds.get(t.id);
    if (first !== undefined) errors.add(`tiles[${i}].id 和 tiles[${first}] 重复：${t.id}`);
    else layerIds.set(t.id, i);
  });
  const entitiesRaw = list(migrated.entities, "entities", errors);
  if (entitiesRaw.length > MAX_ENTITIES) return { ok: false, errors: [`entities 最多 ${MAX_ENTITIES} 个，这里有 ${entitiesRaw.length} 个`] };
  const entities = entitiesRaw.map((e, i) => entityOf(e, `entities[${i}]`, errors));
  const seen = new Map<string, number>();
  entities.forEach((e, i) => {
    const first = seen.get(e.id);
    if (first !== undefined) errors.add(`entities[${i}].id 和 entities[${first}] 重复：${e.id}`);
    else seen.set(e.id, i);
  });
  checkParents(entities, errors);
  const level: LevelFile = {
    format: LEVEL_FORMAT,
    version: LEVEL_VERSION,
    name: migrated.name === undefined ? "" : text(migrated.name, "name", errors),
    night: flag(migrated.night, "night", errors),
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
export function emptyLevel(o: { name: string; cols: number; rows: number; tile: number; texture: string; grid: [number, number]; solid: number[] } & TileRules): LevelFile {
  const rules: TileRules = {};
  for (const key of ["oneWay", "slopes", "animated", "materials"] as const) if (o[key] !== undefined) Object.assign(rules, { [key]: o[key] });
  return {
    format: LEVEL_FORMAT,
    version: LEVEL_VERSION,
    name: o.name,
    night: false,
    tiles: [{ id: "level", texture: o.texture, grid: o.grid, tile: o.tile, cols: o.cols, rows: o.rows, x: 0, y: (o.rows - 1) * o.tile, solid: o.solid, ...rules, data: Array.from({ length: o.rows }, () => Array(o.cols).fill(-1)) }],
    entities: [],
  };
}

/** 没写 z 的层按层的顺序叠：第 i 层（从 0 数）的 z 是 i 乘这个（很小，不会盖过 z 是 1 的精灵）。 */
export const LAYER_Z_STEP = 0.001;

/** 第 `index` 层瓦片 → 引擎的瓦片地图。没写 z 的按层的顺序叠（第一层不写 z，和以前一样）。 */
export function toTilemap(t: TileLayer, index = 0): TilemapNode {
  // 名字、显隐、锁定只给编辑器用
  const { data, rows: _rows, name: _name, hidden: _hidden, locked: _locked, ...rest } = t;
  const map: TilemapNode = { ...rest, data: data.flat() };
  return index > 0 && t.z === undefined ? { ...map, z: index * LAYER_Z_STEP } : map;
}
