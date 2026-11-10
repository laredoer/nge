// 编辑器的状态和操作（docs/editor.md）：放在组件外面，试玩回来时原样还在。界面只调这里的函数。
import { batch, signal } from "nsh";
import { checkTypes, emptyLevel, type Entity, type LevelFile } from "../../ts/level";
import type { CameraView } from "../../ts/nge";
import { boundsOf, newEntity, placed, sizeOf, TILESET, TYPES } from "../platformer/catalog";
import { fromFile, type Built } from "../platformer/levels";
import { copy, nextId, pasted } from "./clipboard";
import { addEntities, moveTo, removeEntities, reorder, resizeLayer, setLevel, updateEntity } from "./commands";
import { fieldPatch, parseSize } from "./fields";
import { fileName, openFrom, saveTo, type LevelStore } from "./files";
import { execute, isDirty, markSaved, newEditor, redo, undo, type Command } from "./history";
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

  const run = (cmd: Command, select?: string[]) => {
    batch(() => {
      editor.set(execute(editor(), cmd, now()));
      selection.set(prune(select ?? selection(), level()));
    });
  };
  const travel = (fn: typeof undo) => {
    batch(() => {
      editor.set(fn(editor()));
      selection.set(prune(selection(), level()));
    });
  };

  const remove = () => {
    if (selection().length > 0) run(removeEntities(level(), selection()), []);
  };
  const paste = (clip: Entity[]) => {
    if (clip.length === 0) return;
    const added = pasted(level(), clip, [cell(), 0]);
    clipboard = added;
    run(addEntities(added), added.map((e) => e.id));
  };
  /** 微移：选中的挪 (dx, dy)；没选中时平移视图。连着按的合成一步撤销。 */
  const nudge = (dx: number, dy: number) => {
    if (selection().length === 0) return view.set({ ...view(), x: view().x + dx * 4, y: view().y + dy * 4 });
    const targets = level().entities.filter((e) => selection().includes(e.id)).map((e) => {
      const p = placed(e, e.x + dx, e.y + dy);
      return { id: e.id, x: p.x, y: p.y };
    });
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
    if (field === "name") return level().name === text ? null : (run(setLevel(level(), { name: text })), null);
    if (field === "night") return (run(setLevel(level(), { night: text === "true" })), null);
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
  const refreshFiles = async () => {
    try {
      files.set(await (await deps.files()).list());
    } catch (e) {
      files.set([]);
      status.set(`列不出关卡文件：${e instanceof Error ? e.message : String(e)}`);
    }
  };
  const open = async (name: string) => {
    if (!(await guard())) return;
    const r = await openFrom(await deps.files(), name);
    if (!r.ok) return status.set(`打不开 ${name}：${r.errors.slice(0, 3).join("；")}`);
    reset(r.level, name);
    const problems = checkTypes(r.level, TYPES);
    status.set(problems.length === 0 ? `打开了 ${name}` : `打开了 ${name}（${problems[0]}）`);
  };
  const write = async (name: string): Promise<boolean> => {
    try {
      await saveTo(await deps.files(), name, level());
    } catch (e) {
      status.set(`保存失败：${e instanceof Error ? e.message : String(e)}`);
      return false;
    }
    batch(() => {
      editor.set(markSaved(editor()));
      file.set(name);
      panel.set(null);
      const problems = checkTypes(level(), TYPES);
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
    await refreshFiles();
    if (name !== file() && files().includes(name)) {
      const answer = await deps.ask(`${name} 已经有了`, "要覆盖它吗？", [{ id: "overwrite", label: "覆盖", role: "destructive" }, { id: "cancel", label: "取消", role: "cancel" }]);
      if (answer !== "overwrite") return false;
    }
    return write(name);
  };

  /** 画布上的指针：`shift` 按着点击是加选 / 减选。 */
  const ctx = (): Ctx => ({
    level: level(), selection: selection(), view: view(), size: size(), grid: grid(), tool: tool(), placeType: placeType(),
    frame: frame(), layer: 0, boundsOf, sizeOf, place: placed, make: newEntity,
  });
  const apply = (out: Out) => {
    batch(() => {
      drag.set(out.drag);
      if (out.view !== undefined) view.set(out.view);
      if (out.command !== undefined) run(out.command, out.selection);
      else if (out.selection !== undefined) selection.set(out.selection);
    });
  };
  const pointer = (phase: "down" | "move" | "up" | "cancel", p: [number, number], shift = false) => {
    const c = ctx();
    const before = drag();
    if (phase === "cancel") return drag.set(IDLE);
    if (phase === "down") {
      const out = pointerDown(c, p);
      return apply(shift ? { drag: out.drag } : out);
    }
    if (before.kind === "idle") return;
    if (phase === "move") return apply(pointerMove(c, before, p));
    const out = pointerUp(c, before, p);
    // 按着 shift 点一下：点中的加进来或去掉，点空白不变
    if (shift && c.tool === "select" && before.kind === "press" && out.command === undefined && out.drag.kind === "idle" && out.selection !== undefined) {
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
    duplicate: () => paste(copy(level(), selection())),
    copy: () => {
      clipboard = copy(level(), selection());
      status.set(`复制了 ${clipboard.length} 个`);
    },
    paste: () => paste(clipboard),
    selectAll: () => selection.set(level().entities.map((e) => e.id)),
    nudge,
    reorderBy: (id: string, delta: number) => {
      const i = level().entities.findIndex((e) => e.id === id);
      if (i >= 0) run(reorder(level(), id, i + delta));
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
