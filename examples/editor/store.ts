// 编辑器的状态和操作（docs/editor.md、docs/editor-more.md）：放在组件外面，试玩回来时原样还在。界面只调这里的函数。
import { batch, signal } from "nsh";
import { checkTypes, emptyLevel, formatLevel, parseLevel, type Entity, type LevelFile } from "../../ts/level";
import type { CameraView } from "../../ts/nge";
import { boundsOf, labelOf, newEntity, placed, sizeOf, TILESET, TYPES } from "../platformer/catalog";
import { depthOf } from "../platformer/preview";
import { fromFile, type Built } from "../platformer/levels";
import { copy, nextId, pasted } from "./clipboard";
import { addEntities, compose, moveTo, removeEntities, reorder, resizeLayer, setLevel, updateEntity } from "./commands";
import { fieldPatch, nameError, parseFrames, parseNumber, parseOptional, parseParallax, parseSize } from "./fields";
import { canParent, descendantsOf, setParent, withDescendants } from "./hierarchy";
import { addLayer, frameRule, moveLayer, removeLayer, rulePatch, updateLayer, type FrameRule, type LayerPatch } from "./layers";
import { parseOverride, withoutOverride, withOverride } from "./overrides";
import { addPoint, removePoint } from "./points";
export { EDIT_MAX_COLS, EDIT_MAX_ROWS } from "./fields";
import { fileName, openFrom, saveTo, type LevelStore } from "./files";
import { upgradeTiles } from "./tiles";
import { execute, isDirty, markSavedAt, newEditor, redo, savePoint, undo, type Command } from "./history";
import type { SceneInput } from "./scene";
import { prune, toggle } from "./select";
import { snapPoint } from "./snap";
import { hoverOf, IDLE, pointerDown, pointerMove, pointerUp, previewOf, type Ctx, type Drag, type Out, type ToolName } from "./tool";
import { edgeScroll, pan, zoomAt } from "./view";

/** 问用户的对话框：返回选中的动作 id，关掉是 null。 */
export type Ask = (title: string, description: string, actions: { id: string; label: string; role?: "cancel" | "destructive" }[]) => Promise<string | null>;

export interface Deps {
  files: () => Promise<LevelStore>;
  ask: Ask;
  now?: () => number;
}

export interface Playing {
  name: string;
  built: Built;
  night: boolean;
}

/** 新建的关卡：60 × 15，底下一行草地，出生点、箱子、终点都摆好了（不然没法试玩）。 */
export function blankLevel(): LevelFile {
  const level = emptyLevel({ name: "新关卡", cols: 60, rows: 15, ...TILESET });
  const tiles = level.tiles.map((t) => ({ ...t, friction: 0, data: t.data.map((row, r) => (r === t.rows - 1 ? row.map(() => 1) : row)) }));
  const entities: Entity[] = [newEntity("spawn", "e1", 96, 16), newEntity("crate", "e2", 1600, 20), newEntity("goal", "e3", 1792, 40)];
  return { ...level, tiles, entities };
}

/** 出生点、终点要在地图里（没有视差的瓦片层合起来的范围），不然一开始就掉下去或者到不了。 */
export function outsideMap(level: LevelFile): string[] {
  const ground = level.tiles.filter((t) => t.parallax === undefined || (t.parallax[0] === 1 && t.parallax[1] === 1));
  if (ground.length === 0) return ["没有挡人的瓦片层（都有视差）"];
  const left = Math.min(...ground.map((t) => t.x));
  const right = Math.max(...ground.map((t) => t.x + t.cols * t.tile));
  const top = Math.max(...ground.map((t) => t.y));
  const bottom = Math.min(...ground.map((t) => t.y - t.rows * t.tile));
  return level.entities
    .filter((e) => (e.type === "spawn" || e.type === "goal") && (e.x < left || e.x > right || e.y < bottom || e.y > top))
    .map((e) => `${labelOf({ ...e, name: undefined })}在地图外面`);
}

/** 状态栏：试玩中；热重载时关卡不合规、没换掉正在玩的。 */
const PLAYING = "试玩中，按 Esc 回到编辑";
const STALE = "试玩里先不更新";

export const START_VIEW: CameraView = { x: 480, y: 200, zoom: 1 };

export function createStore(deps: Deps) {
  const now = deps.now ?? Date.now;
  const editor = signal(newEditor(blankLevel()));
  const selection = signal<string[]>([]);
  const view = signal<CameraView>(START_VIEW);
  const size = signal<[number, number]>([800, 600]);
  const tool = signal<ToolName>("select");
  const placeType = signal<string | null>(null);
  const frame = signal(1);
  const grid = signal(32);
  const drag = signal<Drag>(IDLE);
  const file = signal<string | null>(null);
  const status = signal("");
  const panel = signal<null | "open" | "saveAs">(null);
  const files = signal<string[]>([]);
  const playing = signal<Playing | null>(null);
  /** 当前瓦片层（刷子、矩形、油漆桶画这一层，层面板、关卡设置里的大小也是它）。 */
  const layer = signal(0);
  /** 场景树里折起来的分组。 */
  const collapsed = signal<string[]>([]);
  /** 指针下面的实体（悬停高亮）、指针最后在画布上的位置（自动滚动、⌘+ 缩放的中心）。 */
  const hover = signal<string | null>(null);
  const pointerAt = signal<[number, number] | null>(null);
  let clipboard: Entity[] = [];
  /** 剪贴板里的东西挂在哪些没一起复制的父节点下面：复制以后这个 id 被新东西用了（或者换了关卡）就去掉，粘贴时不再挂上去。 */
  let outside = new Set<string>();

  const level = () => editor().level;
  /** 当前层的下标（层被删了时夹到范围里）。 */
  const layerIndex = () => Math.max(0, Math.min(layer(), level().tiles.length - 1));
  const dirty = () => isDirty(editor());
  const cell = () => (grid() > 0 ? grid() : TILESET.tile);
  const entity = (id: string) => level().entities.find((e) => e.id === id);

  /** 试玩时关卡变了：合规就换掉正在玩的（热重载），不合规的留着上一次的，状态栏说为什么。 */
  const relive = () => {
    if (playing() === null) return;
    const r = trialOf(level());
    if (!r.ok) return status.set(`${STALE}：${r.errors.slice(0, 2).join("；")}`);
    playing.set(r.playing);
    // 上一次没更新、这次好了：状态栏换回来
    if (status().startsWith(STALE)) status.set(PLAYING);
  };
  // 关卡变了，正在拖的就作废（拖着的东西可能没了，一笔的旧值也不对了）
  const run = (cmd: Command, select?: string[]) => {
    batch(() => {
      drag.set(IDLE);
      const before = new Set(level().entities.map((e) => e.id));
      editor.set(execute(editor(), cmd, now()));
      // 新建出来的 id 碰上了剪贴板记着的父节点：那已经不是原来那个了
      for (const e of level().entities) if (!before.has(e.id)) outside.delete(e.id);
      selection.set(prune(select ?? selection(), level()));
      relive();
    });
  };
  const travel = (fn: typeof undo) => {
    batch(() => {
      drag.set(IDLE);
      editor.set(fn(editor()));
      selection.set(prune(selection(), level()));
      relive();
    });
  };

  /** 删除选中的，连子孙一起。 */
  const remove = () => {
    if (selection().length > 0) run(removeEntities(level(), withDescendants(level(), selection())), []);
  };
  /** 贴进来：`remember` 时剪贴板换成贴进来的（接着粘贴再错开一格），⌘D 不动剪贴板。 */
  const paste = (clip: Entity[], remember: boolean) => {
    if (clip.length === 0) return;
    // ⌘D（不经过剪贴板）复制的就是现在的，父节点总是原来那个
    const added = pasted(level(), clip, [cell(), 0], remember ? (p) => outside.has(p) : undefined);
    if (remember) clipboard = added;
    run(addEntities(added), added.map((e) => e.id));
  };
  /** 微移：选中的挪 (dx, dy)；没选中时平移视图。连着按的合成一步撤销。 */
  const nudge = (dx: number, dy: number) => {
    if (selection().length === 0) return view.set({ ...view(), x: view().x + dx * 4, y: view().y + dy * 4 });
    const chosen = new Set(withDescendants(level(), selection()));
    const targets = level().entities.filter((e) => chosen.has(e.id)).flatMap((e) => {
      const p = placed(e, e.x + dx, e.y + dy);
      return p.x === e.x && p.y === e.y ? [] : [{ id: e.id, x: p.x, y: p.y }];
    });
    // 站在地上的按上下：没变化，不进历史
    if (targets.length === 0) return;
    run(moveTo(level(), targets, `nudge:${selection().join(",")}`));
  };
  /** 在视图中间放一个，吸附，选中它。 */
  const addAtCenter = (type: string) => {
    const id = nextId(level());
    const draft = newEntity(type, id, 0, 0);
    const [x, y] = snapPoint([view().x, view().y], grid(), sizeOf(draft));
    placeType.set(type);
    run(addEntities([placed(draft, x, y)]), [id]);
  };
  /** 属性面板提交一个输入框：合规就改，不然返回原因。改了父节点的 x / y，子孙挪一样多。 */
  const setField = (id: string, field: string, text: string): string | null => {
    const e = entity(id);
    if (e === undefined) return "选中的东西不在了";
    if (field === "parent") return setParentOf([id], text.trim() === "" ? undefined : text.trim());
    const r = fieldPatch(e, field, text);
    if (!r.ok) return r.error;
    const changed = Object.entries(r.value).some(([k, v]) => JSON.stringify(v) !== JSON.stringify(e[k as keyof Entity]));
    if (!changed) return null;
    const [dx, dy] = [(r.value.x ?? e.x) - e.x, (r.value.y ?? e.y) - e.y];
    const below = new Set(dx === 0 && dy === 0 ? [] : descendantsOf(level(), id));
    const kids = level().entities.filter((k) => below.has(k.id));
    // 自己的路线点也跟着平移
    const shifted = e.points === undefined || (dx === 0 && dy === 0) ? r.value : { ...r.value, points: e.points.map(([x, y]): [number, number] => [x + dx, y + dy]) };
    const update = updateEntity(level(), id, shifted);
    run(kids.length === 0 ? update : compose("修改", [update, moveTo(level(), kids.map((k) => ({ id: k.id, x: k.x + dx, y: k.y + dy })))]));
    return null;
  };
  /** 一组实体换父节点（`undefined` 回到最外层）。 */
  const setParentOf = (ids: string[], parent: string | undefined): string | null => {
    if (parent !== undefined && entity(parent) === undefined) return `没有 ${parent}`;
    // 不合规的（会放进自己或自己的子孙里）跳过，别的照放；全不合规才说原因
    const fits = ids.filter((id) => canParent(level(), id, parent));
    if (fits.length === 0) return "不能放进自己或自己的子孙里";
    const cmd = setParent(level(), fits, parent);
    if (cmd !== null) run(cmd);
    if (fits.length < ids.length) status.set(`跳过了 ${ids.length - fits.length} 个：不能放进自己或自己的子孙里`);
    return null;
  };
  /** 场景树里拖一行放到另一行上：选中的（含拖的这一行）放进那一行；`null` 是放回最外层。 */
  const dropOn = (id: string, target: string | null) => {
    const ids = selection().includes(id) ? selection() : [id];
    const why = setParentOf(ids, target ?? undefined);
    if (why !== null) status.set(why);
  };

  /** 当前层的字段：名字、视差 x / y、z。 */
  const setLayerField = (field: "name" | "px" | "py" | "z", text: string): string | null => {
    const i = layerIndex();
    const t = level().tiles[i];
    if (t === undefined) return "没有瓦片层";
    let patch: LayerPatch;
    if (field === "name") {
      if (text.length > 64 || /[\u0000-\u001f\u007f]/.test(text)) return "名字太长或有控制字符";
      patch = { name: text.trim() === "" ? undefined : text };
    } else if (field === "z") {
      const r = parseOptional(text, -1e6, 1e6);
      if (!r.ok) return r.error;
      patch = { z: r.value };
    } else {
      const r = parseParallax(text);
      if (!r.ok) return r.error;
      const [px, py] = t.parallax ?? [1, 1];
      const next: [number, number] = field === "px" ? [r.value, py] : [px, r.value];
      patch = { parallax: next[0] === 1 && next[1] === 1 ? undefined : next };
    }
    const changed = Object.entries(patch).some(([k, v]) => JSON.stringify(v) !== JSON.stringify(t[k as keyof typeof t]));
    if (changed) run(updateLayer(level(), i, patch));
    return null;
  };
  /** 当前层上 `frame` 这一帧的规则的一项：种类、坡的两头、动画帧、fps、摩擦、弹性。 */
  const setFrameRule = (frame: number, field: "kind" | "left" | "right" | "frames" | "fps" | "friction" | "restitution", text: string): string | null => {
    const i = layerIndex();
    const t = level().tiles[i];
    if (t === undefined) return "没有瓦片层";
    const old = frameRule(t, frame);
    let rule: FrameRule = old;
    if (field === "kind") {
      if (!["solid", "oneWay", "slope", "none"].includes(text)) return "种类不对";
      rule = { ...old, kind: text as FrameRule["kind"] };
    } else if (field === "left" || field === "right") {
      if (old.kind !== "slope") return "不是斜坡";
      const r = parseNumber(text, 0, 1);
      if (!r.ok) return r.error;
      const slope = { ...old.slope!, [field]: r.value };
      if (slope.left === 0 && slope.right === 0) return "两头不能都是 0";
      rule = { ...old, slope };
    } else if (field === "frames") {
      const r = parseFrames(text, t.grid[0] * t.grid[1]);
      if (!r.ok) return r.error;
      const { animation: _a, ...rest } = old;
      rule = r.value === undefined ? rest : { ...old, animation: { frames: r.value, fps: old.animation?.fps ?? 4 } };
    } else if (field === "fps") {
      if (old.animation === undefined) return "先填动画帧";
      const r = parseNumber(text, 0.1, 60);
      if (!r.ok) return r.error;
      rule = { ...old, animation: { ...old.animation, fps: r.value } };
    } else {
      const r = parseOptional(text, 0, 10);
      if (!r.ok) return r.error;
      const { [field]: _gone, ...rest } = old;
      rule = r.value === undefined ? rest : { ...old, [field]: r.value };
    }
    if (JSON.stringify(rule) === JSON.stringify(old)) return null;
    run(updateLayer(level(), i, rulePatch(t, frame, rule)));
    return null;
  };
  const layerOps = {
    selectLayer: (i: number) => layer.set(i),
    addLayer: () => {
      run(addLayer(level(), layerIndex()));
      layer.set(layerIndex() + 1);
    },
    removeLayer: () => {
      const cmd = removeLayer(level(), layerIndex());
      if (cmd === null) return status.set("最后一层不能删");
      run(cmd);
      layer.set(layerIndex());
    },
    moveLayerBy: (delta: number) => {
      const from = layerIndex();
      const cmd = moveLayer(level(), from, from + delta);
      if (cmd === null) return;
      run(cmd);
      layer.set(from + delta);
    },
    toggleLayer: (i: number, key: "hidden" | "locked") => {
      const t = level().tiles[i];
      if (t !== undefined) run(updateLayer(level(), i, { [key]: t[key] === true ? undefined : true }));
    },
  };

  /** 选中的走地怪加一个路线点 / 删掉第 i 个。 */
  const addRoutePoint = (id: string) => {
    const e = entity(id);
    if (e === undefined || TYPES[e.type]?.route !== true) return;
    const points = addPoint(e, cell());
    if (points.length !== (e.points ?? []).length) run(updateEntity(level(), id, { points }));
  };
  const removeRoutePoint = (id: string, i: number) => {
    const e = entity(id);
    if (e?.points?.[i] !== undefined) run(updateEntity(level(), id, { points: removePoint(e, i) }));
  };
  /** 预制体实例加一条覆盖（同一项再加是换掉）、删一条。 */
  const addOverride = (id: string, path: string, field: string, text: string): string | null => {
    const e = entity(id);
    if (e === undefined || e.type !== "prefab") return "选中的不是预制体";
    const r = parseOverride(path, field, text);
    if (!r.ok) return r.error;
    if (e.set?.[r.value.path]?.[r.value.field] === r.value.value) return null;
    if (Object.keys(e.set ?? {}).length >= 32 && e.set?.[r.value.path] === undefined) return "最多 32 个子节点";
    run(updateEntity(level(), id, { set: withOverride(e.set, r.value.path, r.value.field, r.value.value) }));
    return null;
  };
  const removeOverride = (id: string, path: string, field: string) => {
    const e = entity(id);
    if (e?.set?.[path]?.[field] !== undefined) run(updateEntity(level(), id, { set: withoutOverride(e.set, path, field) }));
  };
  /** 关卡设置：名字、夜里、宽（列）、高（行）。 */
  const setLevelField = (field: "name" | "night" | "cols" | "rows", text: string): string | null => {
    if (field === "name") {
      const why = nameError(text);
      if (why !== null) return why;
      return level().name === text ? null : (run(setLevel(level(), { name: text })), null);
    }
    if (field === "night") {
      const night = text === "true";
      return level().night === night ? null : (run(setLevel(level(), { night })), null);
    }
    const t = level().tiles[layerIndex()];
    if (t === undefined) return "没有瓦片层";
    const r = parseSize(text, field === "rows");
    if (!r.ok) return r.error;
    const [cols, rows] = field === "cols" ? [r.value, t.rows] : [t.cols, r.value];
    if (cols !== t.cols || rows !== t.rows) run(resizeLayer(level(), layerIndex(), cols, rows));
    return null;
  };

  /** 有没保存的修改时先问：保存（有文件名才行）、不保存、取消。返回能不能接着往下走。 */
  const guard = async (): Promise<boolean> => {
    if (!dirty()) return true;
    const answer = await deps.ask("有没保存的修改", "要先保存吗？", [{ id: "save", label: "保存" }, { id: "discard", label: "不保存", role: "destructive" }, { id: "cancel", label: "取消", role: "cancel" }]);
    if (answer === "discard") return true;
    if (answer !== "save") return false;
    return save();
  };
  const reset = (next: LevelFile, name: string | null) => {
    batch(() => {
      editor.set(newEditor(next));
      outside = new Set();
      selection.set([]);
      file.set(name);
      view.set(START_VIEW);
      panel.set(null);
      layer.set(0);
      collapsed.set([]);
      hover.set(null);
      // 换了一关：试玩停掉（正在玩的是上一关，起点也是上一关的）
      playing.set(null);
    });
  };
  const newLevel = async () => {
    if (!(await guard())) return;
    reset(blankLevel(), null);
    status.set("新建了一关");
  };
  /** 重新列一遍文件；列不出来返回 false。 */
  const refreshFiles = async (): Promise<boolean> => {
    try {
      files.set(await (await deps.files()).list());
      return true;
    } catch (e) {
      files.set([]);
      status.set(`列不出关卡文件：${e instanceof Error ? e.message : String(e)}`);
      return false;
    }
  };
  const open = async (name: string) => {
    if (!(await guard())) return;
    const r = await openFrom(await deps.files(), name);
    if (!r.ok) return status.set(`打不开 ${name}：${r.errors.slice(0, 3).join("；")}`);
    const { level: opened, upgraded } = upgradeTiles(r.level);
    reset(opened, name);
    const problems = checkTypes(opened, TYPES);
    const notes = [...(upgraded ? ["瓦片按现在的瓦片图升级了，保存后生效"] : []), ...problems.slice(0, 1)];
    status.set(notes.length === 0 ? `打开了 ${name}` : `打开了 ${name}（${notes.join("；")}）`);
  };
  const write = async (name: string): Promise<boolean> => {
    // 记下开始保存时的关卡和位置：写盘时又改了的，写完还是脏的
    const saving = level();
    const serial = savePoint(editor());
    // 存出去的要能读回来（名字、属性都合规），不然不写，免得覆盖掉好文件
    const check = parseLevel(formatLevel(saving));
    if (!check.ok) {
      status.set(`不能保存：${check.errors.slice(0, 3).join("；")}`);
      return false;
    }
    try {
      await saveTo(await deps.files(), name, saving);
    } catch (e) {
      status.set(`保存失败：${e instanceof Error ? e.message : String(e)}`);
      return false;
    }
    batch(() => {
      editor.set(markSavedAt(editor(), serial));
      file.set(name);
      panel.set(null);
      const problems = checkTypes(saving, TYPES);
      status.set(problems.length === 0 ? `保存了 ${name}` : `保存了 ${name}（还不能试玩：${problems[0]}）`);
    });
    return true;
  };
  /** 保存：还没有文件名时打开另存为。 */
  const save = async (): Promise<boolean> => {
    const name = file();
    if (name === null) {
      panel.set("saveAs");
      status.set("先起个文件名");
      return false;
    }
    return write(name);
  };
  const saveAs = async (input: string): Promise<boolean> => {
    const name = fileName(input);
    if (name === null) {
      status.set("文件名只能有字母、数字、中文、- 和 _");
      return false;
    }
    // 列不出已有的文件就不知道会不会覆盖：不写
    if (!(await refreshFiles())) return false;
    // macOS 的文件名不分大小写：只差大小写的也是同一个文件，覆盖时写回它原来的名字
    const same = (a: string | null) => a !== null && a.toLowerCase() === name.toLowerCase();
    const existing = files().find(same);
    if (existing !== undefined && !same(file())) {
      const answer = await deps.ask(`${existing} 已经有了`, "要覆盖它吗？", [{ id: "overwrite", label: "覆盖", role: "destructive" }, { id: "cancel", label: "取消", role: "cancel" }]);
      if (answer !== "overwrite") return false;
    }
    return write(existing ?? name);
  };

  /** 画布上的指针：`shift` 按着点击是加选 / 减选。 */
  const ctx = (): Ctx => ({
    level: level(), selection: selection(), view: view(), size: size(), grid: grid(), tool: tool(), placeType: placeType(),
    frame: frame(), layer: layerIndex(), boundsOf, sizeOf, place: placed, make: newEntity, depthOf,
  });
  const apply = (out: Out) => {
    batch(() => {
      drag.set(out.drag);
      if (out.note !== undefined) status.set(out.note);
      if (out.view !== undefined) view.set(out.view);
      if (out.command !== undefined) run(out.command, out.selection);
      else if (out.selection !== undefined) selection.set(out.selection);
    });
  };
  /** 按下之前选中的：按下就改了选中，被取消时（触屏第二根手指按下）改回去。 */
  let beforeDown: string[] = [];
  const pointer = (phase: "down" | "move" | "up" | "cancel", p: [number, number], shift = false) => {
    const c = ctx();
    const before = drag();
    pointerAt.set(p);
    if (phase !== "move") hover.set(null);
    if (phase === "cancel") {
      return batch(() => {
        if (before.kind !== "idle") selection.set(prune(beforeDown, level()));
        drag.set(IDLE);
      });
    }
    if (phase === "down") {
      beforeDown = selection();
      const out = pointerDown(c, p);
      return apply(shift ? { drag: out.drag } : out);
    }
    if (before.kind === "idle") return;
    if (phase === "move") return apply(pointerMove(c, before, p));
    const out = pointerUp(c, before, p);
    // 按着 shift 点一下：点中的加进来或去掉，点空白不变
    if (shift && before.kind === "press" && before.tool === "select" && out.command === undefined && out.drag.kind === "idle" && out.selection !== undefined) {
      return apply({ drag: IDLE, selection: before.hit === null ? selection() : toggle(selection(), before.hit) });
    }
    apply(out);
  };
  /** 指针在画布上挪（没按着）：记下位置，选择工具下高亮下面的实体；`null` 是出了画布。 */
  const hoverAt = (p: [number, number] | null) => {
    batch(() => {
      pointerAt.set(p);
      hover.set(p === null || drag().kind !== "idle" ? null : hoverOf(ctx(), p));
    });
  };
  /** 拖动、框选、画矩形、拖点时指针在画布边上：视图往那边挪一点，拖着的东西跟着指针重算。定时调（App 里每 33 ms）。 */
  const autoScroll = () => {
    const d = drag();
    const p = pointerAt();
    if (p === null || !["move", "box", "rect", "point", "paint"].includes(d.kind)) return;
    const [dx, dy] = edgeScroll(size(), p);
    if (dx === 0 && dy === 0) return;
    // 视图挪了，按屏幕坐标记的起点也跟着挪，起点下面还是同一个世界点；batch 里读 view() 还是旧值，新视图直接传
    const next = pan(view(), -dx, -dy);
    const kept: Drag = d.kind === "move" || d.kind === "point" ? { ...d, start: [d.start[0] - dx, d.start[1] - dy] } : d;
    batch(() => {
      view.set(next);
      apply(pointerMove({ ...ctx(), view: next }, kept, p));
    });
  };
  /** 画布上要画的：拖动中的预览、框选框、悬停、隐藏的层。 */
  const sceneInput = (): SceneInput => {
    const d = drag();
    return { level: previewOf(ctx(), d), selection: selection(), view: view(), size: size(), grid: grid(), box: d.kind === "box" ? [d.start, d.end] : null, hover: hover() };
  };

  /** 试玩前要查的：格式和类型、出生点和终点在不在地图里；合规的顺便转好（只转一次）。 */
  const trialOf = (l: LevelFile): { ok: true; playing: Playing } | { ok: false; errors: string[] } => {
    const r = fromFile(l);
    if (!r.ok) return r;
    const out = outsideMap(l);
    if (out.length > 0) return { ok: false, errors: out };
    return { ok: true, playing: { name: r.name || "试玩", built: r.built, night: r.night } };
  };
  /** 试玩：先查关卡合不合规，不合规的不进。 */
  const play = () => {
    const r = trialOf(level());
    if (!r.ok) return status.set(`还不能试玩：${r.errors.slice(0, 3).join("；")}`);
    batch(() => {
      drag.set(IDLE);
      playing.set(r.playing);
      status.set(PLAYING);
    });
  };
  const stop = () => {
    batch(() => {
      playing.set(null);
      status.set("回到编辑");
    });
  };

  return {
    editor, selection, view, size, tool, placeType, frame, grid, drag, file, status, panel, files, playing,
    layer, collapsed, hover, pointerAt,
    level, dirty, cell, entity, run, layerIndex,
    undo: () => travel(undo),
    redo: () => travel(redo),
    remove,
    duplicate: () => paste(copy(level(), selection()), false),
    copy: () => {
      clipboard = copy(level(), selection());
      const copied = new Set(clipboard.map((e) => e.id));
      outside = new Set(clipboard.flatMap((e) => (e.parent !== undefined && !copied.has(e.parent) ? [e.parent] : [])));
      status.set(`复制了 ${clipboard.length} 个`);
    },
    paste: () => paste(clipboard, true),
    selectAll: () => selection.set(level().entities.map((e) => e.id)),
    nudge,
    reorderBy: (id: string, delta: number) => {
      const i = level().entities.findIndex((e) => e.id === id);
      // 已经在头上、尾上的不动，不进历史
      const to = Math.max(0, Math.min(level().entities.length - 1, i + delta));
      if (i >= 0 && to !== i) run(reorder(level(), id, to));
    },
    addAtCenter,
    setField,
    setProp: (id: string, key: string, value: string | boolean) => setField(id, `prop:${key}`, String(value)),
    setLevelField,
    newLevel, open, save, saveAs, refreshFiles,
    play, stop,
    pointer, sceneInput, hoverAt, autoScroll,
    setParentOf, dropOn, setLayerField, setFrameRule, ...layerOps,
    addRoutePoint, removeRoutePoint, addOverride, removeOverride,
    toggleCollapse: (id: string) => collapsed.set(collapsed().includes(id) ? collapsed().filter((c) => c !== id) : [...collapsed(), id]),
    /** 缩放：指针在画布上时以指针为中心，不然以画布中心。 */
    zoomBy: (factor: number) => view.set(zoomAt(view(), size(), pointerAt() ?? [size()[0] / 2, size()[1] / 2], factor)),
    selectOnly: (id: string, add = false) => selection.set(add ? toggle(selection(), id) : [id]),
  };
}

export type Store = ReturnType<typeof createStore>;
