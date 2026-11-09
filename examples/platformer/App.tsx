import { batch, effect, mount, type NativeComponentRef, platform, signal, Text, View } from "nsh";
import { floatText, gameView, KeyButton, push, saved, saveSlots, sound, tween, type Scene, type SpriteNode } from "../../ts/nge";
import { COIN_SHADER, FLASH, FLASH_MS, levelAudio, night, portalOver, SHADERS } from "./effects";
import { levelKey, otherLocale, strings } from "./strings";
import { NEW_SAVE, nextSlot, SAVE_MIGRATIONS, SAVE_VERSION, saveDataOf, saveSummary, SLOTS } from "./savefile";
import { hpBar, outOfHearts, TITLE_HOLD_MS, titleBubble } from "./hud";
import { finish, flagOn, LEVELS, ROWS, TILE, TOWER_TRIGGER, towerAt, type Progress } from "./levels";

/** Rust 发的事件：掉出关卡或被敌人撞到送回起点、按 R 重新开始、踩到敌人。 */
interface Events {
  respawn: { falls: number };
  restart: {};
  stomp: { id: string; x: number; y: number };
}

const GameView = gameView<{}, Events>({ events: ["respawn", "restart", "stomp"] });

const COIN_SOUND = "asset:/@platformer/coin.wav";
const WIN_SOUND = "asset:/@platformer/win.wav";

/** 同一只幽灵发现角色的声音多久最多响一次（毫秒）。 */
const SPOT_COOLDOWN_MS = 2000;

/** 通关后多久进下一关（毫秒）。 */
const NEXT_DELAY = 1500;
/** 幽灵比角色慢一截，看得见 7 格，离家 14 格就回去。 */
const GHOST_CHASE = { target: "player", speed: 110, sight: 224, leash: 448 };

/** 视差背景：远山（一张能无缝接上的山丘贴图，横向重复铺满）跟着相机挪 0.2、树挪 0.5，竖直方向跟着关卡（立在地上）。z < 0，画在 3D 箱子后面。树按相机在 x = 0 时摆，所以铺得宽。 */
const BACKDROP: SpriteNode[] = [
  // 底边对齐地面的底边
  // 模型和精灵按深度混排（camera3d.interleave），背景的 z 要比箱子（前后各 20 多）更靠后，才不会盖住箱子
  { id: "hills", texture: "asset:/@platformer/hills.ktx2", x: 0, y: 120 - TILE, w: 384, h: 240, z: -100, color: "#232c42", parallax: [0.2, 1], repeat: "x" },
  ...Array.from({ length: 24 }, (_, i): SpriteNode[] => {
    const x = -600 + i * 110 + (i % 4) * 17;
    return [
      { id: `trunk-${i}`, x, y: 40, w: 10, h: 80, z: -60, color: "#2a3550", parallax: [0.5, 1] },
      { id: `crown-${i}`, x, y: 90 + (i % 2) * 14, w: 44, rotation: 45, z: -50, color: "#2f3d5c", parallax: [0.5, 1] },
    ];
  }).flat(),
];


/** 文案（见 docs/localization.md）：默认跟系统语言，按 L 在中文和英文之间切，记在设备上。 */
const lang = strings();
const t = lang.t;

function App() {
  const chosenLang = saved<string | null>("lang", null);
  effect(() => lang.setLocale(chosenLang()));
  let game: NativeComponentRef | undefined;
  /** 第几关（从 0 数）、第几次开始：选同一关也算新的一局，引擎照样重建。 */
  /** 局数从启动时间起数：开发时只重载 JS、引擎还留着时，第一局的 level 也和上次不同，照样换关。 */
  const current = signal({ index: 0, round: Date.now() });
  const level = () => current().index;
  /** 场景的 `level`：「关卡号-局数」。 */
  const levelId = () => `${level() + 1}-${current().round}`;
  /** 三个存档位（见 docs/saves.md）：以前 `saved("platformer")` 存的进度自动进第一个。 */
  const slots = saveSlots({
    key: "platformer",
    slots: SLOTS,
    version: SAVE_VERSION,
    migrations: SAVE_MIGRATIONS,
    parse: saveDataOf,
    summary: saveSummary,
    adopt: { key: "platformer", version: 1 },
  });
  /** 当前存档位（从 0 数）和它的进度。 */
  const slot = signal(0);
  const progress = signal<Progress>(NEW_SAVE.progress);
  /** 当前存档位坏了：不覆盖，直到在它上面通关一次。`"newer"` 是更新的游戏版本存的，永远不覆盖。 */
  const brokenSlot = signal<false | "broken" | "newer">(false);
  /** 要换到的存档位：连着按 S 时按它往下数，不等读档回来。 */
  let target = 0;
  /** 换存档位的次数：读档晚到时只认最后一次。 */
  let picking = 0;
  /** 读完存档位以后才存：开局那一关的 levelStart 不能把还没读出来的档覆盖掉。 */
  let picked = false;
  const taken = signal<string[]>([]);
  const falls = signal(0);
  const won = signal<number | null>(null);
  const stomped = signal<string[]>([]);
  /** 幽灵上一次发现角色时响声的时间。 */
  const spotted = new Map<string, number>();
  /** 这一局的 3D 箱子已经撞散了。 */
  let toppled = false;
  /** 箱子摞了第几次：按 R 重来（关卡不重建）时也换 id，让箱子重新摞起来。 */
  const towerRound = signal(0);
  /** 关卡名气泡（存关卡号，画的时候取文案，换语言跟着变）：每关开始时冒出来，打完过一会儿去掉。 */
  const title = signal<number | null>(null);
  let titleEnd: ReturnType<typeof setTimeout> | undefined;
  /** 刚被撞回起点：角色闪白一下。 */
  const flashing = signal(false);
  let flashEnd: ReturnType<typeof setTimeout> | undefined;
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
    toppled = false;
    towerRound.set(towerRound() + 1);
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
      // 上一关的名字不带到新关卡上：新关卡建好（levelStart）时再冒
      clearTimeout(titleEnd);
      title.set(null);
      current.set({ index, round: current().round + 1 });
    });
  };

  /** 存进当前存档位：进度和现在这关。 */
  const persist = () => {
    if (!picked || brokenSlot() === "newer") return;
    slots.save(slot(), { progress: progress(), level: level() }).then(
      () => brokenSlot.set(false),
      (e) => console.warn(`存档失败: ${e}`),
    );
  };
  /** 换到第 `index` 个存档位：读它的进度，从它存的那关开始。 */
  const pick = (index: number) => {
    target = index;
    const ticket = ++picking;
    slots.load(index).then((r) => {
      if (ticket !== picking) return;
      picked = true;
      if (r.status === "broken") console.warn(`存档 ${index + 1} 坏了: ${r.reason}`);
      const data = r.status === "ok" ? r.data : NEW_SAVE;
      batch(() => {
        slot.set(index);
        brokenSlot.set(r.status !== "broken" ? false : r.newer ? "newer" : "broken");
        progress.set(data.progress);
      });
      play(data.level);
    });
  };
  slots.loaded.then(() => pick(slots.latest() ?? 0));

  /** 这一关的那摞 3D 箱子（没有就是 null）：id 带着摞的次数，重新开始时重建。 */
  const tower = () => {
    const x = now().tower;
    return x === null ? null : towerAt(x, towerRound());
  };
  /** 角色撞上去：箱子朝右飞散，前后错开一点，上面的飞得高。 */
  const topple = () => {
    const t = tower();
    if (toppled || t === null) return;
    toppled = true;
    t.models.filter((m) => m.body?.kind !== "fixed").forEach((m, i) => push(game, { id: m.id, x: 160 + i * 25, y: 120 + i * 40, z: (i % 2 ? 1 : -1) * 60 }));
  };

  const scene = (): Scene => ({
    level: levelId(),
    background: LEVELS[level()].night ? "#0b0f1e" : "#1b2233",
    shaders: SHADERS,
    ...(LEVELS[level()].night ? night(now().goal) : {}),
    audio: { preload: [COIN_SOUND, WIN_SOUND], ...levelAudio(LEVELS[level()].night ?? false) },
    physics: { gravity: [0, -1800] },
    camera: { follow: "player", y: 60, smooth: 0.15, bounds: { left: 0, right: now().right, bottom: -160, top: (ROWS - 1) * TILE } },
    camera3d: { match2d: true, interleave: true },
    // 箱子顶上的小箱子在大箱子顶面投下影子
    lights: [{ id: "sun", kind: "directional", direction: [-1, -2, -1.5], shadows: true }],
    ambient: { brightness: 0.35 },
    // 3D 箱子摞起来的地面和关卡的地面一样高
    physics3d: { gravity: [0, -1800, 0] },
    models: [now().crate.model, flagOn(now().goal), ...(tower()?.models ?? [])],
    bars: [hpBar(falls())],
    bubbles: title() === null ? [] : [titleBubble(t(levelKey(title()!)))],
    joints: now().swing.joints,
    tilemaps: now().tilemaps,
    sprites: [
      ...BACKDROP,
      { id: "player", x: 3 * TILE, y: 16, w: 24, h: 32, z: 5, color: "#ff7a59", body: { kind: "character", friction: 0, collide: true }, ...(flashing() ? FLASH : {}) },
      // 金币自己发光，夜里也不暗
      ...now().coins.filter((c) => !taken().includes(c.id)).map(
        (c): SpriteNode => ({ id: c.id, x: c.x, y: c.y, w: 14, z: 2, rotation: 45, color: "#ffd93d", body: { kind: "fixed", sensor: true }, ...COIN_SHADER, unlit: true }),
      ),
      ...now().lifts.map(({ node }): SpriteNode => ({ ...node, z: 1, color: "#c08552", body: { kind: "kinematic", friction: 0 } })),
      // 踩到的不删：去掉刚体、变透明，巡逻补间接着走，重新开始时直接恢复
      ...now().enemies.map(({ id, x }): SpriteNode =>
        stomped().includes(id)
          ? { id, x, y: 12, w: 24, h: 24, z: 4, color: "#00000000" }
          : { id, x, y: 12, w: 24, h: 24, z: 4, color: "#9b59b6", body: { kind: "kinematic", friction: 0 } },
      ),
      // 幽灵：看见角色就绕开平台追过来，追远了飘回去（见 docs/ai.md）；踩到的和敌人一样变透明
      ...now().ghosts.map(({ id, x, y }): SpriteNode =>
        stomped().includes(id)
          ? { id, x, y, w: 22, h: 22, z: 4, color: "#00000000" }
          : { id, x, y, w: 22, h: 22, z: 4, color: "#c8d2ffd0", unlit: true, body: { kind: "kinematic", friction: 0 }, chase: GHOST_CHASE },
      ),
      ...now().ramps,
      ...(tower() === null ? [] : [tower()!.trigger]),
      now().crate.body,
      ...now().swing.sprites,
      now().goal,
      ...portalOver(now().goal),
    ],
  });

  const best = () => progress().best[String(level() + 1)];

  return (
    <View class="flex-1 bg-black">
      <Text class="text-white p-4">
        {platform.desktop ? t("help", { count: progress().unlocked }) : ""}
        {t("slot", { n: slot() + 1 })}
        {brokenSlot() === "newer" ? t("slotNewer") : brokenSlot() ? t("slotBroken") : ""}
        {" · "}
        {t("level", { n: level() + 1, name: t(levelKey(level())) })}
        {" · "}
        {t("coins", { got: taken().length, all: now().coins.length, count: now().coins.length })}
        {" · "}
        {t("falls", { count: falls() })}
        {best() === undefined ? "" : t("best", { seconds: best()! })}
        {won() === null ? "" : t("won", { seconds: won()! })}
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
          clearTimeout(titleEnd);
          title.set(level());
          // 换关时记下打到第几关；坏了的存档位不覆盖
          if (!brokenSlot()) persist();
        }}
        onCollide={(e) => {
          if (switching || !e.started || (e.a !== "player" && e.b !== "player")) return;
          const other = e.a === "player" ? e.b : e.a;
          if (other === TOWER_TRIGGER) return topple();
          const coin = now().coins.find((c) => c.id === other);
          if (coin && !taken().includes(coin.id)) {
            taken.set([...taken(), coin.id]);
            floatText(game, { x: coin.x, y: coin.y + 10, text: "+1", color: "#ffd93d", duration: 0.6 });
            sound(game, { src: COIN_SOUND, volume: 0.5 });
          } else if (other === "goal" && won() === null) {
            const seconds = Math.round((Date.now() - started) / 100) / 10;
            won.set(seconds);
            progress.set(finish(progress(), level(), seconds));
            persist();
            sound(game, { src: WIN_SOUND, volume: 0.6 });
            // 最后一关之后回到第一关
            next = setTimeout(() => play((level() + 1) % LEVELS.length), NEXT_DELAY);
          }
        }}
        onKey={(e) => {
          if (e.down && !e.repeat && e.code === "KeyS") return pick(nextSlot(target));
          if (e.down && !e.repeat && e.code === "KeyL") return chosenLang.set(otherLocale(lang.locale()));
          // Digit1 … Digit9：选已经解锁的关
          const n = /^Digit([1-9])$/.exec(e.code)?.[1];
          if (!e.down || e.repeat || n === undefined) return;
          const index = Number(n) - 1;
          if (index < LEVELS.length && index < progress().unlocked) play(index);
        }}
        onRespawn={(e) => {
          if (switching) return;
          falls.set(e.falls);
          // 血掉光了：整关重来（重建关卡，引擎那边的失手次数也清零）
          if (outOfHearts(e.falls)) return play(level());
          flashing.set(true);
          clearTimeout(flashEnd);
          flashEnd = setTimeout(() => flashing.set(false), FLASH_MS);
        }}
        onChase={(e) => {
          if (switching || e.state === "idle") return;
          // 发现角色时从幽灵那儿响一声低八度的金币声：在左边的听起来在左边。同一只两秒内只响一次，追追停停时不吵
          if (e.state === "chase" && Date.now() - (spotted.get(e.id) ?? 0) > SPOT_COOLDOWN_MS) {
            spotted.set(e.id, Date.now());
            sound(game, { src: COIN_SOUND, pitch: 0.5, volume: 0.6, x: e.x, y: e.y });
          }
          floatText(game, { x: e.x, y: e.y + 18, text: e.state === "chase" ? "!" : "?", color: "#c8d2ff", duration: 0.6 });
        }}
        onStomp={(e) => {
          if (switching) return;
          stomped.set([...stomped(), e.id]);
          floatText(game, { x: e.x, y: e.y + 10, text: t("stomp"), color: "#c39bd3", duration: 0.6 });
        }}
        onTyped={(e) => {
          if (e.id !== "title" || switching) return;
          clearTimeout(titleEnd);
          titleEnd = setTimeout(() => title.set(null), TITLE_HOLD_MS);
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
        <KeyButton game={props.game} code="Space" label={t("jump")} class="w-20 h-20" />
      </View>
    </View>
  );
}

mount(App);
