// 波次推进和镜头：纯函数。
import { clamp, VIEW_HALF } from "./belt";
import { WAVES, WORLD, type Spawn } from "./levels";

/** 锁定区（镜头和玩家都出不去）。 */
export interface Lock {
  left: number;
  right: number;
}

export interface WaveState {
  /** 下一波是第几波（WAVES 的下标）。 */
  next: number;
  /** 正在打的是第几波；没在打是 null。 */
  active: number | null;
  /** 这一波开打多久了。 */
  t: number;
  /** 这一波还没放出来的兵。 */
  pending: Spawn[];
  lock: Lock | null;
  /** 打完了几波。 */
  cleared: number;
  /** 全部打完。 */
  done: boolean;
}

/** 镜头比玩家往前看多少。 */
export const LEAD = 60;
/** 锁定区的中心在触发点右边多少。 */
const LOCK_AHEAD = 120;
/** 镜头追上目标的快慢（每秒的指数衰减率）。 */
const CAMERA_RATE = 5;
/** 玩家离镜头边多近就走不过去。 */
const EDGE = 24;
/** 兵在锁定区外多远出来。 */
const SPAWN_OUT = 60;

export function newWaves(): WaveState {
  return { next: 0, active: null, t: 0, pending: [], lock: null, cleared: 0, done: false };
}

/** 在 at 触发的那一波的锁定区：一屏宽，往前看一点，不出世界。 */
export function lockOf(at: number): Lock {
  const center = clamp(at + LOCK_AHEAD, WORLD.left + VIEW_HALF, WORLD.right - VIEW_HALF);
  return { left: center - VIEW_HALF, right: center + VIEW_HALF };
}

/** 兵出来的 x：锁定区左边或右边外面一点。 */
export function spawnX(s: Spawn, lock: Lock): number {
  return s.side === "right" ? lock.right + SPAWN_OUT : lock.left - SPAWN_OUT;
}

/**
 * 推进一步：玩家走到下一波的触发点就开打（锁镜头）；按 delay 放兵；兵都放完、场上没有活着的敌人（`alive`）就解锁。
 * 返回这一步要放出来的兵。
 */
export function stepWaves(w: WaveState, playerX: number, alive: number, dt: number): { waves: WaveState; spawns: Spawn[] } {
  if (w.active === null) {
    if (w.done || w.next >= WAVES.length || playerX < WAVES[w.next].at) return { waves: w, spawns: [] };
    const wave = WAVES[w.next];
    w = { ...w, active: w.next, next: w.next + 1, t: 0, pending: wave.spawns, lock: lockOf(wave.at) };
  } else {
    w = { ...w, t: w.t + dt };
  }
  const spawns = w.pending.filter((s) => s.delay <= w.t);
  const pending = w.pending.filter((s) => s.delay > w.t);
  // 这一步刚放出来的兵还没算进 alive
  if (pending.length === 0 && spawns.length === 0 && alive === 0) {
    const cleared = w.cleared + 1;
    return { waves: { ...w, active: null, pending, lock: null, cleared, done: w.next >= WAVES.length }, spawns };
  }
  return { waves: { ...w, pending }, spawns };
}

/** 镜头想去哪：锁住时对准锁定区中间；不然在玩家前面一点，只往右不往回走。都不出世界。 */
export function cameraTarget(playerX: number, cam: number, lock: Lock | null): number {
  const target = lock ? (lock.left + lock.right) / 2 : Math.max(cam, playerX + LEAD);
  return clamp(target, WORLD.left + VIEW_HALF, WORLD.right - VIEW_HALF);
}

/** 镜头平滑地追过去。 */
export function followCamera(cam: number, target: number, dt: number): number {
  return cam + (target - cam) * (1 - Math.exp(-dt * CAMERA_RATE));
}

/** 玩家能走的 x 范围：不出镜头（锁住时不出锁定区），不出世界。 */
export function playerRange(cam: number, lock: Lock | null): { minX: number; maxX: number } {
  const left = Math.max(WORLD.left, cam - VIEW_HALF, lock?.left ?? -Infinity) + EDGE;
  const right = Math.min(WORLD.right, lock ? lock.right : Infinity) - EDGE;
  return { minX: left, maxX: right };
}
