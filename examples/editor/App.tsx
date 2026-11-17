// 关卡编辑器（docs/editor.md）：左边场景树和物件库，中间游戏视图，右边属性；⌘R 试玩，Esc 回来。
import { createAnimatedValue, effect, keymap, mount, signal, Text, View, type GestureResponderEvent } from "nsh";
import { AlertDialog } from "nsh/base";
import { Window } from "nsh/desktop";
import { gameView, gestureHandlers, type Gesture } from "../../ts/nge";
import { Game } from "../platformer/game";
import { pickStore, type Picked } from "./fsstore";
import { FilePanel, Inspector, Library, SceneTree, Toolbar } from "./panels";
import { editorScene } from "./scene";
import { createStore, type Store } from "./store";
import { pinchView, zoomAt } from "./view";

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

function Editing(p: { store: Store }) {
  const s = p.store;
  bindKeys(s);
  /** 鼠标在画布上时 shift 按没按着（点选时加选）。 */
  const shift = signal(false);
  const at = (e: { locationX: number; locationY: number }): [number, number] => [e.locationX, e.locationY];
  const trackpad = trackpadZoom(s);
  // 触屏：第二根手指按下时取消正在进行的工具操作，双指捏合缩放、平移（docs/touch-input.md）
  const touch = gestureHandlers((g: Gesture) => {
    if (g.kind === "pinchStart") s.pointer("cancel", [g.x, g.y]);
    if (g.kind === "pinch") s.view.set(pinchView(s.view(), s.size(), g));
  });
  const single = (e: GestureResponderEvent) => e.touches.length < 2;
  return (
    <View class="flex-1 flex-row">
      <View class="w-56 bg-neutral-800">
        <SceneTree store={s} shift={shift} />
        <Library store={s} />
      </View>
      <View
        class="flex-1"
        onMouseMove={(e) => {
          shift.set(e.modifiers.shift);
          trackpad.mouse = [e.position.x, e.position.y];
        }}
        pinch={{ scale: trackpad.scale }}
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
      <Inspector store={s} />
    </View>
  );
}

/**
 * Mac 触控板的捏合（和 Control + 滚轮）：nsh 只在开始、结束时告诉 JS，中间的倍数在 `pinch` 的动画值里，
 * 捏的时候每 33 ms 读一次，以鼠标最后在画布上的位置为中心缩放（docs/touch-input.md）。
 */
function trackpadZoom(s: Store) {
  const scale = createAnimatedValue(1);
  let base = s.view();
  let center: [number, number] = [0, 0];
  let timer: number | undefined;
  const apply = (v: number) => s.view.set(zoomAt(base, s.size(), center, v));
  const t = {
    scale,
    /** 鼠标最后在画布上的位置；没进过画布时用中心。 */
    mouse: null as [number, number] | null,
    start() {
      base = s.view();
      center = t.mouse ?? [s.size()[0] / 2, s.size()[1] / 2];
      clearInterval(timer);
      timer = setInterval(() => void scale.getValue().then((v) => timer !== undefined && apply(v)), 33);
    },
    end(e: { scale: number }) {
      clearInterval(timer);
      timer = undefined;
      apply(e.scale);
      scale.set(1);
    },
  };
  return t;
}

export function App() {
  const s = store;
  effect(() => Window.setTitle(`${s.file() ?? "未命名"}${s.dirty() ? " ●" : ""} — 关卡编辑器`));
  return (
    <View class="flex-1 bg-neutral-900">
      {s.playing() === null ? <Toolbar store={s} /> : null}
      {s.playing() === null ? <Editing store={s} /> : <Game trial={s.playing()!} onExit={s.stop} />}
      <View class="flex-row px-2 py-1 bg-neutral-950">
        <Text testID="status" class="text-neutral-300 text-xs flex-1">{s.status()}</Text>
        <Text testID="file" class="text-neutral-400 text-xs">{`${s.file() ?? "未命名"}${s.dirty() ? " ●" : ""}`}</Text>
      </View>
      {s.panel() !== null && s.playing() === null ? <FilePanel store={s} /> : null}
    </View>
  );
}

mount(App);
