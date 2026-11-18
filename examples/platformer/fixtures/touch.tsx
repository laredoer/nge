// touch-widgets.test.tsx 用的：假的游戏视图把收到的命令记下来；按钮改父组件的信号、卸载组件、改冷却。
import { computed, mount, signal, Text, View, type NativeComponentRef } from "nsh";
import { ActionButton, touchGestures, VirtualStick } from "../../../ts/touch";

const log = signal<string[]>([]);
const fake = {
  command: (_name: string, a: { action: string; down: boolean; player?: number; x?: number; y?: number; source?: string }) =>
    log.set([...log(), [a.action, a.down ? "down" : "up", a.x === undefined ? "" : `${a.x},${a.y}`, a.player === undefined ? "" : `p${a.player}`, a.source ?? ""].filter((s) => s !== "").join(" ")]),
} as unknown as NativeComponentRef;

/** 收手势的一块：手势记进日志。 */
function Gestures() {
  const g = touchGestures((e) => log.set([...log(), e.kind === "dragEnd" ? `dragEnd${e.cancelled ? " cancelled" : ""}` : e.kind]));
  return <View testID="gestures" class="absolute right-0 bottom-0 w-40 h-40" {...g} />;
}

function App() {
  const frame = signal(0);
  const mounted = signal(true);
  const cooldown = signal(0);
  const shown = computed(() => mounted());
  const button = (id: string, run: () => void) => <View testID={id} class="w-10 h-10" onPress={run} />;
  return (
    <View class="flex-1">
      <View class="absolute top-0 right-0 flex-row" style={{ zIndex: 10 }}>
        {button("frame", () => frame.set(frame() + 1))}
        {button("unmount", () => mounted.set(false))}
        {button("cool", () => cooldown.set(0.5))}
        {button("warm", () => cooldown.set(0))}
      </View>
      <Text testID="log" class="absolute top-12 right-0">{`frame ${frame()}: ${log().join(" | ")}`}</Text>
      {shown() ? (
        <View class="absolute top-0 left-0 right-0 bottom-0" style={{ pointerEvents: "box-none" }}>
          <VirtualStick game={() => fake} action="move" player={1} source="pad" down="crouch" class="absolute" style={{ left: 0, bottom: 0, width: 300, height: 400 }} />
          <Gestures />
          <ActionButton game={() => fake} action="fire" label="火" repeat={0.1} repeatDelay={0.2} cooldown={() => cooldown() + frame() * 0} class="absolute right-4 top-40" />
        </View>
      ) : null}
    </View>
  );
}

mount(App);
