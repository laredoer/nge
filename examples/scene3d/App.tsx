import { mount, type NativeComponentRef, signal, Text, View } from "nsh";
import { burst, floatText, gameView, type Burst, type Scene } from "../../ts/nge";

const GameView = gameView();

const asset = (file: string) => `asset:/@scene3d/${file}`;
const FONT = asset("Inter-Bold.ttf");
const SPARK = asset("spark.png");
const CRATES = [
  { id: "red", x: -2.2, spin: 25 },
  { id: "green", x: 0, spin: 40 },
  { id: "blue", x: 2.2, spin: 60 },
];

/** 点中箱子时炸的那一团。 */
const BOOM: Burst = {
  count: 250,
  lifetime: [0.4, 0.9],
  speed: [80, 280],
  gravity: [0, -320],
  size: [14, 2],
  color: ["#ffd060", "#ff300000"],
  texture: SPARK,
};

const scene = (): Scene => ({
  background: "#0a0f1e",
  camera3d: { x: 0, y: 3, z: 7.5, target: [0, 0.4, 0] },
  lights: [{ id: "sun", kind: "directional", direction: [-1, -2, -1.5] }],
  ambient: { brightness: 0.25 },
  models: CRATES.map((c) => ({ id: c.id, model: asset("crate.glb"), x: c.x, y: 0.5, spin: c.spin, tap: true })),
  texts: [
    ...CRATES.map((c) => ({ id: `name-${c.id}`, text: c.id, follow: c.id, lift: 1.1, font: FONT, size: 18 })),
    { id: "hint", text: "tap a crate", screen: true, x: 16, y: 16, anchor: "top-left" as const, font: FONT, color: "#a0b0d0" },
  ],
  sprites: [{ id: "chimney", color: "#40404a", w: 28, h: 90, x: -230, y: -150 }],
  emitters: [
    {
      id: "smoke",
      x: -230,
      y: -105,
      rate: 40,
      angle: [80, 100],
      speed: [30, 70],
      lifetime: [1.5, 2.5],
      gravity: [6, 10],
      size: [10, 36],
      color: ["#b0b0b880", "#60606800"],
      texture: SPARK,
    },
  ],
});

function App() {
  const score = signal(0);
  const fps = signal(0);
  let game: NativeComponentRef | undefined;

  return (
    <View class="flex-1 bg-black">
      <Text class="text-white p-4">
        得分 {score()} · {fps()} FPS
      </Text>
      <GameView
        class="flex-1"
        ref={(n: NativeComponentRef) => (game = n)}
        scene={scene()}
        onTap={(t) => {
          if (!t.point) return;
          score.set(score() + 10);
          burst(game, { ...BOOM, x: t.x, y: t.y });
          floatText(game, { x: t.x, y: t.y + 24, text: "+10", font: FONT, color: "#ffe680" });
        }}
        onFps={(s) => fps.set(s.fps)}
      />
    </View>
  );
}

mount(App);
