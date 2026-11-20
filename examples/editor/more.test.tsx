// 编辑器补齐的操作（store.ts，docs/editor-more.md）：层、瓦片规则、层级、路线点、覆盖、悬停、自动滚动、缩放中心、试玩热重载。
import { expect, test } from "nsh/test";
import { formatLevel, parseLevel } from "../../ts/level";
import { memoryStore } from "./files";
import { blankLevel, createStore, outsideMap } from "./store";
import { edgeScroll } from "./view";

const setup = (answer: string | null = null) => {
  let t = 0;
  return createStore({ files: async () => memoryStore(), ask: async () => answer, now: () => (t += 1000) });
};
/** 视图在世界原点、800 × 600：世界 (x, y) 在屏幕上是 (400 + x, 300 - y)。 */
const centered = (answer: string | null = null) => {
  const s = setup(answer);
  s.view.set({ x: 0, y: 0, zoom: 1 });
  s.size.set([800, 600]);
  return s;
};
const screen = (x: number, y: number): [number, number] => [400 + x, 300 - y];

test("层：加一层成为当前层，刷子画在当前层；删当前层、挪层后当前层跟着；撤销回来", () => {
  const s = centered();
  s.addLayer();
  expect(s.level().tiles.length).toBe(2);
  expect(s.layerIndex()).toBe(1);
  const t = s.level().tiles[1];
  s.tool.set("brush");
  s.frame.set(2);
  // 第 2 层左上角是 (0, 448)，第 0 行第 0 列的中间
  s.view.set({ x: 0, y: 448, zoom: 1 });
  s.pointer("down", [416, 316]);
  s.pointer("up", [416, 316]);
  expect(s.level().tiles[1].data[0][0]).toBe(2);
  expect(s.level().tiles[0].data[0][0]).toBe(-1);
  expect(t.id).toBe("layer2");
  s.moveLayerBy(-1);
  expect(s.level().tiles.map((x) => x.id)).toEqual(["layer2", "level"]);
  expect(s.layerIndex()).toBe(0);
  s.removeLayer();
  expect(s.level().tiles.map((x) => x.id)).toEqual(["level"]);
  s.removeLayer();
  expect(s.status()).toContain("最后一层不能删");
  s.undo();
  s.undo();
  expect(s.level().tiles.map((x) => x.id)).toEqual(["level", "layer2"]);
});

test("锁上、藏起来的层画不上，状态栏说原因；显隐、锁定能撤销", () => {
  const s = centered();
  s.toggleLayer(0, "locked");
  expect(s.level().tiles[0].locked).toBe(true);
  s.tool.set("fill");
  s.view.set({ x: 0, y: 448, zoom: 1 });
  s.pointer("down", [416, 316]);
  s.pointer("up", [416, 316]);
  expect(s.level().tiles[0].data[0][0]).toBe(-1);
  expect(s.status()).toContain("锁着");
  s.toggleLayer(0, "locked");
  expect("locked" in s.level().tiles[0]).toBe(false);
  s.toggleLayer(0, "hidden");
  s.tool.set("brush");
  s.pointer("down", [416, 316]);
  expect(s.status()).toContain("藏着");
  s.undo();
  expect(s.level().tiles[0].hidden).toBe(undefined);
});

test("矩形和油漆桶：在当前层上，一步撤销", () => {
  const s = centered();
  s.view.set({ x: 0, y: 448, zoom: 1 });
  s.frame.set(0);
  s.tool.set("rect");
  // 从 (0, 0) 格拉到 (1, 2) 格
  s.pointer("down", [416, 316]);
  s.pointer("move", [416 + 64, 316 + 32]);
  expect(s.sceneInput().level.tiles[0].data[1][2]).toBe(0);
  expect(s.level().tiles[0].data[1][2]).toBe(-1);
  s.pointer("up", [416 + 64, 316 + 32]);
  expect(s.level().tiles[0].data.slice(0, 2).map((r) => r.slice(0, 4))).toEqual([[0, 0, 0, -1], [0, 0, 0, -1]]);
  s.tool.set("fill");
  s.frame.set(10);
  s.pointer("down", [416 + 32 * 5, 316]);
  s.pointer("up", [416 + 32 * 5, 316]);
  // 空格全换成水（底下的草地和刚画的石块不动）
  expect(s.level().tiles[0].data[0][5]).toBe(10);
  expect(s.level().tiles[0].data[13][59]).toBe(10);
  expect(s.level().tiles[0].data[14][0]).toBe(1);
  expect(s.level().tiles[0].data[0][0]).toBe(0);
  s.undo();
  expect(s.level().tiles[0].data[0][5]).toBe(-1);
});

test("视差层：刷子按画出来的位置画", () => {
  const s = centered();
  s.addLayer();
  s.setLayerField("px", "0.5");
  expect(s.level().tiles[1].parallax).toEqual([0.5, 1]);
  // 相机在 x = 64：这一层画在右边 32 处；屏幕 448 是世界 112、这一层的 80，第 2 格（没有视差会是第 3 格）
  s.view.set({ x: 64, y: 448, zoom: 1 });
  s.tool.set("brush");
  s.frame.set(2);
  s.pointer("down", [400 + 48, 316]);
  s.pointer("up", [400 + 48, 316]);
  expect(s.level().tiles[1].data[0].slice(0, 4)).toEqual([-1, -1, 2, -1]);
  // 视差设回 1：去掉这一项
  s.setLayerField("px", "1");
  expect(s.level().tiles[1].parallax).toBe(undefined);
  expect(s.setLayerField("py", "9")).toContain("0 到 4");
  expect(s.setLayerField("name", "a\u0007")).toContain("控制字符");
  s.setLayerField("z", "-5");
  expect(s.level().tiles[1].z).toBe(-5);
  s.setLayerField("z", "");
  expect(s.level().tiles[1].z).toBe(undefined);
});

test("瓦片规则：改种类、坡的两头、动画、摩擦，写错的说原因不改；存得下来", () => {
  const s = setup();
  expect(s.setFrameRule(2, "kind", "oneWay")).toBe(null);
  const t = () => s.level().tiles[0];
  expect([t().solid.includes(2), t().oneWay?.includes(2)]).toEqual([false, true]);
  expect(s.setFrameRule(2, "left", "0.5")).toContain("不是斜坡");
  s.setFrameRule(2, "kind", "slope");
  expect(t().slopes?.find((x) => x.frame === 2)).toEqual({ frame: 2, left: 0, right: 1 });
  expect(s.setFrameRule(2, "right", "0")).toContain("不能都是 0");
  s.setFrameRule(2, "left", "0.5");
  expect(t().slopes?.find((x) => x.frame === 2)).toEqual({ frame: 2, left: 0.5, right: 1 });
  expect(s.setFrameRule(2, "fps", "4")).toContain("先填动画帧");
  expect(s.setFrameRule(2, "frames", "2, 99")).toContain("帧号");
  s.setFrameRule(2, "frames", "2，3 4");
  s.setFrameRule(2, "fps", "8");
  expect(t().animated?.find((x) => x.frame === 2)).toEqual({ frame: 2, frames: [2, 3, 4], fps: 8 });
  s.setFrameRule(2, "friction", "0.02");
  expect(s.setFrameRule(2, "restitution", "-1")).toContain("0 到 10");
  expect(t().materials).toEqual([{ frame: 2, friction: 0.02 }]);
  s.setFrameRule(2, "friction", "");
  expect(t().materials).toBe(undefined);
  expect(parseLevel(formatLevel(s.level())).ok).toBe(true);
  // 一样的不进历史
  const steps = s.editor().history.done.length;
  s.setFrameRule(2, "kind", "slope");
  expect(s.editor().history.done.length).toBe(steps);
});

test("层级：填父节点、拖到另一行上；不能放进自己的子孙；挪、删、复制父节点带着子孙", () => {
  const s = centered();
  s.addAtCenter("group");
  const group = s.selection()[0];
  s.addAtCenter("coin");
  const coin = s.selection()[0];
  expect(s.setField(coin, "parent", group)).toBe(null);
  expect(s.entity(coin)!.parent).toBe(group);
  expect(s.setField(group, "parent", coin)).toContain("不能放进自己");
  expect(s.setField(group, "parent", "nope")).toContain("没有 nope");
  // 改父节点的 x：孩子挪一样多，一步撤销
  const x = s.entity(coin)!.x;
  s.setField(group, "x", String(s.entity(group)!.x + 100));
  expect(s.entity(coin)!.x).toBe(x + 100);
  s.undo();
  expect(s.entity(coin)!.x).toBe(x);
  // 方向键微移父节点
  s.selection.set([group]);
  s.nudge(1, 0);
  expect(s.entity(coin)!.x).toBe(x + 1);
  // 画布上拖父节点
  const g = s.entity(group)!;
  s.pointer("down", screen(g.x, g.y));
  s.pointer("move", screen(g.x + 64, g.y));
  s.pointer("up", screen(g.x + 64, g.y));
  expect(s.entity(coin)!.x).not.toBe(x + 1);
  expect(s.entity(coin)!.x - s.entity(group)!.x).toBe(x + 1 - (g.x));
  // 复制父节点：孩子也复制，挂在新的父节点下面
  s.selection.set([group]);
  s.duplicate();
  const copies = s.level().entities.slice(-2);
  expect(copies[1].parent).toBe(copies[0].id);
  // 删父节点：孩子一起删
  s.selection.set([group]);
  s.remove();
  expect(s.entity(coin)).toBe(undefined);
  s.undo();
  expect(s.entity(coin)!.parent).toBe(group);
  // 拖到另一行上：放回最外层
  s.selection.set([coin]);
  s.dropOn(coin, null);
  expect(s.entity(coin)!.parent).toBe(undefined);
  s.dropOn(coin, coin);
  expect(s.entity(coin)!.parent).toBe(undefined);
  expect(s.status()).toContain("不能放进自己");
  // 折叠
  s.toggleCollapse(group);
  expect(s.collapsed()).toEqual([group]);
  s.toggleCollapse(group);
  expect(s.collapsed()).toEqual([]);
});

test("旋转和 z：只有能转、能单独设 z 的类型才行", () => {
  const s = setup();
  s.addAtCenter("deco");
  const deco = s.selection()[0];
  expect(s.setField(deco, "rotation", "30")).toBe(null);
  expect(s.setField(deco, "z", "-4")).toBe(null);
  expect([s.entity(deco)!.rotation, s.entity(deco)!.z]).toEqual([30, -4]);
  s.setField(deco, "rotation", "0");
  expect("rotation" in s.entity(deco)!).toBe(false);
  expect(s.setField("e2", "rotation", "30")).toContain("不能转");
  expect(s.setField("e2", "z", "1")).toContain("由类型定");
  expect(s.setField(deco, "rotation", "99999")).toContain("之间");
});

test("路线点：加、删，画布上按住拖（按网格吸附），一步撤销", () => {
  const s = centered();
  s.grid.set(0);
  s.addAtCenter("walker");
  const w = s.selection()[0];
  s.addRoutePoint(w);
  s.addRoutePoint(w);
  const e = s.entity(w)!;
  expect(e.points).toEqual([[e.x + 32, e.y], [e.x + 64, e.y]]);
  s.removeRoutePoint(w, 0);
  expect(s.entity(w)!.points).toEqual([[e.x + 64, e.y]]);
  // 拖这个点
  const [px, py] = s.entity(w)!.points![0];
  s.pointer("down", screen(px, py));
  s.pointer("move", screen(px + 10, py + 20));
  expect(s.sceneInput().level.entities.find((x) => x.id === w)!.points).toEqual([[px + 10, py + 20]]);
  s.pointer("up", screen(px + 10, py + 20));
  expect(s.entity(w)!.points).toEqual([[px + 10, py + 20]]);
  // 走地怪本身没动
  expect(s.entity(w)!.x).toBe(e.x);
  s.undo();
  expect(s.entity(w)!.points).toEqual([[px, py]]);
  s.removeRoutePoint(w, 0);
  expect("points" in s.entity(w)!).toBe(false);
  // 不是走地怪的加不了
  s.addRoutePoint("e2");
  expect(s.entity("e2")!.points).toBe(undefined);
});

test("预制体覆盖：加、换、删，写错的说原因；不是预制体的不行", () => {
  const s = setup();
  s.addAtCenter("prefab");
  const p = s.selection()[0];
  expect(s.addOverride(p, "b/crown", "y", "110")).toBe(null);
  expect(s.addOverride(p, "b/crown", "y", "90")).toBe(null);
  expect(s.entity(p)!.set).toEqual({ "b/crown": { y: 90 } });
  expect(s.addOverride(p, "b//crown", "y", "1")).toContain("子节点");
  expect(s.addOverride("e2", "a", "y", "1")).toContain("不是预制体");
  s.removeOverride(p, "b/crown", "y");
  expect("set" in s.entity(p)!).toBe(false);
  s.undo();
  expect(s.entity(p)!.set).toEqual({ "b/crown": { y: 90 } });
});

test("悬停：选择工具下指针下面的实体；拖动中、别的工具、出了画布都没有", () => {
  const s = centered();
  const crate = s.entity("e2")!;
  s.hoverAt(screen(crate.x, crate.y));
  expect(s.hover()).toBe("e2");
  expect(s.sceneInput().hover).toBe("e2");
  s.hoverAt(screen(crate.x, crate.y + 500));
  expect(s.hover()).toBe(null);
  s.hoverAt(screen(crate.x, crate.y));
  s.hoverAt(null);
  expect(s.hover()).toBe(null);
  s.tool.set("brush");
  s.hoverAt(screen(crate.x, crate.y));
  expect(s.hover()).toBe(null);
});

test("拖到画布边上自动滚动：视图往那边挪，拖着的东西跟着指针；不在拖或不在边上不动", () => {
  expect(edgeScroll([800, 600], [400, 300])).toEqual([0, 0]);
  expect(edgeScroll([800, 600], [799, 300])[0]).toBeGreaterThan(edgeScroll([800, 600], [780, 300])[0]);
  expect(edgeScroll([800, 600], [-50, 300])).toEqual([-12, 0]);
  expect(edgeScroll([800, 600], [400, 590])[1]).toBeGreaterThan(0);
  expect(edgeScroll([40, 40], [1, 1])).toEqual([0, 0]);
  const s = centered();
  s.grid.set(0);
  const crate = s.entity("e2")!;
  s.autoScroll();
  expect(s.view().x).toBe(0);
  s.pointer("down", screen(crate.x, crate.y));
  // 拖到右边边上
  s.pointer("move", [798, 300 - crate.y]);
  const dragged = s.sceneInput().level.entities.find((e) => e.id === "e2")!.x;
  s.autoScroll();
  expect(s.view().x).toBeGreaterThan(0);
  // 指针没动，视图挪了：拖着的东西跟着往右
  expect(s.sceneInput().level.entities.find((e) => e.id === "e2")!.x).toBeGreaterThan(dragged);
  // 拖到下边边上：视图往下（世界 y 变小），拖着的东西也往下
  s.pointer("move", [400 + crate.x, 598]);
  const low = s.sceneInput().level.entities.find((e) => e.id === "e2")!.y;
  const vy = s.view().y;
  s.autoScroll();
  expect(s.view().y).toBeLessThan(vy);
  expect(s.sceneInput().level.entities.find((e) => e.id === "e2")!.y).toBeLessThanOrEqual(low);
  s.pointer("up", [400 + crate.x, 598]);
  const vx = s.view().x;
  s.autoScroll();
  expect(s.view().x).toBe(vx);
});

test("⌘+ / ⌘- 缩放：指针在画布上时以指针为中心（指针下的世界点不动），不然以画布中心", () => {
  const s = centered();
  s.zoomBy(2);
  expect(s.view()).toEqual({ x: 0, y: 0, zoom: 2 });
  s.view.set({ x: 0, y: 0, zoom: 1 });
  s.hoverAt([600, 100]);
  s.zoomBy(2);
  // (600, 100) 下的世界点是 (200, 200)，缩放后还在那儿
  expect(s.view()).toEqual({ x: 100, y: 100, zoom: 2 });
});

test("试玩热重载：改了关卡正在玩的就地换掉（瓦片、实体、名字）；不合规的留着上一次的，状态栏说原因", () => {
  const s = setup();
  s.play();
  const first = s.playing()!;
  s.addAtCenter("coin");
  expect(s.playing()!.built.coins.length).toBe(first.built.coins.length + 1);
  s.setLevelField("name", "改了名");
  expect(s.playing()!.name).toBe("改了名");
  s.selection.set(["e3"]);
  s.remove();
  expect(s.status()).toContain("试玩里先不更新");
  expect(s.playing()!.built.goal.x).toBe(first.built.goal.x);
  s.undo();
  expect(s.playing()!.built.goal.x).toBe(first.built.goal.x);
  s.stop();
  // 不在试玩时不动
  s.addAtCenter("coin");
  expect(s.playing()).toBe(null);
});

test("出生点、终点在不在地图里：按没有视差的层合起来算", () => {
  const level = blankLevel();
  const far = { ...level.tiles[0], id: "far", x: -5000, parallax: [0.5, 1] as [number, number] };
  const out = { ...level, tiles: [level.tiles[0], far], entities: level.entities.map((e) => (e.type === "goal" ? { ...e, x: -100 } : e)) };
  expect(outsideMap(out)).toEqual(["终点在地图外面"]);
  const wide = { ...out, tiles: [level.tiles[0], { ...level.tiles[0], id: "left", x: -320 }] };
  expect(outsideMap(wide)).toEqual([]);
});

test("挪走地怪（拖、方向键、改 x / y、挪它的父节点）时路线点一起平移；撤销一起还原", () => {
  const s = centered();
  s.grid.set(0);
  s.addAtCenter("walker");
  const w = s.selection()[0];
  s.addRoutePoint(w);
  const pts = () => s.entity(w)!.points!;
  const [px, py] = pts()[0];
  // 属性面板改 x
  s.setField(w, "x", String(s.entity(w)!.x + 50));
  expect(pts()).toEqual([[px + 50, py]]);
  s.undo();
  expect(pts()).toEqual([[px, py]]);
  // 方向键
  s.selection.set([w]);
  s.nudge(3, 0);
  expect(pts()).toEqual([[px + 3, py]]);
  s.undo();
  // 画布上拖（拖的时候预览里也跟着）
  const e = s.entity(w)!;
  s.pointer("down", screen(e.x, e.y));
  s.pointer("move", screen(e.x + 40, e.y));
  expect(s.sceneInput().level.entities.find((x) => x.id === w)!.points).toEqual([[px + 40, py]]);
  s.pointer("up", screen(e.x + 40, e.y));
  expect(pts()).toEqual([[px + 40, py]]);
  s.undo();
  expect(pts()).toEqual([[px, py]]);
  // 挪父节点
  s.addAtCenter("group");
  const g = s.selection()[0];
  s.setField(w, "parent", g);
  s.setField(g, "x", String(s.entity(g)!.x + 20));
  expect(pts()).toEqual([[px + 20, py]]);
  s.undo();
  expect(pts()).toEqual([[px, py]]);
});

test("拖几行放进一个分组：不合规的（分组本身、它的祖先）跳过，别的照放，状态栏说跳过了几个；全不合规才不放", () => {
  const s = centered();
  s.addAtCenter("group");
  const outer = s.selection()[0];
  s.addAtCenter("group");
  const inner = s.selection()[0];
  s.setField(inner, "parent", outer);
  s.addAtCenter("coin");
  const coin = s.selection()[0];
  // 选中金币和外层分组，拖到里层分组上：外层放不进（是里层的祖先），金币照放
  s.selection.set([coin, outer]);
  s.dropOn(coin, inner);
  expect(s.entity(coin)!.parent).toBe(inner);
  expect(s.entity(outer)!.parent).toBe(undefined);
  expect(s.status()).toContain("跳过了 1 个");
  // 只有不合规的：不放，说原因
  s.selection.set([outer]);
  s.dropOn(outer, inner);
  expect(s.entity(outer)!.parent).toBe(undefined);
  expect(s.status()).toContain("不能放进自己");
});

test("粘贴时父节点没一起复制：还是原来那个就留在它下面（删了又撤销回来也算）；id 被新东西重用过、换了关卡就放到最外层", async () => {
  const s = centered("discard");
  s.addAtCenter("group");
  const g = s.selection()[0];
  s.addAtCenter("coin");
  const coin = s.selection()[0];
  s.setField(coin, "parent", g);
  s.selection.set([coin]);
  s.copy();
  // 原来那个分组还在：粘贴进同一个分组
  s.paste();
  expect(s.entity(s.selection()[0])!.parent).toBe(g);
  s.undo();
  // 删了分组又撤销回来（还是原来那个）：照样粘贴进去
  s.selection.set([g]);
  s.remove();
  s.undo();
  s.paste();
  expect(s.entity(s.selection()[0])!.parent).toBe(g);
  s.undo();
  // 删了分组（金币一起删），新放一个平台拿到同一个 id：粘贴出来的不挂到平台下面
  s.selection.set([g]);
  s.remove();
  s.addAtCenter("lift");
  expect(s.selection()[0]).toBe(g);
  s.paste();
  expect(s.entity(s.selection()[0])!.parent).toBe(undefined);
  // 换了关卡：放到最外层
  await s.newLevel();
  s.addAtCenter("group");
  expect(s.selection()[0]).toBe(g);
  s.paste();
  expect(s.entity(s.selection()[0])!.parent).toBe(undefined);
});

test("试玩中新建、打开关卡：先停掉试玩，不把正在玩的换成另一关（起点还是旧关的）", async () => {
  const s = centered("discard");
  s.addAtCenter("coin");
  s.play();
  expect(s.playing()).not.toBe(null);
  await s.newLevel();
  expect(s.playing()).toBe(null);
  expect(s.level().entities.length).toBe(3);
});

test("试玩热重载不合规以后又改好了：状态栏不再停在「先不更新」", () => {
  const s = setup();
  s.play();
  s.selection.set(["e3"]);
  s.remove();
  expect(s.status()).toContain("试玩里先不更新");
  s.undo();
  expect(s.status()).not.toContain("先不更新");
  expect(s.status()).toContain("试玩中");
});

test("所有层都有视差（没有挡人的地面）：不能试玩，说原因", () => {
  const level = blankLevel();
  const floating = { ...level, tiles: level.tiles.map((t) => ({ ...t, parallax: [0.5, 1] as [number, number] })) };
  expect(outsideMap(floating)).toEqual(["没有挡人的瓦片层（都有视差）"]);
});
