// 关卡编辑器（docs/editor.md、docs/editor-more.md）：左边场景树、层和物件库，中间游戏视图，右边属性；⌘R 试玩，Esc 回来。
// 试玩时只换掉中间的画布，两边的面板还在，改了关卡正在玩的这一关就地更新。
import { cleanup, computed, createAnimatedValue, effect, keymap, mount, signal, Text, View, type GestureResponderEvent } from "nsh";
import { AlertDialog } from "nsh/base";
import { Window } from "nsh/desktop";
import { gameView, touchGestures, type Gesture } from "../../ts/nge";
import { Game } from "../platformer/game";
import { pickStore, type Picked } from "./fsstore";
import { FilePanel, Inspector, LayerPanel, Library, SceneTree, Toolbar } from "./panels";
import { editorScene } from "./scene";
import { createStore, type Store } from "./store";
import { trackpadZoom } from "./trackpad";
import { pinchView } from "./view";

const GameView = gameView();

let picked: Promise<Picked> | null = null;
const store = createStore({
  // 第一次存取文件时才决定用目录还是应用存储
  files: async () => {
    if (picked === null) {
      picked = pickStore();
      picked.then((p) => store.status.set(`关卡存在${p.where}`));
    }
    return (await picked).store;
  },
  ask: async (title, description, actions) => (await AlertDialog.show({ title, description, actions })).action ?? null,
});

/** 编辑时的快捷键：输入框有焦点时不响应，打字不受影响。 */
function bindKeys(s: Store) {
  const step = () => s.cell();
  keymap.bind(
    {
      delete: s.remove,
      backspace: s.remove,
      "mod+z": s.undo,
      "mod+shift+z": s.redo,
      "mod+y": s.redo,
      "mod+d": s.duplicate,
      "mod+c": s.copy,
      "mod+v": s.paste,
      "mod+a": s.selectAll,
      "mod+s": () => void s.save(),
      "mod+shift+s": () => void s.refreshFiles().then(() => s.panel.set("saveAs")),
      "mod+o": () => void s.refreshFiles().then(() => s.panel.set("open")),
      "mod+n": () => void s.newLevel(),
      "mod+r": s.play,
      f5: s.play,
      "mod+plus": () => s.zoomBy(1.25),
      "mod+minus": () => s.zoomBy(1 / 1.25),
      v: () => s.tool.set("select"),
      h: () => s.tool.set("pan"),
      p: () => s.tool.set("place"),
      b: () => s.tool.set("brush"),
      e: () => s.tool.set("erase"),
      r: () => s.tool.set("rect"),
      g: () => s.tool.set("fill"),
    },
    { when: "notEditing" },
  );
  // Esc 在文件名输入框里也要能关掉文件面板
  keymap.bind("escape", () => (s.panel() !== null ? s.panel.set(null) : s.selection.set([])), { when: "always" });
  // 方向键微移一点，加 shift 挪一格；没选中时平移视图
  keymap.bind(
    {
      left: () => s.nudge(-1, 0),
      right: () => s.nudge(1, 0),
      up: () => s.nudge(0, 1),
      down: () => s.nudge(0, -1),
      "shift+left": () => s.nudge(-step(), 0),
      "shift+right": () => s.nudge(step(), 0),
      "shift+up": () => s.nudge(0, step()),
      "shift+down": () => s.nudge(0, -step()),
    },
    { when: "notEditing", repeat: true },
  );
}

/** 自动滚动多久看一次（毫秒）。 */
const AUTO_SCROLL_MS = 33;

/** 画布：编辑时的游戏视图，指针、触控板、悬停、自动滚动、快捷键。 */
function Canvas(p: { store: Store; shift: { (): boolean; set(v: boolean): void } }) {
  const s = p.store;
  const shift = p.shift;
  bindKeys(s);
  // 拖到画布边上时自动滚动：没在拖的时候 autoScroll 什么都不做
  const scrolling = setInterval(s.autoScroll, AUTO_SCROLL_MS);
  cleanup(() => clearInterval(scrolling));
  const at = (e: { locationX: number; locationY: number }): [number, number] => [e.locationX, e.locationY];
  const scale = createAnimatedValue(1);
  const trackpad = trackpadZoom(s, scale);
  cleanup(trackpad.stop);
  // 触屏：第二根手指按下时取消正在进行的工具操作，双指捏合缩放、平移（docs/touch-input.md）
  const touch = touchGestures((g: Gesture) => {
    if (g.kind === "pinchStart") s.pointer("cancel", [g.x, g.y]);
    if (g.kind === "pinch") s.view.set(pinchView(s.view(), s.size(), g));
  });
  const single = (e: GestureResponderEvent) => e.touches.length < 2;
  return (
    <View
      class="flex-1"
      onMouseMove={(e) => {
        shift.set(e.modifiers.shift);
        trackpad.mouse = [e.position.x, e.position.y];
        // 按着的时候（拖动）不改悬停，位置由触摸事件记
        if (e.pressedButton === null) s.hoverAt([e.position.x, e.position.y]);
      }}
      onMouseLeave={() => s.hoverAt(null)}
      pinch={{ scale }}
      onPinchStart={trackpad.start}
      onPinchEnd={trackpad.end}
    >
      <GameView
        class="flex-1"
        scene={editorScene(s.sceneInput())}
        captureKeys={[]}
        onLayout={(e) => s.size.set([e.bounds.size.width, e.bounds.size.height])}
        onTouchStart={(e) => {
          touch.onTouchStart(e);
          if (single(e)) s.pointer("down", at(e), shift());
        }}
        onTouchMove={(e) => {
          touch.onTouchMove(e);
          if (single(e)) s.pointer("move", at(e), shift());
        }}
        onTouchEnd={(e) => {
          touch.onTouchEnd(e);
          s.pointer("up", at(e), shift());
        }}
        onTouchCancel={(e) => {
          touch.onTouchCancel(e);
          s.pointer("cancel", at(e));
        }}
      />
    </View>
  );
}

/** 试玩时的快捷键：撤销、重做、保存照常（别的键给游戏）。 */
function TrialKeys(p: { store: Store }) {
  const s = p.store;
  keymap.bind({ "mod+z": s.undo, "mod+shift+z": s.redo, "mod+y": s.redo, "mod+s": () => void s.save() }, { when: "notEditing" });
  return null;
}


export function App() {
  const s = store;
  effect(() => Window.setTitle(`${s.file() ?? "未命名"}${s.dirty() ? " ●" : ""} — 关卡编辑器`));
  /** 鼠标在画布上时 shift 按没按着（点选时加选）。 */
  const shift = signal(false);
  // 条件只读布尔（docs/touch-input.md）：试玩里关卡每变一次 playing 都是新对象，不能让游戏视图跟着重建
  const playing = computed(() => s.playing() !== null);
  /** 热重载：游戏每次取值是编辑器里现在的（回到编辑的那一刻 playing 已经是 null，用最后一次的）。 */
  let last = s.playing();
  const trial = () => (last = s.playing() ?? last)!;
  return (
    <View class="flex-1 bg-neutral-900">
      <Toolbar store={s} />
      <View class="flex-1 flex-row">
        <View class="w-56 bg-neutral-800">
          <SceneTree store={s} shift={shift} />
          <LayerPanel store={s} />
          <Library store={s} />
        </View>
        {playing() ? (
          <View class="flex-1">
            <TrialKeys store={s} />
            <Game trial={trial} onExit={s.stop} />
          </View>
        ) : (
          <Canvas store={s} shift={shift} />
        )}
        <Inspector store={s} />
      </View>
      <View class="flex-row px-2 py-1 bg-neutral-950">
        <Text testID="status" class="text-neutral-300 text-xs flex-1">{s.status()}</Text>
        <Text testID="file" class="text-neutral-400 text-xs">{`${s.file() ?? "未命名"}${s.dirty() ? " ●" : ""}`}</Text>
      </View>
      {s.panel() !== null ? <FilePanel store={s} /> : null}
    </View>
  );
}

mount(App);
