// Tiled 地图（JSON 格式，`.tmj` 或导出的 `.json`）转成场景的瓦片地图和对象。见 docs/tiled.md。
import type { TilemapNode } from "./nge";

/** Tiled 自定义属性。 */
export interface TiledProperty {
  name: string;
  type?: string;
  value: unknown;
}

export interface TiledTileset {
  firstgid: number;
  name?: string;
  columns?: number;
  tilecount?: number;
  tilewidth?: number;
  tileheight?: number;
  margin?: number;
  spacing?: number;
  /** 外部图块集（`.tsx`）：不支持，要在 Tiled 里选「嵌入地图」。 */
  source?: string;
  tiles?: { id: number; properties?: TiledProperty[] }[];
}

export interface TiledObject {
  id: number;
  name?: string;
  type?: string;
  /** Tiled 1.9 起 `type` 改叫 `class`。 */
  class?: string;
  x: number;
  y: number;
  width?: number;
  height?: number;
  rotation?: number;
  visible?: boolean;
  point?: boolean;
  ellipse?: boolean;
  /** 多边形、折线的点，相对 (x, y)。 */
  polygon?: { x: number; y: number }[];
  polyline?: { x: number; y: number }[];
  /** 用了对象模板（外部 `.tx`）：不支持。 */
  template?: string;
  /** 图块对象：左下角对齐 (x, y)。 */
  gid?: number;
  properties?: TiledProperty[];
}

export interface TiledLayer {
  type: string;
  name?: string;
  visible?: boolean;
  width?: number;
  height?: number;
  data?: number[] | string;
  encoding?: string;
  chunks?: unknown[];
  offsetx?: number;
  offsety?: number;
  parallaxx?: number;
  parallaxy?: number;
  objects?: TiledObject[];
  layers?: TiledLayer[];
  properties?: TiledProperty[];
}

export interface TiledMap {
  orientation?: string;
  infinite?: boolean;
  width: number;
  height: number;
  tilewidth: number;
  tileheight: number;
  /** 视差的参照点（像素，相对地图左上角），默认 (0, 0)。 */
  parallaxoriginx?: number;
  parallaxoriginy?: number;
  layers: TiledLayer[];
  tilesets: TiledTileset[];
}

/** 转出来的对象：世界坐标（y 朝上），`(x, y)` 是中心。 */
export interface MapObject {
  id: number;
  name: string;
  type: string;
  /** 对象所在的图层名（图层的视差不作用在对象上）。 */
  layer: string;
  x: number;
  y: number;
  w: number;
  h: number;
  /** 和精灵的 `rotation` 一样：度，逆时针为正（Tiled 里是顺时针为正，已经换过来）。 */
  rotation: number;
  point: boolean;
  ellipse: boolean;
  properties: Record<string, unknown>;
}

export interface TiledOptions {
  /** 图块集名 → 场景用的图集（`asset:/` 路径）。 */
  textures: Record<string, string>;
  /** 地图左上角的世界坐标，默认 (0, 0)。带视差的层按 Tiled 的参照点换算，在相机对准同一处时和 Tiled 里看到的一样。 */
  x?: number;
  y?: number;
  /** 前缀，加在瓦片地图的 id 上，默认 ""（id 就是图层名）。 */
  prefix?: string;
}

export interface TiledResult {
  tilemaps: TilemapNode[];
  objects: MapObject[];
  /** 地图的世界大小。 */
  width: number;
  height: number;
}

/** 水平、竖直、对角翻转和六角地图的 120° 旋转。 */
const FLIP_FLAGS = 0xf0000000;

const props = (list?: TiledProperty[]): Record<string, unknown> => Object.fromEntries((list ?? []).map((p) => [p.name, p.value]));

/**
 * 把 Tiled 地图转成场景的 `tilemaps` 和对象列表。
 * - 每个图块层一张瓦片地图，id 是 `prefix + 图层名`；一层只能用一个图块集。
 * - 图块集里属性 `solid = true` 的图块是实心的；图层属性 `solid = true` 时整层实心，`false` 时整层不实心（背景层）。图层属性 `z`、`friction`、`restitution`、`collide` 照抄。
 * - 对象层的对象换成世界坐标（y 朝上，中心点），游戏自己决定生成什么。
 * 不支持的东西（无限地图、外部图块集、图像集合、压缩数据、非正交、瓦片不是正方形、对象模板）直接抛错，说明怎么在 Tiled 里改。
 */
export function tiled(map: TiledMap, options: TiledOptions): TiledResult {
  const fail = (why: string): never => {
    throw new Error(`Tiled 地图：${why}`);
  };
  if ((map.orientation ?? "orthogonal") !== "orthogonal") fail(`只支持正交地图，这张是 ${map.orientation}`);
  if (map.infinite) fail("不支持无限地图，在 Tiled 的地图属性里关掉「无限」");
  if (map.tilewidth !== map.tileheight || !(map.tilewidth > 0)) fail(`瓦片要是正方形，这张是 ${map.tilewidth} × ${map.tileheight}`);
  const tile = map.tilewidth;
  const ox = options.x ?? 0;
  const oy = options.y ?? 0;
  const prefix = options.prefix ?? "";
  const tilesets = [...map.tilesets].sort((a, b) => a.firstgid - b.firstgid);
  for (const t of tilesets) {
    if (t.source !== undefined) fail(`图块集 ${t.source} 是外部文件，在 Tiled 里把它嵌入地图`);
    if (!Number.isInteger(t.columns) || (t.columns as number) <= 0 || (t.margin ?? 0) !== 0 || (t.spacing ?? 0) !== 0) {
      fail(`图块集 ${t.name ?? ""} 要用一张图、边距和间距为 0（不支持图像集合）`);
    }
  }
  // Tiled 的视差：画的位置 = 位置 + (视野中心 − 参照点) × (1 − 系数)；引擎的参照点是世界原点，把差的挪到位置上
  const originX = ox + (map.parallaxoriginx ?? 0);
  const originY = oy - (map.parallaxoriginy ?? 0);
  const tilesetOf = (gid: number) => tilesets.filter((t) => t.firstgid <= gid).pop();

  const tilemaps: TilemapNode[] = [];
  const objects: MapObject[] = [];

  const visit = (layer: TiledLayer, dx: number, dy: number, px: number, py: number) => {
    if (layer.visible === false) return;
    const name = layer.name ?? "";
    const offx = dx + (layer.offsetx ?? 0);
    const offy = dy + (layer.offsety ?? 0);
    const parx = px * (layer.parallaxx ?? 1);
    const pary = py * (layer.parallaxy ?? 1);
    const lp = props(layer.properties);
    if (layer.type === "group") {
      for (const child of layer.layers ?? []) visit(child, offx, offy, parx, pary);
    } else if (layer.type === "tilelayer") {
      if (layer.chunks !== undefined) fail(`图层 ${name} 是分块的（无限地图）`);
      if (!Array.isArray(layer.data)) fail(`图层 ${name} 的数据是 ${layer.encoding ?? "编码"} 的，在 Tiled 的地图属性里把「图层格式」改成 CSV`);
      const data = layer.data as number[];
      const cols = layer.width ?? map.width;
      const gids = data.map((g) => (g & ~FLIP_FLAGS) >>> 0);
      if (data.some((g) => (g & FLIP_FLAGS) !== 0)) console.warn(`Tiled 地图：图层 ${name} 有翻转的瓦片，按不翻转画（每层警告一次）`);
      const used = gids.filter((g) => g > 0);
      if (used.length === 0) return;
      const set = tilesetOf(used[0]);
      if (set === undefined) return fail(`图层 ${name} 的瓦片 ${used[0]} 不在任何图块集里`);
      if (used.some((g) => tilesetOf(g) !== set)) fail(`图层 ${name} 用了好几个图块集，一层只能用一个`);
      const setName = set.name ?? "";
      const texture = options.textures[setName];
      if (texture === undefined) fail(`没给图块集 ${setName} 对应的图集（options.textures）`);
      const columns = set.columns as number;
      const count = set.tilecount ?? columns;
      const over = used.find((g) => g - set.firstgid >= count);
      if (over !== undefined) fail(`图层 ${name} 的瓦片 ${over} 超出了图块集 ${setName}`);
      const solidTiles = (set.tiles ?? []).filter((t) => props(t.properties).solid === true).map((t) => t.id);
      const node: TilemapNode = {
        id: prefix + name,
        texture,
        grid: [columns, Math.ceil(count / columns)],
        tile,
        cols,
        x: ox + offx - originX * (1 - parx),
        y: oy - offy - originY * (1 - pary),
        data: gids.map((g) => (g === 0 ? -1 : g - set.firstgid)),
        solid: lp.solid === true ? Array.from({ length: count }, (_, i) => i) : lp.solid === false ? [] : solidTiles,
      };
      if (typeof lp.z === "number") node.z = lp.z;
      if (typeof lp.friction === "number") node.friction = lp.friction;
      if (typeof lp.restitution === "number") node.restitution = lp.restitution;
      if (typeof lp.collide === "boolean") node.collide = lp.collide;
      if (parx !== 1 || pary !== 1) node.parallax = [parx, pary];
      tilemaps.push(node);
    } else if (layer.type === "objectgroup") {
      for (const o of layer.objects ?? []) {
        if (o.template !== undefined) fail(`对象 ${o.id} 用了模板 ${o.template}，在 Tiled 里把它从模板分离`);
        if (o.visible === false) continue;
        // 多边形、折线按点的外接框
        const points = o.polygon ?? o.polyline;
        const [minX, minY] = points ? [Math.min(...points.map((q) => q.x)), Math.min(...points.map((q) => q.y))] : [0, 0];
        const w = points ? Math.max(...points.map((q) => q.x)) - minX : (o.width ?? 0);
        const h = points ? Math.max(...points.map((q) => q.y)) - minY : (o.height ?? 0);
        // Tiled 绕 (x, y) 顺时针转（y 朝下）：图块对象的 (x, y) 是左下角，别的是左上角
        const turn = ((o.rotation ?? 0) * Math.PI) / 180;
        const [cx, cy] = [minX + w / 2, minY + (o.gid !== undefined ? -h / 2 : h / 2)];
        const rx = cx * Math.cos(turn) - cy * Math.sin(turn);
        const ry = cx * Math.sin(turn) + cy * Math.cos(turn);
        objects.push({
          id: o.id,
          name: o.name ?? "",
          // Tiled 1.10 总写 "type": ""，空的时候看 class
          type: o.type || o.class || "",
          layer: name,
          x: ox + offx + o.x + rx,
          y: oy - offy - (o.y + ry),
          w,
          h,
          rotation: -(o.rotation ?? 0) || 0,
          point: o.point === true,
          ellipse: o.ellipse === true,
          properties: props(o.properties),
        });
      }
    }
  };
  for (const layer of map.layers) visit(layer, 0, 0, 1, 1);
  return { tilemaps, objects, width: map.width * tile, height: map.height * tile };
}
