// 关卡文件格式（ts/level.ts）：读入时校验、格式化、类型表检查（见 docs/editor.md）。
import { expect, test } from "nsh/test";
import stairs from "./levels/stairs.json";
import { checkProp, checkTypes, emptyLevel, formatLevel, LEVEL_MIGRATIONS, LEVEL_VERSION, migrateLevel, parseLevel, toTilemap, type LevelFile, type TypeSpecs } from "../../ts/level";

const sample = (): LevelFile => ({
  format: "nge-level",
  version: LEVEL_VERSION,
  name: "测试",
  night: false,
  tiles: [{ id: "level", texture: "asset:/@g/tiles.png", grid: [3, 1], tile: 32, cols: 3, rows: 2, x: 0, y: 32, solid: [0, 1], data: [[-1, 2, -1], [1, 1, 0]] }],
  entities: [
    { id: "e1", type: "coin", x: 16, y: 40 },
    { id: "e2", type: "enemy", name: "小怪", x: 48, y: 12, w: 24, h: 24, props: { patrol: 2, angry: true, mood: "坏" } },
  ],
});

const errorsOf = (input: unknown) => {
  const r = parseLevel(input);
  if (r.ok) throw new Error("应该报错");
  return r.errors;
};

test("合法的关卡原样读出来，字符串和解析过的值都行", () => {
  const level = sample();
  expect(parseLevel(JSON.stringify(level))).toEqual({ ok: true, level });
  expect(parseLevel(level)).toEqual({ ok: true, level });
});

test("可选字段：没有 name / night / props / w / h 时补默认值，多余的字段丢掉", () => {
  const raw = { format: "nge-level", version: 1, tiles: [], entities: [{ id: "a", type: "coin", x: 1, y: 2, extra: 9 }], junk: true };
  expect(parseLevel(raw)).toEqual({ ok: true, level: { format: "nge-level", version: LEVEL_VERSION, name: "", night: false, tiles: [], entities: [{ id: "a", type: "coin", x: 1, y: 2 }] } });
});

test("不是 JSON、不是对象、格式名不对都报错，不抛异常", () => {
  expect(errorsOf("{坏的")[0]).toContain("不是合法的 JSON");
  expect(errorsOf("null")[0]).toContain("不是对象");
  expect(errorsOf([1, 2])[0]).toContain("不是对象");
  expect(errorsOf({ ...sample(), format: "tiled" })[0]).toContain("format");
});

test("版本：缺了、不是正整数的报错，比现在新的说是更新的编辑器存的", () => {
  expect(errorsOf({ ...sample(), version: undefined })[0]).toContain("version");
  expect(errorsOf({ ...sample(), version: 1.5 })[0]).toContain("version");
  expect(errorsOf({ ...sample(), version: 0 })[0]).toContain("version");
  expect(errorsOf({ ...sample(), version: LEVEL_VERSION + 1 })[0]).toContain("更新的编辑器");
});

test("实体：id 空的、重复的，坐标不是有限的数、超出范围的，都报出路径", () => {
  const bad = sample();
  const raw = JSON.parse(JSON.stringify(bad));
  raw.entities.push({ id: "e1", type: "coin", x: 0, y: 0 });
  raw.entities.push({ id: "", type: "coin", x: 0, y: 0 });
  raw.entities.push({ id: "e9", type: "", x: "1", y: 2e7 });
  raw.entities.push({ id: "e10", type: "coin", x: 0, y: 0, w: -3, props: { a: { b: 1 } } });
  const errors = errorsOf(raw).join("\n");
  expect(errors).toContain("entities[2].id 和 entities[0] 重复");
  expect(errors).toContain("entities[3].id");
  expect(errors).toContain("entities[4].type");
  expect(errors).toContain("entities[4].x");
  expect(errors).toContain("entities[4].y");
  expect(errors).toContain("entities[5].w");
  expect(errors).toContain("entities[5].props.a");
});

test("NaN、Infinity 进不来（JSON 之外直接给的值也查）", () => {
  const raw = sample() as any;
  raw.entities[0].x = NaN;
  raw.entities[1].props.patrol = Infinity;
  const errors = errorsOf(raw).join("\n");
  expect(errors).toContain("entities[0].x");
  expect(errors).toContain("entities[1].props.patrol");
});

test("字符串太长或有控制字符的报错", () => {
  const raw = sample() as any;
  raw.name = "x".repeat(300);
  raw.entities[0].name = "a\u0007b";
  const errors = errorsOf(raw).join("\n");
  expect(errors).toContain("name");
  expect(errors).toContain("entities[0].name");
});

test("瓦片层：行数、每行的格数对不上，帧号不是整数或超出图集的报错", () => {
  const raw = sample() as any;
  raw.tiles[0].data = [[-1, 2], [1, 1, 3.5]];
  const errors = errorsOf(raw).join("\n");
  expect(errors).toContain("tiles[0].data[0] 要有 3 格");
  expect(errors).toContain("tiles[0].data[1][2]");
  const rows = sample() as any;
  rows.tiles[0].data = [[-1, 2, -1]];
  expect(errorsOf(rows).join("\n")).toContain("tiles[0].data 要有 2 行");
  const frame = sample() as any;
  frame.tiles[0].data[0][0] = 3;
  expect(errorsOf(frame).join("\n")).toContain("tiles[0].data[0][0]");
});

test("瓦片层太大、实体太多的报错，错误最多 20 条", () => {
  const raw = sample() as any;
  raw.tiles[0].cols = 5000;
  expect(errorsOf(raw).join("\n")).toContain("tiles[0].cols");
  const many = sample() as any;
  many.entities = Array.from({ length: 5001 }, (_, i) => ({ id: `e${i}`, type: "coin", x: 0, y: 0 }));
  expect(errorsOf(many)[0]).toContain("entities");
  const noisy = sample() as any;
  noisy.entities = Array.from({ length: 50 }, (_, i) => ({ id: `e${i}`, type: "coin", x: "坏", y: 0 }));
  expect(errorsOf(noisy).length).toBe(20);
});

test("格式化：瓦片一行一行、实体一个一行，读回来一样", () => {
  const level = sample();
  const text = formatLevel(level);
  expect(text.endsWith("\n")).toBe(true);
  expect(text).toContain('\n        [-1, 2, -1],\n        [1, 1, 0]\n');
  expect(text).toContain('\n    { "id": "e1", "type": "coin", "x": 16, "y": 40 },\n');
  expect(parseLevel(text)).toEqual({ ok: true, level });
  // 名字里有引号、冒号、逗号也原样读回来
  const odd = { ...level, name: 'a":,"b', entities: [{ ...level.entities[1], name: 'x":1, "y' }] };
  expect(parseLevel(formatLevel(odd))).toEqual({ ok: true, level: odd });
  // 空列表也能读回来
  const empty = { ...level, tiles: [], entities: [] };
  expect(parseLevel(formatLevel(empty))).toEqual({ ok: true, level: empty });
});

test("新建的空关卡：一层全空的瓦片，读回来合法", () => {
  const level = emptyLevel({ name: "新关卡", cols: 4, rows: 2, tile: 32, texture: "asset:/@g/t.png", grid: [3, 1], solid: [0] });
  expect(level.tiles[0].data).toEqual([[-1, -1, -1, -1], [-1, -1, -1, -1]]);
  expect(level.tiles[0].y).toBe(32);
  expect(parseLevel(formatLevel(level))).toEqual({ ok: true, level });
});

test("瓦片层的单向平台、斜坡、动画瓦片、材质：可选，读回来一样，转成瓦片地图时带上（docs/gameplay2d.md）", () => {
  const rules = {
    oneWay: [2],
    slopes: [{ frame: 1, left: 0, right: 0.5 }],
    animated: [{ frame: 0, frames: [0, 1], fps: 4 }],
    materials: [{ frame: 1, friction: 0.05 }, { frame: 0, restitution: 0.5 }],
  };
  const level = sample();
  level.tiles[0] = { ...level.tiles[0], ...rules };
  expect(parseLevel(formatLevel(level))).toEqual({ ok: true, level });
  expect(toTilemap(level.tiles[0])).toEqual({ id: "level", texture: "asset:/@g/tiles.png", grid: [3, 1], tile: 32, cols: 3, x: 0, y: 32, solid: [0, 1], ...rules, data: [-1, 2, -1, 1, 1, 0] });
  // 新建关卡时从图集抄
  const empty = emptyLevel({ name: "", cols: 2, rows: 1, tile: 32, texture: "t", grid: [3, 1], solid: [0], ...rules });
  expect(empty.tiles[0].slopes).toEqual(rules.slopes);
  expect(parseLevel(formatLevel(empty))).toEqual({ ok: true, level: empty });
});

test("瓦片规则写错了报出哪一项：帧号超出图集、坡高不在 0 … 1、两头都是 0、动画没有帧、fps 不是正数、材质是负数", () => {
  const bad = (rules: object) => {
    const level = sample();
    return errorsOf({ ...level, tiles: [{ ...level.tiles[0], ...rules }] });
  };
  expect(bad({ oneWay: [3] })).toEqual(["tiles[0].oneWay[0] 要是 0 到 2 的整数"]);
  expect(bad({ slopes: [{ frame: 1, left: 0, right: 2 }] })).toEqual(["tiles[0].slopes[0].right 要是 0 到 1 的数"]);
  expect(bad({ slopes: [{ frame: 1, left: 0, right: 0 }] })).toEqual(["tiles[0].slopes[0] 的 left、right 不能都是 0"]);
  expect(bad({ animated: [{ frame: 0, frames: [], fps: 4 }] })).toEqual(["tiles[0].animated[0].frames 不能是空的"]);
  expect(bad({ animated: [{ frame: 0, frames: [1], fps: 0 }] })[0]).toContain("tiles[0].animated[0].fps");
  expect(bad({ materials: [{ frame: 0, friction: -1 }] })).toEqual(["tiles[0].materials[0].friction 要是不小于 0 的数"]);
  expect(bad({ slopes: "坡" })).toEqual(["tiles[0].slopes 要是数组"]);
});

test("瓦片层转成引擎的瓦片地图：data 摊平成一维", () => {
  const map = toTilemap(sample().tiles[0]);
  expect(map).toEqual({ id: "level", texture: "asset:/@g/tiles.png", grid: [3, 1], tile: 32, cols: 3, x: 0, y: 32, solid: [0, 1], data: [-1, 2, -1, 1, 1, 0] });
});

const TYPES: TypeSpecs = {
  coin: { props: [] },
  enemy: {
    props: [
      { key: "patrol", label: "巡逻", kind: "number", min: 1, max: 20, default: 3 },
      { key: "angry", label: "生气", kind: "bool", default: false },
      { key: "mood", label: "心情", kind: "choice", options: ["好", "坏"], default: "好" },
    ],
  },
  goal: { label: "终点", props: [], min: 1, max: 1 },
};

test("类型表：不认识的类型、属性类型不对、超范围、不认识的属性、数量不对都报错", () => {
  expect(checkTypes(sample(), { ...TYPES, goal: { props: [] } })).toEqual([]);
  const level = sample();
  const bad: LevelFile = {
    ...level,
    entities: [
      ...level.entities,
      { id: "e3", type: "dragon", x: 0, y: 0 },
      { id: "e4", type: "enemy", x: 0, y: 0, props: { patrol: 99, angry: "yes", mood: "一般", fly: 1 } },
    ],
  };
  const errors = checkTypes(bad, TYPES).join("\n");
  expect(errors).toContain("e3：不认识的类型 dragon");
  expect(errors).toContain("e4 的 巡逻");
  expect(errors).toContain("e4 的 生气");
  expect(errors).toContain("e4 的 心情");
  expect(errors).toContain("e4：不认识的属性 fly");
  expect(errors).toContain("终点要有 1 个");
});

test("单个属性的检查：数字范围、步长不限、布尔、选项", () => {
  const patrol = TYPES.enemy.props[0];
  expect(checkProp(patrol, 3)).toBeNull();
  expect(checkProp(patrol, 0)).toContain("1 到 20");
  expect(checkProp(patrol, "3")).toContain("数");
  expect(checkProp(TYPES.enemy.props[1], true)).toBeNull();
  expect(checkProp(TYPES.enemy.props[2], "坏")).toBeNull();
  const color = { key: "c", label: "颜色", kind: "text" as const, default: "#fff", pattern: /^#[0-9a-f]{6}$/, hint: "颜色" };
  expect(checkProp(color, "#a0b1c2")).toBeNull();
  expect(checkProp(color, "红")).toContain("颜色");
});

test("属性名只能是字母开头的字母、数字、下划线，__proto__ 这种不收", () => {
  const withProps = (props: unknown) => ({ ...sample(), entities: [{ id: "e1", type: "coin", x: 0, y: 0, props }] });
  expect(errorsOf(withProps(JSON.parse('{"__proto__": 5}')))[0]).toMatch(/props/);
  expect(errorsOf(formatLevel(sample()).replace('"type": "coin"', '"type": "coin", "props": { "__proto__": 5 }'))[0]).toMatch(/props/);
  expect(errorsOf(withProps({ "a b": 1 }))[0]).toMatch(/props/);
  expect(errorsOf(withProps({ ["k".repeat(40)]: 1 }))[0]).toMatch(/props/);
  expect(parseLevel(withProps({ patrol_2: 1 })).ok).toBe(true);
});

test("瓦片层：最多 16 层，id 不重复；行数不对时不往下查每一格", () => {
  const l = sample();
  expect(errorsOf({ ...l, tiles: Array.from({ length: 17 }, (_, i) => ({ ...l.tiles[0], id: `t${i}` })) })[0]).toMatch(/最多 16 层/);
  expect(errorsOf({ ...l, tiles: [l.tiles[0], l.tiles[0]] })[0]).toMatch(/重复/);
  const wrong = { ...l, tiles: [{ ...l.tiles[0], rows: 2, data: [[1, 1, 1, "x"], [1], [1]] }] };
  expect(errorsOf(wrong)).toEqual(["tiles[0].data 要有 2 行"]);
});

test("实体 id 只能有字母、数字、- 和 _", () => {
  const l = sample();
  for (const id of ["a:to", "a b", "中"]) expect(errorsOf({ ...l, entities: [{ id, type: "coin", x: 0, y: 0 }] })[0]).toMatch(/id/);
});

// ---- 第 2 版（docs/editor-more.md）----

const v2 = (): LevelFile => {
  const l = sample();
  return {
    ...l,
    tiles: [l.tiles[0], { ...l.tiles[0], id: "far", name: "远处", hidden: true, locked: true, parallax: [0.5, 1], z: -70 }],
    entities: [
      { id: "g", type: "group", name: "一组", x: 0, y: 0 },
      { id: "e1", type: "coin", x: 16, y: 40, parent: "g" },
      { id: "e2", type: "deco", x: 48, y: 12, rotation: 30, z: -5, parent: "g", points: [[1, 2], [3, 4]] },
      { id: "e3", type: "prefab", x: 0, y: 0, props: { prefab: "grove" }, set: { "b/crown": { y: 110, unlit: true, color: "#fff" } } },
    ],
  };
};

test("第 2 版：层的名字、显隐、锁定、视差，实体的旋转、z、父节点、路线点、覆盖，读写来回不变", () => {
  const level = v2();
  expect(LEVEL_VERSION).toBe(2);
  expect(parseLevel(formatLevel(level))).toEqual({ ok: true, level });
  expect(parseLevel(level)).toEqual({ ok: true, level });
});

test("第 1 版的文件迁到第 2 版：第 1 版不认识的字段丢掉（意思不变），版本号改成 2，不改传进来的对象", () => {
  const raw = JSON.parse(JSON.stringify({ ...v2(), version: 1 }));
  const before = JSON.stringify(raw);
  const r = parseLevel(raw);
  if (!r.ok) throw new Error(r.errors.join("；"));
  expect(JSON.stringify(raw)).toBe(before);
  expect(r.level.version).toBe(2);
  expect(r.level.tiles[1]).toEqual({ ...sample().tiles[0], id: "far", z: -70 });
  expect(r.level.entities.map((e) => Object.keys(e).sort())).toEqual([["id", "name", "type", "x", "y"], ["id", "type", "x", "y"], ["id", "type", "x", "y"], ["id", "props", "type", "x", "y"]]);
  // 迁移函数自己：一步一步迁
  expect(Object.keys(LEVEL_MIGRATIONS)).toEqual(["1"]);
  const m = migrateLevel({ format: "nge-level", version: 1, tiles: [{ id: "t", locked: true }], entities: [{ id: "a", parent: "b", set: {} }] });
  expect(m).toEqual({ format: "nge-level", version: 2, tiles: [{ id: "t" }], entities: [{ id: "a" }] });
  // 坏的不迁（交给校验去报）
  expect(migrateLevel({ format: "nge-level", version: 1, tiles: "坏", entities: [7] })).toEqual({ format: "nge-level", version: 2, tiles: "坏", entities: [7] });
});

test("第 2 版的字段写错了报出路径：视差、旋转、路线点、覆盖的路径和值", () => {
  const level = v2() as any;
  level.tiles[1].parallax = [0.5, 9];
  level.tiles[1].hidden = "是";
  level.tiles[1].name = "x".repeat(80);
  level.entities[2].rotation = 1e5;
  level.entities[2].points = [[1, 2], [3]];
  level.entities[3].set = { "b//crown": { y: 1 }, a: { "__proto__x": 1 }, c: { y: { deep: 1 } } };
  const errors = errorsOf(level).join("\n");
  for (const part of ["tiles[1].parallax[1]", "tiles[1].hidden", "tiles[1].name", "entities[2].rotation", "entities[2].points[1]", "entities[3].set 的子节点", "entities[3].set.a", "entities[3].set.c.y"]) expect(errors).toContain(part);
  const many = v2() as any;
  many.entities[2].points = Array.from({ length: 65 }, () => [0, 0]);
  expect(errorsOf(many).join("\n")).toContain("entities[2].points 最多 64 个");
});

test("父节点：要指向有的实体，不能是自己，不能绕成圈", () => {
  const level = v2() as any;
  level.entities[0].parent = "e2";
  expect(errorsOf(level).join("\n")).toContain("绕成了圈");
  const self = v2() as any;
  self.entities[1].parent = "e1";
  expect(errorsOf(self).join("\n")).toContain("entities[1].parent");
  const missing = v2() as any;
  missing.entities[1].parent = "nope";
  expect(errorsOf(missing).join("\n")).toContain("entities[1].parent 指向的 nope 不在关卡里");
  // 好几层的父子链（不绕圈）是对的
  const chain = v2() as any;
  const ids = chain.entities.map((e: { id: string }) => e.id);
  chain.entities.forEach((e: { parent?: string }, i: number) => {
    if (i > 0) e.parent = ids[i - 1];
    else delete e.parent;
  });
  expect(ids.length).toBeGreaterThan(2);
  expect(parseLevel(chain).ok).toBe(true);
});

test("转成引擎的瓦片地图：去掉只给编辑器用的名字、显隐、锁定，带上视差", () => {
  const map = toTilemap(v2().tiles[1]);
  expect(map).toEqual({ id: "far", texture: "asset:/@g/tiles.png", grid: [3, 1], tile: 32, cols: 3, x: 0, y: 32, z: -70, parallax: [0.5, 1], solid: [0, 1], data: [-1, 2, -1, 1, 1, 0] });
});

test("没写 z 的层按顺序叠：第一层不写 z（和以前一样），后面的每层高一点点；写了的照写", () => {
  const [a, b] = v2().tiles;
  expect(toTilemap(a, 0).z).toBe(undefined);
  expect(toTilemap(a, 2).z).toBe(0.002);
  expect(toTilemap(b, 1).z).toBe(-70);
});

test("石阶的文件是第 2 版，读进来再格式化、再读回来一样", () => {
  const r = parseLevel(stairs);
  if (!r.ok) throw new Error(r.errors.join("；"));
  expect((stairs as { version: number }).version).toBe(2);
  expect(parseLevel(formatLevel(r.level))).toEqual(r);
  expect(r.level).toEqual(stairs as unknown as LevelFile);
});

test("覆盖的子节点路径不能有 __proto__ 这一段（写进对象会被吞掉）", () => {
  const raw = JSON.parse(formatLevel(v2()));
  raw.entities[3].set = JSON.parse('{"__proto__": {"y": 1}}');
  expect(errorsOf(raw).join("\n")).toContain("__proto__");
  raw.entities[3].set = JSON.parse('{"a/__proto__": {"y": 1}}');
  expect(errorsOf(raw).join("\n")).toContain("__proto__");
  // 别的带下划线的照样行
  raw.entities[3].set = JSON.parse('{"__crown": {"y": 1}}');
  expect(parseLevel(raw).ok).toBe(true);
});
