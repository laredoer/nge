import { mount, signal, Text, View, type YNode } from "nsh";
import { floatText, gameView, tween, type Scene, type SpriteNode, type TilemapNode, type Tween } from "../../ts/nge";

/** Rust 掉出关卡送回起点时发的事件。 */
interface Events {
  respawn: { falls: number };
}

const GameView = gameView<{}, Events>({ events: ["respawn"] });

const TILE = 32;
const COLS = 80;
const ROWS = 15;
/** 图集：0 石块，1 长草的地面，2 木平台。 */
const [STONE, GRASS, PLANK] = [0, 1, 2];
/** 坑：[起始列, 结束列]。 */
const PITS = [[18, 20], [41, 43], [60, 65]];
/** 平台：[行, 起始列, 结束列]，行 r 的顶边在 y = (14 - r) × 32。坑边上面留空，免得从地面起跳撞头。 */
const PLATFORMS = [[11, 10, 13], [9, 16, 19], [11, 24, 27], [8, 30, 33], [10, 36, 39], [10, 56, 59]];
/** 台阶：[列, 最高的行]。 */
const STAIRS = [[48, 13], [49, 13], [50, 12], [51, 12], [52, 11], [53, 11]];

const tileAt = (c: number, r: number): number => {
  if (c === 0 || c === COLS - 1) return STONE;
  if (r === ROWS - 1) return PITS.some(([a, b]) => c >= a && c <= b) ? -1 : GRASS;
  if (STAIRS.some(([col, top]) => c === col && r >= top)) return STONE;
  return PLATFORMS.some(([row, a, b]) => r === row && c >= a && c <= b) ? PLANK : -1;
};

/** 地面顶边在 y = 0，地图左边在 x = 0。 */
const LEVEL: TilemapNode = {
  id: "level",
  texture: "asset:/@platformer/tiles.png",
  grid: [3, 1],
  tile: TILE,
  cols: COLS,
  x: 0,
  y: (ROWS - 1) * TILE,
  data: Array.from({ length: COLS * ROWS }, (_, i) => tileAt(i % COLS, Math.floor(i / COLS))),
  solid: [STONE, GRASS, PLANK],
  // 角色的速度归 Rust 管，地面不要摩擦
  friction: 0,
};

/** 某一列上方站着的高度（平台、台阶、地面里最高的那个顶边）。 */
const standY = (c: number): number => {
  for (let r = 0; r < ROWS; r++) if (tileAt(c, r) >= 0) return (ROWS - 1 - r) * TILE;
  return 0;
};

/** 金币：平台和台阶上面一个，地面上每隔几格一个。 */
const COINS: { id: string; x: number; y: number }[] = [
  ...PLATFORMS.map(([, a, b], i) => ({ id: `coin-p${i}`, x: ((a + b + 1) / 2) * TILE, y: standY(Math.floor((a + b) / 2)) + 28 })),
  ...[6, 23, 35, 46, 66, 70].map((c, i) => ({ id: `coin-g${i}`, x: (c + 0.5) * TILE, y: standY(c) + 28 })),
  { id: "coin-s", x: 53.5 * TILE, y: standY(53) + 28 },
  // 只有坐竖着的移动平台上去才够得着
  { id: "coin-v", x: 42.5 * TILE, y: 200 },
];

/** 移动平台：kinematic 精灵 + 来回一直播的补间。横的那块在第三个坑上面来回；竖的那块在第二个坑里上下，把角色送到高处的金币（底下是坑，不会把人压进地里）。 */
const LIFTS: { node: SpriteNode; to: Tween["to"]; duration: number }[] = [
  { node: { id: "lift-h", x: 61 * TILE, y: -8, w: 64, h: 16 }, to: { x: 65 * TILE }, duration: 2 },
  { node: { id: "lift-v", x: 42.5 * TILE, y: -8, w: 64, h: 16 }, to: { y: 152 }, duration: 2.5 },
];

const GOAL: SpriteNode = { id: "goal", x: 76 * TILE, y: 40, w: 16, h: 80, color: "#6bcb77", body: { kind: "fixed", sensor: true, collide: true } };

function App() {
  let game: YNode | undefined;
  const taken = signal<string[]>([]);
  const falls = signal(0);
  const won = signal<number | null>(null);
  const started = Date.now();

  const scene = (): Scene => ({
    background: "#1b2233",
    physics: { gravity: [0, -1800] },
    camera: { follow: "player", y: 60, smooth: 0.15, bounds: { left: 0, right: COLS * TILE, bottom: -160, top: (ROWS - 1) * TILE } },
    tilemaps: [LEVEL],
    sprites: [
      { id: "player", x: 3 * TILE, y: 16, w: 24, h: 32, z: 5, color: "#ff7a59", body: { fixedRotation: true, friction: 0, collide: true } },
      ...COINS.filter((c) => !taken().includes(c.id)).map(
        (c): SpriteNode => ({ id: c.id, x: c.x, y: c.y, w: 14, z: 2, rotation: 45, color: "#ffd93d", body: { kind: "fixed", sensor: true } }),
      ),
      ...LIFTS.map(({ node }): SpriteNode => ({ ...node, z: 1, color: "#c08552", body: { kind: "kinematic", friction: 0 } })),
      GOAL,
    ],
  });

  return (
    <View class="flex-1 bg-black">
      <Text class="text-white p-4">
        ← → / A D / 手柄 移动 · 空格 / ↑ / A 跳 · 金币 {taken().length} / {COINS.length} · 掉下去 {falls()} 次
        {won() === null ? "" : ` · 通关！用时 ${won()} 秒`}
      </Text>
      <GameView
        class="flex-1"
        ref={(n: YNode) => {
          game = n;
          // 场景和命令按顺序到引擎：平台先建出来，再开始来回
          for (const { node, to, duration } of LIFTS) tween(game, { id: node.id, to, duration, ease: "easeInOut", repeat: -1, yoyo: true });
        }}
        scene={scene()}
        onCollide={(e) => {
          if (!e.started || (e.a !== "player" && e.b !== "player")) return;
          const other = e.a === "player" ? e.b : e.a;
          const coin = COINS.find((c) => c.id === other);
          if (coin && !taken().includes(coin.id)) {
            taken.set([...taken(), coin.id]);
            floatText(game, { x: coin.x, y: coin.y + 10, text: "+1", color: "#ffd93d", duration: 0.6 });
          } else if (other === "goal" && won() === null) {
            won.set(Math.round((Date.now() - started) / 100) / 10);
          }
        }}
        onRespawn={(e) => falls.set(e.falls)}
      />
    </View>
  );
}

mount(App);
