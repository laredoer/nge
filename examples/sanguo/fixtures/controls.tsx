// controls.test.tsx 用的：假的游戏视图把收到的命令记下来，`frame` 每按一下换一个新的 HUD 对象（像游戏每帧那样）。
import { mount, signal, Text, View, type NativeComponentRef } from "nsh";
import { EndScreen, TouchLayer } from "../controls";
import type { Hud } from "../hud";

const log = signal<string[]>([]);
const fake = {
  command: (_name: string, a: { action: string; down: boolean; x?: number; y?: number }) => log.set([...log(), `${a.action} ${a.down ? "down" : "up"}${a.x === undefined ? "" : ` ${a.x},${a.y}`}`]),
} as unknown as NativeComponentRef;

function App() {
  const frame = signal(0);
  const over = signal(false);
  const rage = signal(false);
  const restarts = signal(0);
  let clock = 0;
  const hud = (): Hud => ({
    hp: 1,
    rage: frame() / 1000,
    rageFull: rage(),
    thrust: "",
    kills: `${frame()}`,
    boss: null,
    banner: null,
    end: over() ? { title: "胜", info: `${frame()}`, restart: "R", won: true } : null,
    paused: null,
  });
  const button = (id: string, run: () => void) => <View testID={id} class="w-10 h-10" onPress={run} />;
  return (
    <View class="flex-1">
      <View class="absolute top-0 left-0 flex-row" style={{ zIndex: 10 }}>
        {button("frame", () => frame.set(frame() + 1))}
        {button("over", () => over.set(true))}
        {button("rage", () => rage.set(!rage()))}
        {button("later", () => (clock += 1000))}
      </View>
      <Text testID="log" class="absolute top-12 left-0">{`${log().join(" | ")};restarts ${restarts()}`}</Text>
      <TouchLayer hud={hud} game={() => fake} cooldown={() => 0} />
      <EndScreen end={() => hud().end} onRestart={() => restarts.set(restarts() + 1)} now={() => clock} />
    </View>
  );
}

mount(App);
