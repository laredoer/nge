// 一局游戏：把玩家、敌人、箭、波次、镜头串起来，按固定步长推进。纯函数：返回新状态和这一步要放的音效、震屏等。
import { VIEW_HALF } from "./belt";
import { hitstopFor, resolveEnemyAttacks, resolvePlayerAttack } from "./combat";
import { isAttacking, newEnemy, stepArrow, stepEnemy, type Arrow, type Enemy } from "./enemies";
import { blocked, stageAt, type EnemyKind } from "./levels";
import { newPlayer, stepPlayer, type Input, type Player } from "./player";
import { cameraTarget, followCamera, newWaves, playerRange, spawnX, stepWaves, type WaveState } from "./waves";

export type Phase = "play" | "won" | "lost";
export type FxName = "slash1" | "slash2" | "slash3" | "thrust" | "shock" | "spark" | "smoke";
export type SoundName = "swing" | "hit" | "arrow" | "hooves" | "win" | "lose";

/** 一次性的画面特效（刀光、火星、烟），画完就删。 */
export interface Fx {
  id: string;
  name: FxName;
  x: number;
  d: number;
  h: number;
  face: 1 | -1;
  t: number;
  dur: number;
}

export type GameEvent =
  | { kind: "sound"; name: SoundName; x?: number }
  | { kind: "shake"; strength: number; duration: number }
  | { kind: "burst"; x: number; d: number; h: number; smoke: boolean }
  | { kind: "warn"; x: number; d: number }
  | { kind: "kill"; boss: boolean };

export interface Game {
  phase: Phase;
  /** 打了多久（不算顿帧）。 */
  time: number;
  /** 赢了 / 倒下以后过了多久（到了才出画面）。 */
  endTimer: number;
  player: Player;
  enemies: Enemy[];
  arrows: Arrow[];
  fx: Fx[];
  waves: WaveState;
  /** 镜头的 x。 */
  cam: number;
  kills: number;
  /** 顿帧还剩多久：这段时间整局停住。 */
  hitstop: number;
  nextId: number;
  /** 现在是第几段战场，什么时候进来的（段名横幅用）。 */
  stage: number;
  stageAt: number;
  /** 上一波清完的时间（「前进」提示用）。 */
  clearedAt: number | null;
}

/** 赢了、倒下以后多久出结果画面。 */
export const END_DELAY = { won: 1.2, lost: 1.5 };
const FX_DUR: Record<FxName, number> = { slash1: 0.12, slash2: 0.14, slash3: 0.18, thrust: 0.2, shock: 0.25, spark: 0.18, smoke: 0.6 };

export const START = { x: 120, d: 65 };

export function newGame(): Game {
  return {
    phase: "play", time: 0, endTimer: 0, player: newPlayer(START.x, START.d), enemies: [], arrows: [], fx: [], waves: newWaves(),
    cam: cameraTarget(START.x, 0, null), kills: 0, hitstop: 0, nextId: 1, stage: 0, stageAt: 0, clearedAt: null,
  };
}

export function bossOf(g: Game): Enemy | null {
  return g.enemies.find((e) => e.kind === "boss") ?? null;
}

/** 推进一步。 */
export function step(prev: Game, input: Input, dt: number): { game: Game; events: GameEvent[] } {
  const events: GameEvent[] = [];
  let g: Game = { ...prev, fx: ageFx(prev.fx, dt) };
  if (g.phase !== "play") return { game: g, events };
  if (g.hitstop > 0) return { game: { ...g, hitstop: Math.max(0, g.hitstop - dt) }, events };
  g.time += dt;
  let id = g.nextId;
  const fx = (name: FxName, x: number, d: number, h: number, face: 1 | -1 = 1) => {
    g.fx = [...g.fx, { id: `fx-${id++}`, name, x, d, h, face, t: 0, dur: FX_DUR[name] }];
  };

  // 玩家
  const area = { ...playerRange(g.cam, g.waves.lock), blocked: (x: number, d: number) => blocked(x, d) };
  const moved = stepPlayer(g.player, input, dt, area);
  let player = moved.player;
  for (const e of moved.events) {
    if (e === "swing") {
      events.push({ kind: "sound", name: "swing", x: player.x });
      fx(`slash${player.step}` as FxName, player.x + player.face * 46, player.d, player.h + 52, player.face);
    } else if (e === "thrust") {
      events.push({ kind: "sound", name: "swing", x: player.x });
      fx("thrust", player.x + player.face * 60, player.d, 52, player.face);
    } else if (e === "land" && player.act === "land") {
      fx("shock", player.x, player.d, 0);
      events.push({ kind: "shake", strength: 5, duration: 0.2 });
    }
  }

  // 玩家打敌人、打箭
  const attack = resolvePlayerAttack(player, g.enemies, g.arrows);
  player = attack.player;
  let enemies = attack.enemies;
  let arrows = attack.arrows;
  for (const h of attack.hits) {
    events.push({ kind: "sound", name: "hit", x: h.x });
    events.push({ kind: "shake", strength: h.launch || h.kind === "plunge" ? 6 : 3, duration: 0.15 });
    fx("spark", h.x, h.d, h.h + 50);
    g.hitstop = Math.max(g.hitstop, hitstopFor(h.kind, h.launch));
    if (h.killed) {
      g.kills += 1;
      events.push({ kind: "kill", boss: h.boss });
    }
  }
  for (const a of attack.parried) {
    events.push({ kind: "sound", name: "hit", x: a.x });
    fx("spark", a.x, a.d, a.h);
  }

  // 敌人
  const view = { minX: g.cam - VIEW_HALF, maxX: g.cam + VIEW_HALF };
  const target = { x: player.x, d: player.d, h: player.h, down: player.act === "down" || player.act === "dead" };
  enemies = enemies.map((e) => {
    const attackers = enemies.filter((o) => o !== e && isAttacking(o)).length;
    const r = stepEnemy(e, { player: target, attackers, bounds: view, blocked }, dt);
    for (const ev of r.events) {
      if (ev.kind === "arrow") {
        arrows = [...arrows, { ...ev.arrow, id: `arrow-${id++}` }];
        events.push({ kind: "sound", name: "arrow", x: e.x });
      } else if (ev.kind === "warn") {
        events.push({ kind: "warn", x: e.x, d: e.d });
      } else if (ev.kind === "hooves") {
        events.push({ kind: "sound", name: "hooves", x: e.x });
      } else if (ev.kind === "gone") {
        fx("smoke", r.enemy.x, r.enemy.d, 0);
        events.push({ kind: "burst", x: r.enemy.x, d: r.enemy.d, h: 20, smoke: true });
      }
    }
    return r.enemy;
  });
  arrows = arrows.map((a) => stepArrow(a, dt)).filter((a) => Math.abs(a.x - g.cam) < VIEW_HALF + 200);

  // 敌人打玩家
  const struck = resolveEnemyAttacks(player, enemies, arrows);
  player = struck.player;
  arrows = struck.arrows;
  if (struck.hurt) {
    events.push({ kind: "sound", name: "hit", x: player.x });
    events.push({ kind: "shake", strength: struck.hurt.heavy ? 12 : 5, duration: struck.hurt.heavy ? 0.4 : 0.2 });
    fx("spark", player.x, player.d, player.h + 50);
    g.hitstop = Math.max(g.hitstop, struck.hurt.heavy ? 0.12 : 0.05);
  }
  enemies = enemies.filter((e) => !e.gone);

  // 波次：放兵、解锁
  const before = g.waves.cleared;
  const waved = stepWaves(g.waves, player.x, enemies.filter((e) => !e.dead).length, dt);
  g.waves = waved.waves;
  for (const s of waved.spawns) {
    enemies = [...enemies, spawn(s.kind, spawnX(s, g.waves.lock!), s.d, id)];
    id += 1;
  }
  if (g.waves.cleared > before && !g.waves.done) g.clearedAt = g.time;

  // 镜头和段名
  g.cam = followCamera(g.cam, cameraTarget(player.x, g.cam, g.waves.lock), dt);
  const stage = stageAt(player.x);
  if (stage > g.stage) {
    g.stage = stage;
    g.stageAt = g.time;
  }

  g = { ...g, player, enemies, arrows, nextId: id };
  return { game: settle(g, dt, events), events };
}

function spawn(kind: EnemyKind, x: number, d: number, id: number): Enemy {
  return newEnemy(`${kind}-${id}`, kind, x, d, id);
}

function ageFx(fx: Fx[], dt: number): Fx[] {
  return fx.map((f) => ({ ...f, t: f.t + dt })).filter((f) => f.t < f.dur);
}

/** 胜负：倒下了过一会儿算输；全部波次打完、敌人都化成烟了过一会儿算赢。 */
function settle(g: Game, dt: number, events: GameEvent[]): Game {
  const lost = g.player.act === "dead";
  const won = !lost && g.waves.done && g.enemies.length === 0;
  if (!lost && !won) return g;
  const endTimer = g.endTimer + dt;
  if (endTimer < (lost ? END_DELAY.lost : END_DELAY.won)) return { ...g, endTimer };
  events.push({ kind: "sound", name: lost ? "lose" : "win" });
  return { ...g, endTimer, phase: lost ? "lost" : "won" };
}
