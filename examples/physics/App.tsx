import { mount, signal, Text, View, type YNode } from "nsh";
import { Button } from "nsh/base";
import { burst, gameView, push, sound, type Burst, type Scene, type SpriteNode } from "../../ts/nge";

const GameView = gameView();

const SPARK = "asset:/@physics/spark.png";
const DING = "asset:/@physics/ding.wav";
const POP = "asset:/@physics/pop.wav";
const MUSIC = "asset:/@physics/music.wav";
const FLOOR = -260;
const WALL = 320;
const MAX_BALLS = 1200;
const COLORS = ["#ff6b6b", "#ffd93d", "#6bcB77", "#4d96ff", "#c77dff"];

interface Ball {
  id: string;
  x: number;
  y: number;
  r: number;
  color: string;
}

/** 点中弹球时冒的一小团火花。 */
const SPARKS: Burst = {
  count: 40,
  lifetime: [0.2, 0.5],
  speed: [60, 200],
  size: [8, 0],
  color: ["#ffffff", "#ffd06000"],
  texture: SPARK,
};

/** 不会动的东西：地面、两面墙；最底下一层透明的「天空」接住点空地的点击。 */
const STATIC: SpriteNode[] = [
  { id: "sky", w: 4000, h: 4000, z: -10, color: "#00000000", tap: true },
  { id: "floor", y: FLOOR - 20, w: WALL * 2 + 40, h: 40, color: "#3a4256", body: { kind: "fixed" } },
  { id: "wall-l", x: -WALL - 10, y: FLOOR + 480, w: 20, h: 1000, color: "#3a4256", body: { kind: "fixed" } },
  { id: "wall-r", x: WALL + 10, y: FLOOR + 480, w: 20, h: 1000, color: "#3a4256", body: { kind: "fixed" } },
];

/** 一开始就有的一摞箱子。 */
const CRATES: SpriteNode[] = Array.from({ length: 8 }, (_, i) => ({
  id: `crate-${i}`,
  x: 120 + (i % 2) * 44 - 22,
  y: FLOOR + 22 + Math.floor(i / 2) * 44,
  w: 40,
  color: "#b07a4a",
  body: { friction: 0.8 },
}));

let next = 0;
const ball = (x: number, y: number): Ball => ({
  id: `ball-${next++}`,
  x,
  y,
  r: 8 + Math.random() * 10,
  color: COLORS[next % COLORS.length],
});

function App() {
  const balls = signal<Ball[]>([]);
  const hits = signal(0);
  const fps = signal(0);
  const muted = signal(false);
  let game: YNode | undefined;
  /** 球的半径，按 id 查：撞击声的音高跟着球的大小走。 */
  const radius = new Map<string, number>();
  /** 最近一段时间发出去的撞击声：碰撞成片时 TSX 这边先限一下，不往引擎塞一堆 command。 */
  let recent: number[] = [];

  const ding = (id: string) => {
    const now = Date.now();
    recent = recent.filter((t) => now - t < 100);
    const r = radius.get(id);
    if (r === undefined || recent.length >= 6) return;
    recent.push(now);
    sound(game, { src: DING, volume: 0.35, pitch: 14 / r });
  };

  const add = (list: Ball[]) => {
    const next = [...balls(), ...list].slice(-MAX_BALLS);
    radius.clear();
    for (const b of next) radius.set(b.id, b.r);
    balls.set(next);
  };

  const scene = (): Scene => ({
    background: "#10131c",
    audio: { music: MUSIC, musicVolume: 0.5, muted: muted(), preload: [DING, POP] },
    sprites: [
      ...STATIC,
      ...CRATES,
      ...balls().map((b) => ({
        id: b.id,
        x: b.x,
        y: b.y,
        w: b.r * 2,
        color: b.color,
        texture: SPARK,
        z: 1,
        tap: true,
        body: { shape: "circle" as const, restitution: 0.5, collide: true },
      })),
    ],
  });

  return (
    <View class="flex-1 bg-black">
      <View class="flex-row items-center gap-3 p-4">
        <Text class="text-white flex-1">
          {balls().length} 个球 · 碰撞 {hits()} 次 · {fps()} FPS
        </Text>
        <Button class="bg-blue-600 active:bg-blue-800 rounded px-3 py-1" onPress={() => add(Array.from({ length: 200 }, () => ball((Math.random() - 0.5) * (WALL - 30) * 2, FLOOR + 100 + Math.random() * 300)))}>
          <Text class="text-white">+200</Text>
        </Button>
        <Button class="bg-gray-600 active:bg-gray-800 rounded px-3 py-1" onPress={() => {
            radius.clear();
            balls.set([]);
          }}>
          <Text class="text-white">清空</Text>
        </Button>
        <Button class="bg-gray-600 active:bg-gray-800 rounded px-3 py-1" onPress={() => muted.set(!muted())}>
          <Text class="text-white">{muted() ? "开声音" : "静音"}</Text>
        </Button>
      </View>
      <GameView
        class="flex-1"
        ref={(n: YNode) => (game = n)}
        scene={scene()}
        onTap={(t) => {
          if (t.id === "sky") {
            // 只在两面墙之间生成，免得掉到墙外一直往下落
            add([ball(Math.max(-WALL + 20, Math.min(WALL - 20, t.x)), t.y)]);
          } else if (t.id.startsWith("ball-")) {
            push(game, { id: t.id, x: 0, y: 600 });
            burst(game, { ...SPARKS, x: t.x, y: t.y });
            sound(game, { src: POP, volume: 0.6 });
          }
        }}
        onCollide={(c) => {
          if (!c.started) return;
          hits.set(hits() + 1);
          ding(c.a.startsWith("ball-") ? c.a : c.b);
        }}
        onFps={(s) => fps.set(s.fps)}
      />
    </View>
  );
}

mount(App);
