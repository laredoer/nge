// 关卡编辑器（docs/editor.md）：左边场景树和物件库，中间游戏视图，右边属性；⌘R 试玩，Esc 回来。
import { effect, keymap, mount, signal, Text, View } from "nsh";
import { AlertDialog } from "nsh/base";
import { Window } from "nsh/desktop";
import { gameView } from "../../ts/nge";
import { Game } from "../platformer/game";
import { pickStore, type Picked } from "./fsstore";
import { FilePanel, Inspector, Library, SceneTree, Toolbar } from "./panels";
import { editorScene } from "./scene";
import { createStore, type Store } from "./store";

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
      escape: () => (s.panel() !== null ? s.panel.set(null) : s.selection.set([])),
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
  return (
    <View class="flex-1 flex-row">
      <View class="w-56 bg-neutral-800">
        <SceneTree store={s} shift={shift} />
        <Library store={s} />
      </View>
      <View class="flex-1" onMouseMove={(e) => shift.set(e.modifiers.shift)}>
        <GameView
          class="flex-1"
          scene={editorScene(s.sceneInput())}
          captureKeys={[]}
          onLayout={(e) => s.size.set([e.bounds.size.width, e.bounds.size.height])}
          onTouchStart={(e) => s.pointer("down", at(e), shift())}
          onTouchMove={(e) => s.pointer("move", at(e), shift())}
          onTouchEnd={(e) => s.pointer("up", at(e), shift())}
          onTouchCancel={(e) => s.pointer("cancel", at(e))}
        />
      </View>
      <Inspector store={s} />
    </View>
  );
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
