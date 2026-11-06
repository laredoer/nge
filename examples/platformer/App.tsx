import { batch, mount, type NativeComponentRef, platform, signal, Text, View } from "nsh";
import { floatText, gameView, KeyButton, saved, sound, tween, type Scene, type SpriteNode } from "../../ts/nge";
import { finish, LEVELS, NEW_PROGRESS, progressOf, ROWS, TILE, type Progress } from "./levels";

/** Rust 发的事件：掉出关卡或被敌人撞到送回起点、按 R 重新开始、踩到敌人。 */
interface Events {
  respawn: { falls: number };
  restart: {};
  stomp: { id: string; x: number; y: number };
}

const GameView = gameView<{}, Events>({ events: ["respawn", "restart", "stomp"] });

const COIN_SOUND = "asset:/@platformer/coin.wav";
const WIN_SOUND = "asset:/@platformer/win.wav";

/** 通关后多久进下一关（毫秒）。 */
const NEXT_DELAY = 1500;

/** 视差背景：远山（一张能无缝接上的山丘贴图，横向重复铺满）跟着相机挪 0.2、树挪 0.5，竖直方向跟着关卡（立在地上）。z < 0，画在 3D 箱子后面。树按相机在 x = 0 时摆，所以铺得宽。 */
const BACKDROP: SpriteNode[] = [
  // 底边对齐地面的底边
  { id: "hills", texture: "asset:/@platformer/hills.png", x: 0, y: 120 - TILE, w: 384, h: 240, z: -10, color: "#232c42", parallax: [0.2, 1], repeat: "x" },
  ...Array.from({ length: 24 }, (_, i): SpriteNode[] => {
    const x = -600 + i * 110 + (i % 4) * 17;
    return [
      { id: `trunk-${i}`, x, y: 40, w: 10, h: 80, z: -6, color: "#2a3550", parallax: [0.5, 1] },
      { id: `crown-${i}`, x, y: 90 + (i % 2) * 14, w: 44, rotation: 45, z: -5, color: "#2f3d5c", parallax: [0.5, 1] },
    ];
  }).flat(),
];


function App() {
  let game: NativeComponentRef | undefined;
  /** 第几关（从 0 数）、第几次开始：选同一关也算新的一局，引擎照样重建。 */
  /** 局数从启动时间起数：开发时只重载 JS、引擎还留着时，第一局的 level 也和上次不同，照样换关。 */
  const current = signal({ index: 0, round: Date.now() });
  const level = () => current().index;
  /** 场景的 `level`：「关卡号-局数」。 */
  const levelId = () => `${level() + 1}-${current().round}`;
  const stored = saved<Progress>("platformer", NEW_PROGRESS);
  const progress = () => progressOf(stored());
  const taken = signal<string[]>([]);
  const falls = signal(0);
  const won = signal<number | null>(null);
  const stomped = signal<string[]>([]);
  let started = Date.now();
  let next: ReturnType<typeof setTimeout> | undefined;
  /** 换关后、新关卡建好（`levelStart`）之前：路上的事件是上一关的，不算。 */
  let switching = true;
  const now = () => LEVELS[level()].built;

  // 在 levelStart 里发：平台、敌人已经建出来了
  const startTweens = () => {
    for (const { node, to, duration } of now().lifts) tween(game, { id: node.id, to, duration, ease: "easeInOut", repeat: -1, yoyo: true });
    for (const { id, to } of now().enemies) tween(game, { id, to, duration: 1.5, ease: "linear", repeat: -1, yoyo: true });
  };
  const reset = () => {
    taken.set([]);
    stomped.set([]);
    falls.set(0);
    won.set(null);
    started = Date.now();
  };
  /** 换到第 `index` 关（从 0 数）：场景的 `level` 变了，引擎把实体全部重建，建好后发 `levelStart`。 */
  const play = (index: number) => {
    clearTimeout(next);
    next = undefined;
    switching = true;
    // 一次换关只发一次场景
    batch(() => {
      reset();
      current.set({ index, round: current().round + 1 });
    });
  };

  const scene = (): Scene => ({
    level: levelId(),
    background: "#1b2233",
    audio: { preload: [COIN_SOUND, WIN_SOUND] },
    physics: { gravity: [0, -1800] },
    camera: { follow: "player", y: 60, smooth: 0.15, bounds: { left: 0, right: now().right, bottom: -160, top: (ROWS - 1) * TILE } },
    camera3d: { match2d: true },
    lights: [{ id: "sun", kind: "directional", direction: [-1, -2, -1.5] }],
    ambient: { brightness: 0.35 },
    models: [now().crate.model],
    joints: now().swing.joints,
    tilemaps: now().tilemaps,
    sprites: [
      ...BACKDROP,
      { id: "player", x: 3 * TILE, y: 16, w: 24, h: 32, z: 5, color: "#ff7a59", body: { fixedRotation: true, friction: 0, collide: true } },
      ...now().coins.filter((c) => !taken().includes(c.id)).map(
        (c): SpriteNode => ({ id: c.id, x: c.x, y: c.y, w: 14, z: 2, rotation: 45, color: "#ffd93d", body: { kind: "fixed", sensor: true } }),
      ),
      ...now().lifts.map(({ node }): SpriteNode => ({ ...node, z: 1, color: "#c08552", body: { kind: "kinematic", friction: 0 } })),
      // 踩到的不删：去掉刚体、变透明，巡逻补间接着走，重新开始时直接恢复
      ...now().enemies.map(({ id, x }): SpriteNode =>
        stomped().includes(id)
          ? { id, x, y: 12, w: 24, h: 24, z: 4, color: "#00000000" }
          : { id, x, y: 12, w: 24, h: 24, z: 4, color: "#9b59b6", body: { kind: "kinematic", friction: 0 } },
      ),
      now().crate.body,
      ...now().swing.sprites,
      now().goal,
    ],
  });

  const best = () => progress().best[String(level() + 1)];

  return (
    <View class="flex-1 bg-black">
      <Text class="text-white p-4">
        {platform.desktop ? `← → / A D / 手柄 移动 · 空格 / ↑ / A 跳 · R 重来 · 数字键选关（解锁了 ${progress().unlocked} 关）· Q 调试 · ` : ""}第 {level() + 1} 关 {LEVELS[level()].name} · 金币 {taken().length} / {now().coins.length} · 失手 {falls()} 次
        {best() === undefined ? "" : ` · 最好 ${best()} 秒`}
        {won() === null ? "" : ` · 通关！用时 ${won()} 秒`}
      </Text>
      <GameView
        class="flex-1"
        ref={(n: NativeComponentRef) => (game = n)}
        scene={scene()}
        onLevelStart={(e) => {
          // 连着选了两次关：前一次的 levelStart 晚到，不算
          if (e.level !== levelId()) return;
          switching = false;
          startTweens();
        }}
        onCollide={(e) => {
          if (switching || !e.started || (e.a !== "player" && e.b !== "player")) return;
          const other = e.a === "player" ? e.b : e.a;
          const coin = now().coins.find((c) => c.id === other);
          if (coin && !taken().includes(coin.id)) {
            taken.set([...taken(), coin.id]);
            floatText(game, { x: coin.x, y: coin.y + 10, text: "+1", color: "#ffd93d", duration: 0.6 });
            sound(game, { src: COIN_SOUND, volume: 0.5 });
          } else if (other === "goal" && won() === null) {
            const seconds = Math.round((Date.now() - started) / 100) / 10;
            won.set(seconds);
            stored.set(finish(progress(), level(), seconds));
            sound(game, { src: WIN_SOUND, volume: 0.6 });
            // 最后一关之后回到第一关
            next = setTimeout(() => play((level() + 1) % LEVELS.length), NEXT_DELAY);
          }
        }}
        onKey={(e) => {
          // Digit1 … Digit9：选已经解锁的关
          const n = /^Digit([1-9])$/.exec(e.code)?.[1];
          if (!e.down || e.repeat || n === undefined) return;
          const index = Number(n) - 1;
          if (index < LEVELS.length && index < progress().unlocked) play(index);
        }}
        onRespawn={(e) => {
          if (!switching) falls.set(e.falls);
        }}
        onStomp={(e) => {
          if (switching) return;
          stomped.set([...stomped(), e.id]);
          floatText(game, { x: e.x, y: e.y + 10, text: "踩！", color: "#c39bd3", duration: 0.6 });
        }}
        onRestart={() => {
          clearTimeout(next);
          next = undefined;
          reset();
        }}
      />
      {platform.desktop ? null : <TouchPad game={() => game} />}
    </View>
  );
}

/** 手机上的触屏按钮，叠在游戏视图下方：左边跑，右边跳和重来（见 docs/touch-controls.md）。 */
function TouchPad(props: { game: () => NativeComponentRef | undefined }) {
  return (
    <View class="absolute bottom-8 left-6 right-6 flex-row justify-between items-end">
      <View class="flex-row gap-4">
        <KeyButton game={props.game} code="ArrowLeft" label="←" />
        <KeyButton game={props.game} code="ArrowRight" label="→" />
      </View>
      <View class="flex-row gap-4 items-end">
        <KeyButton game={props.game} code="KeyR" label="↺" class="w-12 h-12" />
        <KeyButton game={props.game} code="Space" label="跳" class="w-20 h-20" />
      </View>
    </View>
  );
}

mount(App);
