// `tiled()`：Tiled 地图转成瓦片地图和对象（见 docs/tiled.md），和平台跳跃用 Tiled 画的第三关。
import { expect, test } from "nsh/test";
import { tiled, type TiledMap } from "../../ts/tiled";
import { flagOn, fromTiled, LEVELS, LINKS, TILE } from "./levels";
import level3 from "./maps/level3.json";

const base = (): TiledMap => ({
  orientation: "orthogonal", infinite: false, width: 3, height: 2, tilewidth: 16, tileheight: 16,
  tilesets: [{ firstgid: 1, name: "tiles", columns: 3, tilecount: 6, tiles: [{ id: 1, properties: [{ name: "solid", type: "bool", value: true }] }] }],
  layers: [
    { type: "tilelayer", name: "ground", width: 3, height: 2, data: [0, 1, 2, 2, 2, 0x80000003] },
    { type: "objectgroup", name: "things", objects: [
      { id: 1, name: "coin", type: "coin", x: 16, y: 0, width: 16, height: 16, properties: [{ name: "value", value: 5 }] },
      { id: 2, name: "start", class: "spawn", x: 8, y: 24, point: true },
      { id: 3, gid: 4, x: 0, y: 32, width: 16, height: 16 },
      { id: 4, x: 0, y: 0, width: 20, height: 10, rotation: 90 },
      { id: 5, x: 0, y: 0, visible: false },
    ] },
  ],
});
const T = { tiles: "asset:/@g/tiles.png" };

test("图块层变成瓦片地图：帧号从 0 数，0 是空格，实心按图块属性", () => {
  const r = tiled(base(), { textures: T, x: 100, y: 50 });
  expect(r.tilemaps).toEqual([{ id: "ground", texture: T.tiles, grid: [3, 2], tile: 16, cols: 3, x: 100, y: 50, data: [-1, 0, 1, 1, 1, 2], solid: [1] }]);
  expect([r.width, r.height]).toEqual([48, 32]);
});

test("对象换成世界坐标的中心，y 朝上", () => {
  const { objects } = tiled(base(), { textures: T });
  const by = (id: number) => objects.find((o) => o.id === id)!;
  expect(by(1)).toEqual({ id: 1, name: "coin", type: "coin", layer: "things", x: 24, y: -8, w: 16, h: 16, rotation: 0, point: false, ellipse: false, properties: { value: 5 } });
  expect([by(2).type, by(2).x, by(2).y, by(2).point]).toEqual(["spawn", 8, -24, true]);
  // 图块对象的 (x, y) 是左下角
  expect([by(3).x, by(3).y]).toEqual([8, -24]);
  // Tiled 顺时针转 90°：绕左上角转，中心到了左下方
  expect([by(4).x, by(4).y, by(4).rotation].map(Math.round)).toEqual([-5, -10, -90]);
  expect(objects.some((o) => o.id === 5)).toBe(false);
});

test("图层属性：整层实心、z、视差、偏移，前缀；隐藏的层跳过", () => {
  const map = base();
  map.layers = [
    { type: "group", name: "g", offsetx: 32, parallaxx: 0.5, layers: [
      { type: "tilelayer", name: "back", width: 3, data: [1, 0, 0, 0, 0, 0], offsetx: 16, offsety: 16, parallaxx: 0.5, parallaxy: 0.5, properties: [{ name: "solid", value: true }, { name: "z", value: -5 }, { name: "collide", value: true }] },
    ] },
    { type: "tilelayer", name: "hidden", visible: false, width: 3, data: [1, 1, 1, 1, 1, 1] },
  ];
  const [m, ...rest] = tiled(map, { textures: T, prefix: "l1-" }).tilemaps;
  expect(rest).toEqual([]);
  map.layers = [{ type: "tilelayer", name: "far", width: 3, data: [2, 2, 0, 0, 0, 0], properties: [{ name: "solid", value: false }] }];
  expect(tiled(map, { textures: T }).tilemaps[0].solid).toEqual([]);
  // 组图层和子图层：偏移相加，视差相乘
  expect([m.id, m.x, m.y, m.z, m.collide, m.parallax, m.solid]).toEqual(["l1-back", 48, -16, -5, true, [0.25, 0.5], [0, 1, 2, 3, 4, 5]]);
});

test("带视差的层按 Tiled 的参照点换算：相机对准参照点时画在 Tiled 里的位置", () => {
  const map = base();
  map.parallaxoriginx = 16;
  map.parallaxoriginy = 32;
  map.layers = [{ type: "tilelayer", name: "far", width: 3, data: [1, 0, 0, 0, 0, 0], parallaxx: 0.5, parallaxy: 0.25 }];
  const [m] = tiled(map, { textures: T, x: 100, y: 448 }).tilemaps;
  // 参照点的世界坐标 (116, 416)；引擎画在 位置 + 相机 × (1 − 系数)，相机在参照点时要回到 (100, 448)
  expect([m.x! + 116 * 0.5, m.y! + 416 * 0.75]).toEqual([100, 448]);
});

test("对象的类型：type 是空的看 class；多边形、折线按点的外接框", () => {
  const map = base();
  map.layers = [{ type: "objectgroup", name: "o", objects: [
    { id: 1, type: "", class: "door", x: 0, y: 0, width: 10, height: 10 },
    { id: 2, x: 100, y: 100, polygon: [{ x: 0, y: 0 }, { x: 20, y: -10 }, { x: 40, y: 30 }] },
    { id: 3, x: 0, y: 0, polyline: [{ x: -10, y: 0 }, { x: 10, y: 4 }] },
  ] }];
  const [door, poly, line] = tiled(map, { textures: T }).objects;
  expect(door.type).toBe("door");
  expect([poly.x, poly.y, poly.w, poly.h]).toEqual([120, -110, 40, 40]);
  expect([line.x, line.y, line.w, line.h]).toEqual([0, -2, 20, 4]);
});

test("不支持的地图直接报错，说清怎么改", () => {
  const bad = (change: (m: TiledMap) => void, why: string) => {
    const map = base();
    change(map);
    expect(() => tiled(map, { textures: T })).toThrow(why);
  };
  bad((m) => (m.infinite = true), "无限");
  bad((m) => (m.orientation = "isometric"), "正交");
  bad((m) => (m.tileheight = 8), "正方形");
  bad((m) => (m.tilesets[0].source = "a.tsx"), "嵌入");
  bad((m) => (m.tilesets[0].columns = 0), "图像集合");
  bad((m) => (m.tilesets[0].spacing = 2), "间距");
  bad((m) => ((m.layers[0].data as number[])[0] = 7), "超出");
  bad((m) => (m.layers[1].objects![0].template = "coin.tx"), "模板");
  bad((m) => (m.layers[0] = { ...m.layers[0], data: "AAAA", encoding: "base64" }), "CSV");
  bad((m) => m.tilesets.push({ firstgid: 7, name: "more", columns: 1, tilecount: 1 }) && ((m.layers[0].data as number[])[0] = 7), "好几个图块集");
  expect(() => tiled(base(), { textures: {} })).toThrow("tiles");
});

test("第三关：Tiled 画的地图生成和代码描述的关卡一样的东西", () => {
  const built = fromTiled(level3);
  expect(LEVELS.map((l) => l.name)).toEqual(["草原", "石阶", "云端"]);
  expect(LEVELS[2].built).toEqual(built);
  const [far, ground] = built.tilemaps;
  // 底行的顶边在 y = 0
  expect([ground.id, ground.x, ground.y, ground.cols, ground.solid, ground.friction]).toEqual(["ground", 0, 14 * TILE, 60, [0, 1, 2], 0]);
  expect(ground.data.slice(14 * 60, 14 * 60 + 13)).toEqual([0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1]);
  expect([far.solid, far.z, far.parallax]).toEqual([[], -3, [0.5, 1]]);
  expect(built.coins.length).toBe(10);
  // 平台 [10, 8, 11] 上面的金币：平台顶边 128，往上 28
  expect(built.coins[0]).toEqual({ id: "coin-t0", x: 8 * TILE, y: 156 });
  expect(built.enemies[0]).toEqual({ id: "enemy-0", x: 18.5 * TILE, to: { x: 21.5 * TILE } });
  expect(built.lifts.map((l) => [l.node.x, l.node.y, l.to, l.duration])).toEqual([[29 * TILE, -8, { x: 33 * TILE }, 2.2]]);
  expect([built.goal.x, built.goal.y, built.crate.body.x, built.crate.body.y, built.right]).toEqual([56 * TILE, 40, 54.5 * TILE, 56, 60 * TILE]);
  // 第一个坑上面挂着铁链
  expect(built.swing.joints[0]).toEqual({ id: "swing-j0", a: undefined, b: "swing-0", kind: "revolute", anchorA: [13.5 * TILE, 150] });
});

test("铁链一节节挂着，3D 箱子跟着看不见的刚体", () => {
  for (const { built } of LEVELS) {
    const { sprites, joints } = built.swing;
    expect(sprites.length).toBe(LINKS + 1);
    expect(joints.map((j) => [j.a, j.b])).toEqual(sprites.map((s, i) => [i === 0 ? undefined : sprites[i - 1].id, s.id]));
    // 每个连接点在上一节的底边、这一节的顶边
    for (const [i, j] of joints.entries()) {
      const s = sprites[i];
      expect(j.anchorA![1]).toBe(s.y! + (s.h ?? s.w!) / 2);
    }
    expect(sprites.every((s) => s.body !== undefined)).toBe(true);
    expect(built.crate.model.attach).toBe(built.crate.body.id);
    expect(built.crate.body.body?.kind ?? "dynamic").toBe("dynamic");
  }
});

test("旗子挂在旗杆右上角、在角色前面、播 wave", () => {
  const goal = LEVELS[0].built.goal;
  const flag = flagOn(goal);
  expect([flag.x, flag.y, flag.z, flag.animation?.clip]).toEqual([goal.x! + 8, goal.y! + 38, 30, "wave"]);
});
