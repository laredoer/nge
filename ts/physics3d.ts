// 3D 空间查询（见 docs/gameplay3d.md）：经引擎的 `query3d` command 查射线、形状投射、重叠、最近点；结果是 Promise。
import type { NativeComponentRef } from "nsh";

type V3 = [number, number, number];

/** 查询的形状：球（半径）、竖着的胶囊（[中间高, 半径]）、盒子（长宽高）。 */
export type QueryShape3d = { ball: number } | { capsule: [number, number] } | { box: V3 };

export interface Query3dOptions {
  /** 不算这个模型（通常是自己）。 */
  exclude?: string;
  /** sensor 也算。 */
  sensors?: boolean;
  /** 像这一组的碰撞体一样查：跳过同一组的。 */
  group?: number;
  /** 只查这些分组（0 是不分组的）。 */
  only?: number[];
  /** 形状的角度（度，和模型的 rotation 一样）。 */
  rotation?: V3;
}

/** 一个结果：`id` 是模型的 id，不是模型的碰撞体是 null。重叠只有 `id`；最近点没有 `normal`、有 `inside`。 */
export interface Hit3d {
  id: string | null;
  point?: V3;
  normal?: V3;
  distance?: number;
  inside?: boolean;
}

/** 引擎发来的 `query3d` 事件。 */
export interface Query3dReply {
  id: number;
  hits: Hit3d[];
  /** 查询本身写错了（分组超出 0 … 31、字段拼错等）时的原因；这时 hits 是空的。 */
  error?: string;
}

export interface Queries3d {
  /** 射线：最近的一个，没碰到是 undefined。`distance` 默认 10000。 */
  ray(from: V3, dir: V3, distance?: number, options?: Query3dOptions): Promise<Hit3d | undefined>;
  /** 形状投射：`shape` 放在 `from` 沿 `dir` 扫，最先碰到的。一开始就重叠的 distance 是 0。 */
  cast(shape: QueryShape3d, from: V3, dir: V3, distance?: number, options?: Query3dOptions): Promise<Hit3d | undefined>;
  /** 和 `shape`（放在 `at`）重叠的模型。 */
  overlap(shape: QueryShape3d, at: V3, options?: Query3dOptions): Promise<Hit3d[]>;
  /** 包含这个点的模型。 */
  point(at: V3, options?: Query3dOptions): Promise<Hit3d[]>;
  /** `distance` 以内最近的碰撞体表面上的点（默认不限）。 */
  closest(at: V3, distance?: number, options?: Query3dOptions): Promise<Hit3d | undefined>;
  /** 接 `<GameView onQuery3d={q.handle}>`。 */
  handle(reply: Query3dReply): void;
  /** 还没回来的查询数。 */
  pending(): number;
}

/** 引擎一直不回（command 丢了、没开 3D 物理）时多久按空结果结束（毫秒）。 */
const WAIT_MS = 2000;

/** 所有查询器共用的 id：几个查询器接同一个事件时，结果不会交错。 */
let nextId = 1;

/** `game` 拿游戏视图（`<GameView ref>`）；还没建好时查询直接是空结果。 */
export function queries3d(game: () => NativeComponentRef | undefined, settings: { waitMs?: number } = {}): Queries3d {
  const waiting = new Map<number, { resolve: (hits: Hit3d[]) => void; timer: ReturnType<typeof setTimeout> }>();
  const ask = (args: Record<string, unknown>) =>
    new Promise<Hit3d[]>((resolve) => {
      const view = game();
      if (view === undefined) return resolve([]);
      const id = nextId++;
      const timer = setTimeout(() => {
        if (waiting.delete(id)) resolve([]);
      }, settings.waitMs ?? WAIT_MS);
      waiting.set(id, { resolve, timer });
      // 没写的字段不发（有的桥会把 undefined 编成 null）
      view.command("query3d", Object.fromEntries(Object.entries({ id, ...args }).filter(([, v]) => v !== undefined)));
    });
  const first = (hits: Promise<Hit3d[]>) => hits.then((h) => h[0]);
  const opts = (o: Query3dOptions = {}) => ({ exclude: o.exclude, sensors: o.sensors, group: o.group, only: o.only, rotation: o.rotation });
  return {
    ray: (from, dir, distance, o) => first(ask({ kind: "ray", from, dir, distance, ...opts(o) })),
    cast: (shape, from, dir, distance, o) => first(ask({ kind: "cast", shape, from, dir, distance, ...opts(o) })),
    overlap: (shape, at, o) => ask({ kind: "overlap", shape, from: at, ...opts(o) }),
    point: (at, o) => ask({ kind: "point", from: at, ...opts(o) }),
    closest: (at, distance, o) => first(ask({ kind: "closest", from: at, distance, ...opts(o) })),
    handle(reply) {
      const entry = waiting.get(reply.id);
      if (entry === undefined) return;
      waiting.delete(reply.id);
      clearTimeout(entry.timer);
      if (reply.error !== undefined) console.warn(`query3d ${reply.id}: ${reply.error}`);
      entry.resolve(reply.hits);
    },
    pending: () => waiting.size,
  };
}
