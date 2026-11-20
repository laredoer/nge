// 编辑器的面板（docs/editor.md、docs/editor-more.md）：工具栏、场景树、层、物件库和瓦片、属性、文件。都只调 store 的操作。
import { computed, createAnimatedValue, effect, signal, Scroll, Text, untracked, View } from "nsh";
import { Button, Input } from "nsh/base";
import { labelOf, propOf, TILESET, TYPE_ORDER, TYPES } from "../platformer/catalog";
import { dropTarget, treeRows } from "./hierarchy";
import { frameRule } from "./layers";
import { overrideList } from "./overrides";
import type { Store } from "./store";
import { paletteOf } from "./tiles";
import type { ToolName } from "./tool";

const TOOLS: { name: ToolName; label: string; key: string }[] = [
  { name: "select", label: "选择", key: "V" },
  { name: "pan", label: "平移", key: "H" },
  { name: "place", label: "放置", key: "P" },
  { name: "brush", label: "瓦片刷", key: "B" },
  { name: "erase", label: "擦除", key: "E" },
  { name: "rect", label: "矩形", key: "R" },
  { name: "fill", label: "油漆桶", key: "G" },
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
      {s.playing() === null ? <Btn id="play" label="试玩 ⌘R" class="bg-emerald-700" onPress={s.play} /> : <Btn id="stop" label="回到编辑 Esc" class="bg-emerald-700" onPress={s.stop} />}
    </View>
  );
}

/** 场景树的一行有多高（拖动时按它算拖到了哪一行）。 */
const ROW_H = 24;

/**
 * 场景树：按层级缩进（父节点后面跟着子孙），▸ / ▾ 折叠；点选，shift 加选；上移下移改先后。
 * 按住一行左边的 ⠿ 拖到另一行上松开：选中的（含这一行）放进那一行；拖到最后一行下面（列表底下留了一行空白）是放回最外层。
 */
export function SceneTree(p: { store: Store; shift: () => boolean }) {
  const s = p.store;
  const rows = () => treeRows(s.level(), new Set(s.collapsed()));
  /** 拖动由 nsh 的 `pan` 认（按住、挪出一点才算拖，松开时给出挪了多少）；值不画到样式上，行本身不动。 */
  const grip = createAnimatedValue(0);
  /** 按着第 `index` 行的把手挪了 `dy` 松开：放到哪一行（从这一行的中间算）。 */
  const drop = (index: number, id: string, dy: number) => {
    grip.set(0);
    const t = dropTarget(rows(), index, index * ROW_H + ROW_H / 2 + dy, ROW_H);
    if (t !== undefined) s.dropOn(id, t);
  };
  return (
    <View class="flex-1">
      <Text class="text-neutral-400 text-xs px-2 pt-2">{`场景（${s.level().entities.length}）`}</Text>
      <Scroll class="flex-1">
        {rows().map((row, index) => {
          const e = s.entity(row.id)!;
          return (
            <View class="flex-row items-center" style={{ height: ROW_H, paddingLeft: 4 + row.depth * 12 }}>
              <View testID={`grip-${row.id}`} class="w-4 items-center" pan={{ y: grip }} onPanEnd={(ev) => (ev.cancelled ? grip.set(0) : drop(index, row.id, ev.translation.y))}>
                <Text class="text-neutral-500 text-xs">⠿</Text>
              </View>
              {row.parent ? (
                <Button testID={`fold-${row.id}`} class="w-4" onPress={() => s.toggleCollapse(row.id)}>
                  <Text class="text-neutral-400 text-xs">{row.collapsed ? "▸" : "▾"}</Text>
                </Button>
              ) : (
                <View class="w-4" />
              )}
              <Button
                testID={`tree-${row.id}`}
                class="flex-1 px-1 hover:bg-neutral-700 selected:bg-sky-800"
                selected={s.selection().includes(row.id)}
                onPress={() => s.selectOnly(row.id, p.shift())}
              >
                <Text class="text-white text-sm">{`${labelOf(e)}${e.name ? `「${e.name}」` : ""} · ${row.id}`}</Text>
              </Button>
            </View>
          );
        })}
        <View style={{ height: ROW_H }} />
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

/** 层：每层一行（名字、显隐、锁定），点一下设为当前层；加、删、上移、下移。 */
export function LayerPanel(p: { store: Store }) {
  const s = p.store;
  return (
    <View class="p-2 gap-1 border-t border-neutral-700">
      <Text class="text-neutral-400 text-xs">层</Text>
      {s.level().tiles.map((t, i) => (
        <View class="flex-row items-center gap-1">
          <Button testID={`layer-${i}`} class="flex-1 px-1 rounded hover:bg-neutral-700 selected:bg-sky-800" selected={s.layerIndex() === i} onPress={() => s.selectLayer(i)}>
            <Text class="text-white text-sm">{`${t.name ?? t.id}${t.parallax === undefined ? "" : " · 视差"}`}</Text>
          </Button>
          <Btn id={`layer-${i}-hidden`} label={t.hidden ? "隐" : "显"} selected={t.hidden === true} onPress={() => s.toggleLayer(i, "hidden")} />
          <Btn id={`layer-${i}-locked`} label={t.locked ? "锁" : "开"} selected={t.locked === true} onPress={() => s.toggleLayer(i, "locked")} />
        </View>
      ))}
      <View class="flex-row gap-1">
        <Btn id="layer-add" label="加一层" onPress={s.addLayer} />
        <Btn id="layer-remove" label="删除" disabled={s.level().tiles.length <= 1} onPress={s.removeLayer} />
        <Btn id="layer-up" label="上移" disabled={s.layerIndex() >= s.level().tiles.length - 1} onPress={() => s.moveLayerBy(1)} />
        <Btn id="layer-down" label="下移" disabled={s.layerIndex() === 0} onPress={() => s.moveLayerBy(-1)} />
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
        {paletteOf(s.level().tiles[s.layerIndex()]).map((f) => (
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
      {spec()?.rotatable ? <Field id="field-rotation" label="旋转（度）" value={() => String(e()?.rotation ?? 0)} commit={(t) => s.setField(p.id, "rotation", t)} /> : null}
      {spec()?.layered ? <Field id="field-z" label="z（空的按类型）" value={() => (e()?.z === undefined ? "" : String(e()!.z))} commit={(t) => s.setField(p.id, "z", t)} /> : null}
      <Field id="field-parent" label="父节点" value={() => e()?.parent ?? ""} commit={(t) => s.setField(p.id, "parent", t)} />
      {(spec()?.props ?? []).map((prop) => {
        const { key, label } = prop;
        const value = () => (e() === undefined ? "" : String(propOf(e()!, key)));
        if (prop.kind === "bool") return <Choice id={`prop-${key}`} label={label} options={YES_NO} value={value} set={(v) => s.setProp(p.id, key, v)} />;
        if (prop.kind === "choice") return <Choice id={`prop-${key}`} label={label} options={prop.options.map((o) => ({ value: o, label: o }))} value={value} set={(v) => s.setProp(p.id, key, v)} />;
        return <Field id={`prop-${key}`} label={label} value={value} commit={(t) => s.setProp(p.id, key, t)} />;
      })}
      {spec()?.route ? <RoutePanel store={s} id={p.id} /> : null}
      {e()?.type === "prefab" ? <OverridePanel store={s} id={p.id} /> : null}
    </View>
  );
}

/** 走地怪的路线点：一个个列出来（画布上按住能拖），加一个、删一个。 */
function RoutePanel(p: { store: Store; id: string }) {
  const s = p.store;
  const points = () => s.entity(p.id)?.points ?? [];
  return (
    <View class="gap-1">
      <Text class="text-neutral-400 text-xs">{points().length === 0 ? "路线：往右巡逻（加点后按点来回走）" : `路线点（${points().length}，画布上能拖）`}</Text>
      {points().map(([x, y], i) => (
        <View class="flex-row items-center gap-1">
          <Text class="text-white text-xs flex-1">{`${i + 1}. (${x}, ${y})`}</Text>
          <Btn id={`point-${i}-remove`} label="删" onPress={() => s.removeRoutePoint(p.id, i)} />
        </View>
      ))}
      <Btn id="point-add" label="加路线点" onPress={() => s.addRoutePoint(p.id)} />
    </View>
  );
}

/** 预制体实例的覆盖：列出来能删；填子节点、字段、值加一条（同一项再加是换掉）。 */
function OverridePanel(p: { store: Store; id: string }) {
  const s = p.store;
  const path = signal("");
  const field = signal("");
  const value = signal("");
  const error = signal<string | null>(null);
  const add = () => error.set(s.addOverride(p.id, path(), field(), value()));
  return (
    <View class="gap-1">
      <Text class="text-neutral-400 text-xs">覆盖（子节点 / 字段 = 值）</Text>
      {overrideList(s.entity(p.id)?.set).map((o) => (
        <View class="flex-row items-center gap-1">
          <Text class="text-white text-xs flex-1">{`${o.path} / ${o.field} = ${String(o.value)}`}</Text>
          <Btn id={`override-${o.path}-${o.field}-remove`} label="删" onPress={() => s.removeOverride(p.id, o.path, o.field)} />
        </View>
      ))}
      <View class="flex-row gap-1">
        <Input testID="override-path" class="flex-1 px-1 rounded bg-neutral-900 text-white text-xs" placeholder="b/crown" value={path()} onChange={(e) => path.set(e.value)} />
        <Input testID="override-field" class="w-14 px-1 rounded bg-neutral-900 text-white text-xs" placeholder="y" value={field()} onChange={(e) => field.set(e.value)} />
        <Input testID="override-value" class="w-14 px-1 rounded bg-neutral-900 text-white text-xs" placeholder="值" value={value()} onChange={(e) => value.set(e.value)} onSubmit={add} />
      </View>
      <Btn id="override-add" label="加覆盖" onPress={add} />
      {error() === null ? null : <Text testID="override-error" class="text-red-400 text-xs">{error()!}</Text>}
    </View>
  );
}

const KINDS = [
  { value: "solid", label: "实心" },
  { value: "oneWay", label: "单向" },
  { value: "slope", label: "斜坡" },
  { value: "none", label: "不挡" },
];

/** 当前层：名字、视差、z；当前瓦片在这一层上的规则。 */
function LayerSettings(p: { store: Store }) {
  const s = p.store;
  const t = () => s.level().tiles[s.layerIndex()];
  const rule = () => (t() === undefined ? null : frameRule(t()!, s.frame()));
  const isSlope = computed(() => rule()?.kind === "slope");
  const label = () => TILESET.frames.find((f) => f.frame === s.frame())?.label ?? `第 ${s.frame()} 帧`;
  return (
    <View class="gap-2">
      <Text class="text-white">{`当前层 · ${t()?.id ?? ""}`}</Text>
      <Field id="layer-name" label="名字" value={() => t()?.name ?? ""} commit={(v) => s.setLayerField("name", v)} />
      <Field id="layer-px" label="视差 x" value={() => String(t()?.parallax?.[0] ?? 1)} commit={(v) => s.setLayerField("px", v)} />
      <Field id="layer-py" label="视差 y" value={() => String(t()?.parallax?.[1] ?? 1)} commit={(v) => s.setLayerField("py", v)} />
      <Field id="layer-z" label="z（空的按顺序）" value={() => (t()?.z === undefined ? "" : String(t()!.z))} commit={(v) => s.setLayerField("z", v)} />
      <Text class="text-neutral-500 text-xs">有视差的层游戏里只是画面，不挡人。</Text>
      <Text class="text-white">{`瓦片「${label()}」在这一层`}</Text>
      <Choice id="rule-kind" label="碰撞" options={KINDS} value={() => rule()?.kind ?? "none"} set={(v) => {
          const why = s.setFrameRule(s.frame(), "kind", v);
          if (why !== null) s.status.set(why);
        }} />
      {isSlope() ? <Field id="rule-left" label="左边高" value={() => String(rule()?.slope?.left ?? "")} commit={(v) => s.setFrameRule(s.frame(), "left", v)} /> : null}
      {isSlope() ? <Field id="rule-right" label="右边高" value={() => String(rule()?.slope?.right ?? "")} commit={(v) => s.setFrameRule(s.frame(), "right", v)} /> : null}
      <Field id="rule-frames" label="动画帧" value={() => rule()?.animation?.frames.join(", ") ?? ""} commit={(v) => s.setFrameRule(s.frame(), "frames", v)} />
      <Field id="rule-fps" label="每秒几帧" value={() => String(rule()?.animation?.fps ?? "")} commit={(v) => s.setFrameRule(s.frame(), "fps", v)} />
      <Field id="rule-friction" label="摩擦" value={() => String(rule()?.friction ?? "")} commit={(v) => s.setFrameRule(s.frame(), "friction", v)} />
      <Field id="rule-restitution" label="弹性" value={() => String(rule()?.restitution ?? "")} commit={(v) => s.setFrameRule(s.frame(), "restitution", v)} />
    </View>
  );
}

function LevelPanel(p: { store: Store }) {
  const s = p.store;
  const layer = () => s.level().tiles[s.layerIndex()];
  return (
    <View class="gap-2">
      <Text class="text-white">关卡</Text>
      <Field id="level-name" label="名字" value={() => s.level().name} commit={(t) => s.setLevelField("name", t)} />
      <Choice id="level-night" label="夜里" options={YES_NO} value={() => String(s.level().night)} set={(v) => s.setLevelField("night", v)} />
      <Field id="level-cols" label="宽（格）" value={() => String(layer()?.cols ?? "")} commit={(t) => s.setLevelField("cols", t)} />
      <Field id="level-rows" label="高（格）" value={() => String(layer()?.rows ?? "")} commit={(t) => s.setLevelField("rows", t)} />
      <Text class="text-neutral-500 text-xs">宽高是当前层的；改高度时地面跟着底边走。</Text>
      <LayerSettings store={s} />
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
