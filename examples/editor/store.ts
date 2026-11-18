// 编辑器的状态和操作（docs/editor.md）：放在组件外面，试玩回来时原样还在。界面只调这里的函数。
import { batch, signal } from "nsh";
import { checkTypes, emptyLevel, formatLevel, parseLevel, type Entity, type LevelFile } from "../../ts/level";
import type { CameraView } from "../../ts/nge";
import { boundsOf, labelOf, newEntity, placed, sizeOf, TILESET, TYPES } from "../platformer/catalog";
import { depthOf } from "../platformer/preview";
import { fromFile, type Built } from "../platformer/levels";
import { copy, nextId, pasted } from "./clipboard";
import { addEntities, moveTo, removeEntities, reorder, resizeLayer, setLevel, updateEntity } from "./commands";
import { fieldPatch, nameError, parseSize } from "./fields";
export { EDIT_MAX_COLS, EDIT_MAX_ROWS } from "./fields";
import { fileName, openFrom, saveTo, type LevelStore } from "./files";
import { upgradeTiles } from "./tiles";
import { execute, isDirty, markSavedAt, newEditor, redo, savePoint, undo, type Command } from "./history";
import type { SceneInput } from "./scene";
import { prune, toggle } from "./select";
import { snapPoint } from "./snap";
import { IDLE, pointerDown, pointerMove, pointerUp, previewOf, type Ctx, type Drag, type Out, type ToolName } from "./tool";
import { zoomAt } from "./view";

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

/** 出生点、终点要在地图里（第一层瓦片的范围），不然一开始就掉下去或者到不了。 */
export function outsideMap(level: LevelFile): string[] {
  const t = level.tiles[0];
  if (t === undefined) return [];
  const [left, right, top, bottom] = [t.x, t.x + t.cols * t.tile, t.y, t.y - t.rows * t.tile];
  return level.entities
    .filter((e) => (e.type === "spawn" || e.type === "goal") && (e.x < left || e.x > right || e.y < bottom || e.y > top))
    .map((e) => `${labelOf({ ...e, name: undefined })}在地图外面`);
}

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
  let clipboard: Entity[] = [];

  const level = () => editor().level;
  const dirty = () => isDirty(editor());
  const cell = () => (grid() > 0 ? grid() : TILESET.tile);
  const entity = (id: string) => level().entities.find((e) => e.id === id);

  // 关卡变了，正在拖的就作废（拖着的东西可能没了，一笔的旧值也不对了）
  const run = (cmd: Command, select?: string[]) => {
    batch(() => {
      drag.set(IDLE);
      editor.set(execute(editor(), cmd, now()));
      selection.set(prune(select ?? selection(), level()));
    });
  };
  const travel = (fn: typeof undo) => {
    batch(() => {
      drag.set(IDLE);
      editor.set(fn(editor()));
      selection.set(prune(selection(), level()));
    });
  };

  const remove = () => {
    if (selection().length > 0) run(removeEntities(level(), selection()), []);
  };
  /** 贴进来：`remember` 时剪贴板换成贴进来的（接着粘贴再错开一格），⌘D 不动剪贴板。 */
  const paste = (clip: Entity[], remember: boolean) => {
    if (clip.length === 0) return;
    const added = pasted(level(), clip, [cell(), 0]);
    if (remember) clipboard = added;
    run(addEntities(added), added.map((e) => e.id));
  };
  /** 微移：选中的挪 (dx, dy)；没选中时平移视图。连着按的合成一步撤销。 */
  const nudge = (dx: number, dy: number) => {
    if (selection().length === 0) return view.set({ ...view(), x: view().x + dx * 4, y: view().y + dy * 4 });
    const chosen = new Set(selection());
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
  /** 属性面板提交一个输入框：合规就改，不然返回原因。 */
  const setField = (id: string, field: string, text: string): string | null => {
    const e = entity(id);
    if (e === undefined) return "选中的东西不在了";
    const r = fieldPatch(e, field, text);
    if (!r.ok) return r.error;
    const changed = Object.entries(r.value).some(([k, v]) => JSON.stringify(v) !== JSON.stringify(e[k as keyof Entity]));
    if (changed) run(updateEntity(level(), id, r.value));
    return null;
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
    const t = level().tiles[0];
    if (t === undefined) return "没有瓦片层";
    const r = parseSize(text, field === "rows");
    if (!r.ok) return r.error;
    const [cols, rows] = field === "cols" ? [r.value, t.rows] : [t.cols, r.value];
    if (cols !== t.cols || rows !== t.rows) run(resizeLayer(level(), 0, cols, rows));
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
      selection.set([]);
      file.set(name);
      view.set(START_VIEW);
      panel.set(null);
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
    frame: frame(), layer: 0, boundsOf, sizeOf, place: placed, make: newEntity, depthOf,
  });
  const apply = (out: Out) => {
    batch(() => {
      drag.set(out.drag);
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
  /** 画布上要画的：拖动中的预览、框选框。 */
  const sceneInput = (): SceneInput => {
    const d = drag();
    return { level: previewOf(ctx(), d), selection: selection(), view: view(), size: size(), grid: grid(), box: d.kind === "box" ? [d.start, d.end] : null };
  };

  /** 试玩：先查关卡合不合规，不合规的不进。 */
  const play = () => {
    const r = fromFile(level());
    if (!r.ok) return status.set(`还不能试玩：${r.errors.slice(0, 3).join("；")}`);
    const outside = outsideMap(level());
    if (outside.length > 0) return status.set(`还不能试玩：${outside.join("；")}`);
    batch(() => {
      drag.set(IDLE);
      playing.set({ name: r.name || "试玩", built: r.built, night: r.night });
      status.set("试玩中，按 Esc 回到编辑");
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
    level, dirty, cell, entity, run,
    undo: () => travel(undo),
    redo: () => travel(redo),
    remove,
    duplicate: () => paste(copy(level(), selection()), false),
    copy: () => {
      clipboard = copy(level(), selection());
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
    pointer, sceneInput,
    zoomBy: (factor: number) => view.set(zoomAt(view(), size(), [size()[0] / 2, size()[1] / 2], factor)),
    selectOnly: (id: string, add = false) => selection.set(add ? toggle(selection(), id) : [id]),
  };
}

export type Store = ReturnType<typeof createStore>;
