import { mount, type NativeComponentRef, platform, signal, Text, View } from "nsh";
import { burst, floatText, gameView, shake, sound } from "../../ts/nge";
import { footY, zOf } from "./belt";
import { newGame, step, type Game, type GameEvent } from "./game";
import { hudOf } from "./hud";
import { pace } from "./pace";
import { DT, NO_INPUT, type Input } from "./player";
import { strings } from "./strings";
import { sceneOf, SOUNDS } from "./view";
// 打包时才保留 sanguo 的原生模块（web 上游戏视图在那里注册，见 docs/web.md）
import "sanguo/native";

const GameView = gameView();
/** 文案跟系统语言（见 docs/localization.md）。 */
const lang = strings();
const t = lang.t;

const LEFT = ["ArrowLeft", "KeyA", "GamepadLeft", "GamepadLeftStickLeft"];
const RIGHT = ["ArrowRight", "KeyD", "GamepadRight", "GamepadLeftStickRight"];
const UP = ["ArrowUp", "KeyW", "GamepadUp", "GamepadLeftStickUp"];
const DOWN = ["ArrowDown", "KeyS", "GamepadDown", "GamepadLeftStickDown"];
const ATTACK = ["KeyJ", "GamepadX"];
const THRUST = ["KeyK", "GamepadY"];
const JUMP = ["KeyL", "GamepadA"];

/** 按着的键 → 这一步的输入；J / K / L 只在按下的那一下算（`pressed`）。 */
function inputOf(held: Set<string>, pressed: Set<string>): Input {
  const any = (keys: string[], set: Set<string>) => keys.some((k) => set.has(k));
  return {
    ...NO_INPUT,
    x: (any(RIGHT, held) ? 1 : 0) - (any(LEFT, held) ? 1 : 0),
    y: (any(UP, held) ? 1 : 0) - (any(DOWN, held) ? 1 : 0),
    attack: any(ATTACK, pressed),
    thrust: any(THRUST, pressed),
    jump: any(JUMP, pressed),
  };
}

function App() {
  let game: NativeComponentRef | undefined;
  const state = signal<Game>(newGame());
  const paused = signal(false);
  /** 引擎画出了第一帧（web 上加载完了）：之前不推进游戏。 */
  let ready = false;
  /** 第几局：重开时换值，场景整个重建。 */
  const round = signal(1);
  const held = new Set<string>();
  const pressed = new Set<string>();
  let last: number | undefined;
  let acc = 0;

  /** 这一步的音效、震屏、烟、预警。 */
  const play = (events: GameEvent[]) => {
    for (const e of events) {
      if (e.kind === "sound") sound(game, { src: SOUNDS[e.name], volume: e.name === "win" || e.name === "lose" ? 0.8 : 0.55, ...(e.x === undefined ? {} : { x: e.x, y: footY(65) }) });
      else if (e.kind === "shake") shake(game, { strength: e.strength, duration: e.duration });
      else if (e.kind === "burst")
        burst(game, { x: e.x, y: footY(e.d, e.h), z: zOf(e.d) + 6, count: 26, lifetime: [0.5, 1.1], speed: [20, 70], angle: [30, 150], gravity: [0, 25], size: [16, 30], color: ["#a39890c0", "#4a404000"] });
      else if (e.kind === "warn") floatText(game, { x: e.x, y: footY(e.d, 200), text: "!", size: 44, color: "#ff5a3a", duration: 0.8, rise: 30 });
    }
  };

  // 固定步长推进：显示一帧补上该走的步数
  const tick = (now: number) => {
    if (last !== undefined) {
      const r = pace(acc, (now - last) / 1000, { ready, paused: paused() });
      acc = r.acc;
      let g = state();
      const events: GameEvent[] = [];
      for (let i = 0; i < r.steps; i++) {
        const out = step(g, inputOf(held, pressed), DT);
        pressed.clear();
        g = out.game;
        events.push(...out.events);
      }
      if (g !== state()) state.set(g);
      play(events);
    }
    last = now;
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);

  const restart = () => {
    state.set(newGame());
    round.set(round() + 1);
    paused.set(false);
    pressed.clear();
    acc = 0;
  };

  const hud = () => hudOf(state(), paused(), t);

  return (
    <View class="flex-1 bg-black">
      <GameView
        class="flex-1"
        ref={(n: NativeComponentRef) => (game = n)}
        scene={sceneOf(state(), paused(), round())}
        onReady={() => (ready = true)}
        onKey={(e) => {
          if (!e.down) return held.delete(e.code);
          if (e.repeat) return;
          held.add(e.code);
          if (e.code === "KeyP" || e.code === "GamepadMenu") return state().phase === "play" && paused.set(!paused());
          if (e.code === "KeyR") return restart();
          if (!paused()) pressed.add(e.code);
        }}
      />
      <Hud hud={hud} />
    </View>
  );
}

/** 叠在游戏画面上的 HUD。 */
function Hud(props: { hud: () => ReturnType<typeof hudOf> }) {
  const h = props.hud;
  return (
    <View class="absolute top-0 left-0 right-0 bottom-0">
      <View class="absolute top-4 left-4 w-56 gap-1">
        <Text class="text-white text-sm font-bold">{t("hero")}</Text>
        <View class="flex-row items-center gap-2">
          <Text class="text-white text-xs w-8">{t("hp")}</Text>
          <View class="flex-1 h-3 bg-black/60 rounded">
            <View class="h-3 rounded bg-red-500" style={{ width: `${Math.round(h().hp * 100)}%` }} />
          </View>
        </View>
        <View class="flex-row items-center gap-2">
          <Text class="text-white text-xs w-8">{t("rage")}</Text>
          <View class="flex-1 h-2 bg-black/60 rounded">
            <View class={`h-2 rounded ${h().rageFull ? "bg-amber-300" : "bg-amber-700"}`} style={{ width: `${Math.round(h().rage * 100)}%` }} />
          </View>
        </View>
        <Text class="text-amber-200 text-xs">{h().thrust}</Text>
      </View>
      <Text class="absolute top-4 right-4 text-white text-lg font-bold">{h().kills}</Text>
      {h().banner === null ? null : (
        <View class="absolute top-24 left-0 right-0 items-center">
          <Text class="text-amber-100 text-2xl font-bold">{h().banner}</Text>
        </View>
      )}
      {h().boss === null ? null : (
        <View class="absolute bottom-10 left-16 right-16 gap-1">
          <Text class="text-red-200 text-sm font-bold">{h().boss!.name}</Text>
          <View class="h-3 bg-black/60 rounded">
            <View class="h-3 rounded bg-red-700" style={{ width: `${Math.round(h().boss!.hp * 100)}%` }} />
          </View>
        </View>
      )}
      {h().paused === null ? null : (
        <View class="absolute top-0 left-0 right-0 bottom-0 items-center justify-center bg-black/40">
          <Text class="text-white text-2xl">{h().paused}</Text>
        </View>
      )}
      {h().end === null ? null : (
        <View class={`absolute top-0 left-0 right-0 bottom-0 items-center justify-center gap-3 ${h().end!.won ? "bg-amber-900/50" : "bg-black/60"}`}>
          <Text class={`text-4xl font-bold ${h().end!.won ? "text-amber-200" : "text-red-300"}`}>{h().end!.title}</Text>
          <Text class="text-white text-lg">{h().end!.info}</Text>
          <Text class="text-white/80 text-base">{h().end!.restart}</Text>
        </View>
      )}
      {platform.desktop ? <Text class="absolute bottom-2 left-4 right-4 text-white/60 text-xs">{t("help")}</Text> : null}
    </View>
  );
}

mount(App);
