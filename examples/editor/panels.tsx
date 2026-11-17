// 编辑器的面板（docs/editor.md）：工具栏、场景树、物件库和瓦片、属性、文件。都只调 store 的操作。
import { computed, effect, signal, Scroll, Text, untracked, View } from "nsh";
import { Button, Input } from "nsh/base";
import { labelOf, propOf, TILESET, TYPE_ORDER, TYPES } from "../platformer/catalog";
import type { Store } from "./store";
import type { ToolName } from "./tool";

const TOOLS: { name: ToolName; label: string; key: string }[] = [
  { name: "select", label: "选择", key: "V" },
  { name: "pan", label: "平移", key: "H" },
  { name: "place", label: "放置", key: "P" },
  { name: "brush", label: "瓦片刷", key: "B" },
  { name: "erase", label: "擦除", key: "E" },
];
const GRIDS = [32, 16, 0];

function Btn(p: { id: string; label: string; onPress: () => void; selected?: boolean; disabled?: boolean; class?: string }) {
  return (
    <Button
      testID={p.id}
      class={`px-2 py-1 rounded bg-neutral-700 hover:bg-neutral-600 active:bg-neutral-500 selected:bg-sky-700 disabled:opacity-40 ${p.class ?? ""}`}
      selected={p.selected}
      disabled={p.disabled}
      onPress={p.onPress}
    >
      <Text class="text-white text-sm">{p.label}</Text>
    </Button>
  );
}

export function Toolbar(p: { store: Store }) {
  const s = p.store;
  return (
    <View class="flex-row flex-wrap items-center gap-1 p-2 bg-neutral-800">
      <Btn id="new" label="新建" onPress={() => void s.newLevel()} />
      <Btn id="open" label="打开" onPress={() => void s.refreshFiles().then(() => s.panel.set("open"))} />
      <Btn id="save" label="保存" onPress={() => void s.save()} />
      <Btn id="saveAs" label="另存为" onPress={() => void s.refreshFiles().then(() => s.panel.set("saveAs"))} />
      <Text class="text-neutral-500 px-1">|</Text>
      <Btn id="undo" label="撤销" disabled={s.editor().history.done.length === 0} onPress={s.undo} />
      <Btn id="redo" label="重做" disabled={s.editor().history.undone.length === 0} onPress={s.redo} />
      <Text class="text-neutral-500 px-1">|</Text>
      {TOOLS.map((t) => (
        <Btn id={`tool-${t.name}`} label={`${t.label} ${t.key}`} selected={s.tool() === t.name} onPress={() => s.tool.set(t.name)} />
      ))}
      <Text class="text-neutral-500 px-1">|</Text>
      {GRIDS.map((g) => (
        <Btn id={`grid-${g}`} label={g === 0 ? "不吸附" : `网格 ${g}`} selected={s.grid() === g} onPress={() => s.grid.set(g)} />
      ))}
      <Btn id="zoomOut" label="缩小" onPress={() => s.zoomBy(1 / 1.25)} />
      <Text class="text-white text-sm w-12 text-center">{`${Math.round(s.view().zoom * 100)}%`}</Text>
      <Btn id="zoomIn" label="放大" onPress={() => s.zoomBy(1.25)} />
      <Text class="text-neutral-500 px-1">|</Text>
      <Btn id="play" label="试玩 ⌘R" class="bg-emerald-700" onPress={s.play} />
    </View>
  );
}

/** 场景树：按画的先后（后画的在上面）列出来；点选，shift 加选；上移下移改先后。 */
export function SceneTree(p: { store: Store; shift: () => boolean }) {
  const s = p.store;
  return (
    <View class="flex-1">
      <Text class="text-neutral-400 text-xs px-2 pt-2">{`场景（${s.level().entities.length}）`}</Text>
      <Scroll class="flex-1">
        {s.level().entities.map((e) => (
          <Button
            testID={`tree-${e.id}`}
            class="px-2 py-1 hover:bg-neutral-700 selected:bg-sky-800"
            selected={s.selection().includes(e.id)}
            onPress={() => s.selectOnly(e.id, p.shift())}
          >
            <Text class="text-white text-sm">{`${labelOf(e)}${e.name ? `「${e.name}」` : ""} · ${e.id}`}</Text>
          </Button>
        ))}
      </Scroll>
      <View class="flex-row gap-1 p-2">
        <Btn id="up" label="上移" disabled={s.selection().length !== 1} onPress={() => s.reorderBy(s.selection()[0], 1)} />
        <Btn id="down" label="下移" disabled={s.selection().length !== 1} onPress={() => s.reorderBy(s.selection()[0], -1)} />
        <Btn id="duplicate" label="复制" disabled={s.selection().length === 0} onPress={s.duplicate} />
        <Btn id="delete" label="删除" disabled={s.selection().length === 0} onPress={s.remove} />
      </View>
    </View>
  );
}

/** 物件库（点一下放到视图中间，接着点画布继续放）和瓦片刷的瓦片。 */
export function Library(p: { store: Store }) {
  const s = p.store;
  return (
    <View class="p-2 gap-1 border-t border-neutral-700">
      <Text class="text-neutral-400 text-xs">物件库</Text>
      <View class="flex-row flex-wrap gap-1">
        {TYPE_ORDER.map((type) => (
          <Btn
            id={`lib-${type}`}
            label={TYPES[type].label ?? type}
            selected={s.tool() === "place" && s.placeType() === type}
            onPress={() => {
              s.addAtCenter(type);
              s.tool.set("place");
            }}
          />
        ))}
      </View>
      <Text class="text-neutral-400 text-xs pt-1">瓦片</Text>
      <View class="flex-row flex-wrap gap-1">
        {TILESET.frames.map((f) => (
          <Btn
            id={`tile-${f.frame}`}
            label={f.label}
            selected={s.tool() === "brush" && s.frame() === f.frame}
            onPress={() => {
              s.frame.set(f.frame);
              s.tool.set("brush");
            }}
          />
        ))}
        <Btn id="tile-erase" label="擦除" selected={s.tool() === "erase"} onPress={() => s.tool.set("erase")} />
      </View>
    </View>
  );
}

/** 一个输入框：回车或离开时提交，不合规的在下面说原因，值不变。 */
function Field(p: { id: string; label: string; value: () => string; commit: (text: string) => string | null; disabled?: boolean }) {
  const text = signal(untracked(p.value));
  const error = signal<string | null>(null);
  // 撤销、拖动改了值：输入框跟着变
  effect(() => {
    text.set(p.value());
    error.set(null);
  });
  const submit = () => {
    if (text() !== untracked(p.value)) error.set(p.commit(text()));
  };
  return (
    <View class="gap-0.5">
      <View class="flex-row items-center gap-2">
        <Text class="text-neutral-400 text-xs w-16">{p.label}</Text>
        <Input
          testID={p.id}
          class="flex-1 px-1 py-0.5 rounded bg-neutral-900 text-white text-sm"
          value={text()}
          readonly={p.disabled}
          onChange={(e) => text.set(e.value)}
          onSubmit={submit}
          onBlur={submit}
        />
      </View>
      {error() === null ? null : <Text testID={`${p.id}-error`} class="text-red-400 text-xs">{error()!}</Text>}
    </View>
  );
}

function Choice(p: { id: string; label: string; options: { value: string; label: string }[]; value: () => string; set: (v: string) => void }) {
  return (
    <View class="flex-row items-center gap-2">
      <Text class="text-neutral-400 text-xs w-16">{p.label}</Text>
      <View class="flex-row flex-wrap gap-1 flex-1">
        {p.options.map((o) => (
          <Btn id={`${p.id}-${o.value}`} label={o.label} selected={p.value() === o.value} onPress={() => p.set(o.value)} />
        ))}
      </View>
    </View>
  );
}

const YES_NO = [{ value: "true", label: "是" }, { value: "false", label: "否" }];

function EntityPanel(p: { store: Store; id: string }) {
  const s = p.store;
  const e = () => s.entity(p.id);
  const num = (k: "x" | "y" | "w" | "h") => () => String(e()?.[k] ?? "");
  const spec = () => TYPES[e()?.type ?? ""];
  return (
    <View class="gap-2">
      <Text class="text-white">{`${e() === undefined ? "" : labelOf(e()!)} · ${p.id}`}</Text>
      <Field id="field-name" label="名字" value={() => e()?.name ?? ""} commit={(t) => s.setField(p.id, "name", t)} />
      <Field id="field-x" label="x" value={num("x")} commit={(t) => s.setField(p.id, "x", t)} />
      <Field id="field-y" label="y" value={num("y")} commit={(t) => s.setField(p.id, "y", t)} disabled={spec()?.fixedY !== undefined} />
      {spec()?.resizable ? <Field id="field-w" label="宽" value={num("w")} commit={(t) => s.setField(p.id, "w", t)} /> : null}
      {spec()?.resizable === "wh" ? <Field id="field-h" label="高" value={num("h")} commit={(t) => s.setField(p.id, "h", t)} /> : null}
      {(spec()?.props ?? []).map((prop) => {
        const { key, label } = prop;
        const value = () => (e() === undefined ? "" : String(propOf(e()!, key)));
        if (prop.kind === "bool") return <Choice id={`prop-${key}`} label={label} options={YES_NO} value={value} set={(v) => s.setProp(p.id, key, v)} />;
        if (prop.kind === "choice") return <Choice id={`prop-${key}`} label={label} options={prop.options.map((o) => ({ value: o, label: o }))} value={value} set={(v) => s.setProp(p.id, key, v)} />;
        return <Field id={`prop-${key}`} label={label} value={value} commit={(t) => s.setProp(p.id, key, t)} />;
      })}
    </View>
  );
}

function LevelPanel(p: { store: Store }) {
  const s = p.store;
  const layer = () => s.level().tiles[0];
  return (
    <View class="gap-2">
      <Text class="text-white">关卡</Text>
      <Field id="level-name" label="名字" value={() => s.level().name} commit={(t) => s.setLevelField("name", t)} />
      <Choice id="level-night" label="夜里" options={YES_NO} value={() => String(s.level().night)} set={(v) => s.setLevelField("night", v)} />
      <Field id="level-cols" label="宽（格）" value={() => String(layer()?.cols ?? "")} commit={(t) => s.setLevelField("cols", t)} />
      <Field id="level-rows" label="高（格）" value={() => String(layer()?.rows ?? "")} commit={(t) => s.setLevelField("rows", t)} />
      <Text class="text-neutral-500 text-xs">改高度时地面跟着底边走。</Text>
    </View>
  );
}

/** 属性面板：选中一个时是它的属性，没选中时是关卡设置。 */
export function Inspector(p: { store: Store }) {
  const s = p.store;
  // 只在选中的东西换了时重建面板，改值时不重建（输入框不丢焦点）
  const single = computed(() => (s.selection().length === 1 ? s.selection()[0] : null));
  const count = () => s.selection().length;
  return (
    <Scroll class="w-64 bg-neutral-800 p-2">
      {single() !== null ? <EntityPanel store={s} id={single()!} /> : count() > 1 ? <Text class="text-white">{`选中了 ${count()} 个`}</Text> : <LevelPanel store={s} />}
    </Scroll>
  );
}

/** 打开 / 另存为：levels/ 目录里的文件列表，另存为多一个文件名输入框。 */
export function FilePanel(p: { store: Store }) {
  const s = p.store;
  const name = signal(untracked(() => (s.file() ?? "").replace(/\.json$/, "")));
  const saving = () => s.panel() === "saveAs";
  return (
    <View class="absolute inset-0 items-center justify-center bg-black/50">
      <View class="w-96 max-h-96 bg-neutral-800 rounded p-3 gap-2">
        <Text class="text-white">{saving() ? "另存为" : "打开关卡"}</Text>
        <Scroll class="max-h-60">
          {s.files().length === 0 ? <Text class="text-neutral-400 text-sm">levels/ 里还没有关卡</Text> : null}
          {s.files().map((f: string) => (
            <Button testID={`file-${f}`} class="px-2 py-1 hover:bg-neutral-700" onPress={() => (saving() ? name.set(f.replace(/\.json$/, "")) : void s.open(f))}>
              <Text class="text-white text-sm">{f}</Text>
            </Button>
          ))}
        </Scroll>
        {saving() ? (
          <View class="flex-row gap-2 items-center">
            <Input testID="file-name" class="flex-1 px-1 py-0.5 rounded bg-neutral-900 text-white" placeholder="文件名" value={name()} onChange={(e) => name.set(e.value)} onSubmit={() => void s.saveAs(name())} />
            <Btn id="file-save" label="保存" onPress={() => void s.saveAs(name())} />
          </View>
        ) : null}
        <Btn id="file-cancel" label="取消" onPress={() => s.panel.set(null)} />
      </View>
    </View>
  );
}
