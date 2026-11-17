import { batch, effect, keymap, type NativeComponentRef, platform, safeArea, signal, Text, View } from "nsh";
import { Button } from "nsh/base";
import { ActionButton, floatText, gameView, httpClient, inputMap, netRoom, push, saved, saveSlots, sound, touchGestures, tween, VirtualStick, type ActionEvent, type Gesture, type Scene, type SpriteNode } from "../../ts/nge";
import { DEFAULT_CONTROLS, guestSprite, PLAYER_GROUP } from "./controls";
import { remapPlace, RemapPanel } from "./remap";
import { COIN_SHADER, FLASH, FLASH_MS, levelAudio, night, portalOver, POST, SHADERS, stonesNear } from "./effects";
import { levelKey, otherLocale, strings } from "./strings";
import { NEW_SAVE, nextSlot, SAVE_MIGRATIONS, SAVE_VERSION, saveDataOf, saveSummary, SLOTS } from "./savefile";
import { hpBar, outOfHearts, padPlace, stickPlace, TITLE_HOLD_MS, titleBubble, zoomBy } from "./hud";
import { boardOf, leaderOf, nameTags, netLabel, netNode, netSetting, playerName, remotePlayers, scoresUrl, submitUrl, type Leader } from "./online";
import { campfireAt, finish, flagOn, HERO, JELLY_ZONE, jellyAt, LEVELS, TILE, TOWER_TRIGGER, towerAt, towerSpot, type Built, type Progress } from "./levels";

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

export interface GameProps {
  /** 试玩一关（关卡编辑器用）：只有这一关，不读写存档、不联网、不能按数字选关。 */
  trial?: { name: string; built: Built; night?: boolean };
  /** 试玩时按 Esc 回去。 */
  onExit?: () => void;
}

export function Game(props: GameProps = {}) {
  const trial = props.trial;
  const levels = trial === undefined ? LEVELS : [trial];
  /** 关卡名：试玩用关卡文件里的名字。 */
  const levelName = (index: number) => (trial === undefined ? t(levelKey(index)) : trial.name);
  if (props.onExit !== undefined) keymap.bind("escape", () => props.onExit!());
  const chosenLang = saved<string | null>("lang", null);
  effect(() => lang.setLocale(chosenLang()));
  let game: NativeComponentRef | undefined;
  /** 操作（见 docs/input-actions.md）：绑定存在设备上，引擎按它算动作；改键面板按 K 开关。 */
  const input = inputMap({ key: "platformer", defaults: DEFAULT_CONTROLS });
  const remapping = signal(false);
  /** 镜头缩放：手机上双指捏合（见 docs/touch-input.md）。 */
  const zoom = signal(1);
  const pinchZoom = platform.desktop ? {} : touchGestures((g: Gesture) => g.kind === "pinch" && zoom.set(zoomBy(zoom(), g.factor)));
  const closeRemap = () => {
    input.cancel();
    remapping.set(false);
  };
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
  /** 角色在果冻的感应区里：果冻压一压。 */
  const nearJelly = signal(false);
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
  const now = () => levels[level()].built;
  /** 联网（见 docs/networking.md）：按 N 开关，记在设备上；中继连不上时一直重试，照样单机玩。 */
  const online = netSetting();
  const room = netRoom();
  const http = httpClient(() => game);
  const name = playerName();
  /** 这关的榜首：每关开始、连上中继、通关后去拿；晚到的上一关的不要。 */
  const leader = signal<Leader | null>(null);
  const fetchLeader = () => {
    if (trial !== undefined) return;
    const url = scoresUrl(room.url(), level());
    const wanted = level();
    if (url === null) return;
    http.get(url, { timeout: 5 }).then((r) => {
      if (level() === wanted) leader.set(leaderOf(r));
    });
  };
  const submitScore = (seconds: number) => {
    if (trial !== undefined) return;
    const url = submitUrl(room.url());
    if (url === null) return;
    http.post(url, { board: boardOf(level()), name, score: seconds }, { timeout: 5 }).then(fetchLeader);
  };

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
    nearJelly.set(false);
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
      leader.set(null);
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
  // 试玩不读存档，直接开始
  if (trial === undefined) slots.loaded.then(() => pick(slots.latest() ?? 0));
  else play(0);

  /** 这一关的那摞 3D 箱子（没有就是 null）：id 带着摞的次数，重新开始时重建。 */
  const tower = () => {
    const x = now().tower;
    return x === null ? null : towerAt(x, towerRound());
  };
  /** 这一关的幽灵果冻（没有就是 null）。 */
  const jelly = () => {
    const x = now().jelly;
    return x === null ? null : jellyAt(x, nearJelly());
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
    background: levels[level()].night ? "#0b0f1e" : "#1b2233",
    shaders: SHADERS,
    ...(levels[level()].night ? night(now().goal) : {}),
    // 夜里的压暗在前，bloom 和背景模糊每关都有
    post: [...(levels[level()].night ? night(now().goal).post : []), ...POST],
    audio: { preload: [COIN_SOUND, WIN_SOUND], ...levelAudio(levels[level()].night ?? false) },
    physics: { gravity: [0, -1800] },
    camera: { follow: "player", y: 60, smooth: 0.15, zoom: zoom(), bounds: { left: 0, right: now().right, bottom: -160, top: now().top } },
    camera3d: { match2d: true, interleave: true },
    // 箱子顶上的小箱子在大箱子顶面投下影子；有箱子堆的关再加一盏照着它的聚光灯
    lights: [{ id: "sun", kind: "directional", direction: [-1, -2, -1.5], shadows: true }, ...(now().tower === null ? [] : [towerSpot(now().tower!)]), ...now().lights],
    ambient: { brightness: 0.35 },
    // 3D 箱子摞起来的地面和关卡的地面一样高
    physics3d: { gravity: [0, -1800, 0] },
    models: [now().crate.model, flagOn(now().goal), ...(tower()?.models ?? []), ...(jelly() === null ? [] : [jelly()!.model]), ...now().models],
    emitters3d: now().tower === null ? [] : [campfireAt(now().tower!)],
    bars: [hpBar(falls())],
    bubbles: [...(title() === null ? [] : [titleBubble(levelName(title()!))]), ...nameTags(room.peers())],
    net: netNode(trial === undefined && online.on(), level(), name),
    joints: now().swing.joints,
    tilemaps: now().tilemaps,
    sprites: [
      ...BACKDROP,
      ...(levels[level()].night ? stonesNear(3 * TILE) : []),
      ...remotePlayers(room.peers()),
      { id: "player", x: now().spawn[0], y: now().spawn[1], w: 24, h: 32, z: 5, ...HERO, body: { kind: "character", friction: 0, collide: true, group: PLAYER_GROUP }, ...(flashing() ? FLASH : {}) },
      // 第二个手柄接入时多一个角色，拔掉就去掉
      ...(input.padOf(1) === null ? [] : [guestSprite(now().spawn)]),
      // 金币自己发光，夜里也不暗
      ...now().coins.filter((c) => !taken().includes(c.id)).map(
        (c): SpriteNode => ({ id: c.id, x: c.x, y: c.y, w: 14, z: 2, rotation: 45, color: "#ffd93d", body: { kind: "fixed", sensor: true }, ...COIN_SHADER, unlit: true }),
      ),
      ...now().lifts.map(({ node }): SpriteNode => ({ ...node, z: 1, color: "#c08552", body: { kind: "kinematic", friction: 0 } })),
      // 踩到的不删：去掉刚体、变透明，巡逻补间接着走，重新开始时直接恢复
      ...now().enemies.map(({ id, x, y }): SpriteNode =>
        stomped().includes(id)
          ? { id, x, y, w: 24, h: 24, z: 4, color: "#00000000" }
          : { id, x, y, w: 24, h: 24, z: 4, color: "#9b59b6", body: { kind: "kinematic", friction: 0 } },
      ),
      // 幽灵：看见角色就绕开平台追过来，追远了飘回去（见 docs/ai.md）；踩到的和敌人一样变透明
      ...now().ghosts.map(({ id, x, y }): SpriteNode =>
        stomped().includes(id)
          ? { id, x, y, w: 22, h: 22, z: 4, color: "#00000000" }
          : { id, x, y, w: 22, h: 22, z: 4, color: "#c8d2ffd0", unlit: true, body: { kind: "kinematic", friction: 0 }, chase: GHOST_CHASE },
      ),
      ...now().ramps,
      ...(tower() === null ? [] : [tower()!.trigger]),
      ...(jelly() === null ? [] : [jelly()!.zone]),
      now().crate.body,
      ...now().swing.sprites,
      now().goal,
      ...portalOver(now().goal),
    ],
  });

  const best = () => progress().best[String(level() + 1)];

  return (
    // 让开系统栏和刘海（手机上状态栏盖在最上面；横过来时刘海在左右）
    <View class="flex-1 bg-black" style={{ paddingTop: safeArea().top, paddingLeft: safeArea().left, paddingRight: safeArea().right }}>
      <Text class="text-white p-4">
        {trial !== undefined ? "试玩，按 Esc 回到编辑 · " : platform.desktop ? t("help", { count: progress().unlocked }) : ""}
        {trial !== undefined ? "" : t("slot", { n: slot() + 1 })}
        {trial !== undefined ? "" : brokenSlot() === "newer" ? t("slotNewer") : brokenSlot() ? t("slotBroken") : ""}
        {trial !== undefined ? "" : " · "}
        {t("level", { n: level() + 1, name: levelName(level()) })}
        {" · "}
        {t("coins", { got: taken().length, all: now().coins.length, count: now().coins.length })}
        {" · "}
        {t("falls", { count: falls() })}
        {best() === undefined ? "" : t("best", { seconds: best()! })}
        {won() === null ? "" : t("won", { seconds: won()! })}
        {leader() === null ? "" : t("leader", { ...leader()! })}
        {trial !== undefined ? "" : " · "}
        {trial !== undefined ? "" : netLabel(t, online.on(), room.status(), room.peers().length, room.error())}
      </Text>
      <GameView
        class="flex-1"
        {...pinchZoom}
        ref={(n: NativeComponentRef) => (game = n)}
        scene={scene()}
        actions={input.prop()}
        onPad={input.handlePad}
        onAction={(e: ActionEvent) => {
          input.handle(e);
          if (!e.down || e.player !== 0) return;
          if (e.action === "remap") return remapping() ? closeRemap() : remapping.set(true);
          if (trial !== undefined) return;
          if (e.action === "slot") return pick(nextSlot(target));
          if (e.action === "lang") return chosenLang.set(otherLocale(lang.locale()));
          if (e.action === "net") return online.toggle();
        }}
        onLevelStart={(e) => {
          // 连着选了两次关：前一次的 levelStart 晚到，不算
          if (e.level !== levelId()) return;
          switching = false;
          startTweens();
          clearTimeout(titleEnd);
          title.set(level());
          fetchLeader();
          // 换关时记下打到第几关；坏了的存档位不覆盖
          if (!brokenSlot()) persist();
        }}
        onCollide={(e) => {
          if (switching || (e.a !== "player" && e.b !== "player")) return;
          const other = e.a === "player" ? e.b : e.a;
          // 果冻的感应区进出都算
          if (other === JELLY_ZONE) return nearJelly.set(e.started);
          if (!e.started) return;
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
            submitScore(seconds);
            sound(game, { src: WIN_SOUND, volume: 0.6 });
            // 最后一关之后回到第一关
            next = setTimeout(() => play((level() + 1) % levels.length), NEXT_DELAY);
          }
        }}
        onKey={(e) => {
          // 正在等按键来绑：这次按键拿去绑了
          if (input.handleKey(e)) return;
          if (trial !== undefined) {
            if (e.down && e.code === "Escape") props.onExit?.();
            return;
          }
          // 换存档、语言、联网是动作（onAction）；Digit1 … Digit9：选已经解锁的关
          const n = /^Digit([1-9])$/.exec(e.code)?.[1];
          if (!e.down || e.repeat || n === undefined) return;
          const index = Number(n) - 1;
          if (index < levels.length && index < progress().unlocked) play(index);
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
        onNet={(e) => {
          room.handle(e);
          if (e.kind === "status" && e.status === "online") fetchLeader();
        }}
        onHttp={http.handle}
        onRestart={() => {
          clearTimeout(next);
          next = undefined;
          reset();
        }}
      />
      {platform.desktop ? null : <TouchPad game={() => game} />}
      <Button testID="remap-open" class="absolute rounded bg-white/20 px-3 items-center justify-center active:bg-white/40" style={remapPlace(safeArea()).button} onPress={() => (remapping() ? closeRemap() : remapping.set(true))}>
        <Text class="text-white text-sm">{t("remapOpen")}</Text>
      </Button>
      {remapping() ? <RemapPanel input={input} t={t} onClose={closeRemap} place={remapPlace(safeArea()).panel} /> : null}
    </View>
  );
}

/**
 * 手机上的触屏操作，叠在游戏视图上：左下是摇杆（推多少走多快），右边跳和重来。按动作发，不发键名：改了键也管用
 * （见 docs/input-actions.md、docs/touch-input.md）。
 */
function TouchPad(props: { game: () => NativeComponentRef | undefined }) {
  return (
    <View style={{ pointerEvents: "box-none" }} class="absolute top-0 left-0 right-0 bottom-0">
      <VirtualStick game={props.game} action="move" class="absolute" style={stickPlace(safeArea())} />
      <View class="absolute flex-row justify-end items-end" style={{ pointerEvents: "box-none", ...padPlace(safeArea()) }}>
        <View class="flex-row gap-4 items-end">
          <ActionButton game={props.game} action="restart" label="↺" class="w-12 h-12" />
          <ActionButton game={props.game} action="jump" label={t("jump")} class="w-20 h-20" />
        </View>
      </View>
    </View>
  );
}
