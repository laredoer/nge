// 关卡文件格式（ts/level.ts）：读入时校验、格式化、类型表检查（见 docs/editor.md）。
import { expect, test } from "nsh/test";
import { checkProp, checkTypes, emptyLevel, formatLevel, LEVEL_VERSION, parseLevel, toTilemap, type LevelFile, type TypeSpecs } from "../../ts/level";

const sample = (): LevelFile => ({
  format: "nge-level",
  version: 1,
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
  expect(parseLevel(raw)).toEqual({ ok: true, level: { format: "nge-level", version: 1, name: "", night: false, tiles: [], entities: [{ id: "a", type: "coin", x: 1, y: 2 }] } });
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
