// 判定：玩家打敌人、打掉箭；敌人的刀、冲锋、箭打玩家。只比带子坐标里的盒子。
import { canBeHit, enemyHitbox, hitEnemy, ARROW, type Arrow, type Enemy } from "./enemies";
import { gainRage, hurtPlayer, isInvulnerable, playerHitbox, type Hitbox, type Player } from "./player";

/** 每打中一个敌人攒多少怒气。 */
export const RAGE_PER_HIT = 6;

/** 人身上挨打的范围：左右半宽、纵深半厚、身高。 */
const BODY = { x: 14, d: 8, h: 80 };
const BOSS_BODY = { x: 34, d: 10, h: 110 };

interface Box {
  x0: number;
  x1: number;
  d0: number;
  d1: number;
  h0: number;
  h1: number;
}

/** 盒子碰到站在 (x, d)、离地 h 的人吗。 */
function touches(box: Box, x: number, d: number, h: number, body = BODY): boolean {
  return box.x0 < x + body.x && box.x1 > x - body.x && box.d0 < d + body.d && box.d1 > d - body.d && box.h0 < h + body.h && box.h1 > h;
}

export interface HitResult {
  id: string;
  x: number;
  d: number;
  h: number;
  kind: Hitbox["kind"];
  launch: boolean;
  killed: boolean;
  boss: boolean;
}

export function resolvePlayerAttack(player: Player, enemies: Enemy[], arrows: Arrow[]): { player: Player; enemies: Enemy[]; arrows: Arrow[]; hits: HitResult[]; parried: Arrow[] } {
  const box = playerHitbox(player);
  if (box === null) return { player, enemies, arrows, hits: [], parried: [] };
  const hits: HitResult[] = [];
  const next = enemies.map((e) => {
    if (!canBeHit(e) || e.hitBy === box.swing || !touches(box, e.x, e.d, e.h, e.kind === "boss" ? BOSS_BODY : BODY)) return e;
    const hit = hitEnemy(e, { damage: box.damage, launch: box.launch, fromX: player.x, swing: box.swing });
    hits.push({ id: e.id, x: e.x, d: e.d, h: e.h, kind: box.kind, launch: box.launch && e.kind !== "boss", killed: hit.dead, boss: e.kind === "boss" });
    return hit;
  });
  const parried = arrows.filter((a) => touches(box, a.x, a.d, a.h - 10, { x: 10, d: 6, h: 20 }));
  return {
    player: gainRage(player, hits.length * RAGE_PER_HIT),
    enemies: next,
    arrows: arrows.filter((a) => !parried.includes(a)),
    hits,
    parried,
  };
}

export function resolveEnemyAttacks(player: Player, enemies: Enemy[], arrows: Arrow[]): { player: Player; arrows: Arrow[]; hurt: { damage: number; heavy: boolean; x: number } | null } {
  let p = player;
  let hurt: { damage: number; heavy: boolean; x: number } | null = null;
  for (const e of enemies) {
    const box = enemyHitbox(e);
    if (box === null || isInvulnerable(p) || !touches(box, p.x, p.d, p.h)) continue;
    p = hurtPlayer(p, box.damage, e.x, box.heavy);
    hurt = { damage: box.damage, heavy: box.heavy, x: p.x };
  }
  const left: Arrow[] = [];
  for (const a of arrows) {
    const struck = !isInvulnerable(p) && Math.abs(a.x - p.x) < 16 && Math.abs(a.d - p.d) < 12 && a.h >= p.h && a.h <= p.h + 75;
    if (!struck) {
      left.push(a);
      continue;
    }
    p = hurtPlayer(p, ARROW.damage, a.x - a.vx, false);
    hurt = { damage: ARROW.damage, heavy: false, x: p.x };
  }
  return { player: p, arrows: left, hurt };
}

/** 打中时顿几秒：挑飞、下劈重，突刺一路穿过去轻。 */
export function hitstopFor(kind: Hitbox["kind"], launch: boolean): number {
  if (kind === "thrust") return 0.035;
  if (kind === "plunge") return 0.09;
  return launch ? 0.1 : 0.06;
}
