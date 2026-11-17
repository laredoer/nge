// HUD 上显示什么（纯函数）：左上血条和怒气，右上斩敌数，首领出场时下方的首领血条，段名横幅，胜负画面。
import { bossOf, type Game } from "./game";
import { PLAYER_HP, RAGE_MAX, THRUST } from "./player";
import { stageKey, type Key } from "./strings";

export type T = (key: Key, params?: Record<string, string | number>) => string;

/** 段名横幅、「前进」提示显示多久（秒）。 */
export const BANNER_SECONDS = 2.5;

export interface Hud {
  hp: number;
  rage: number;
  /** 怒气够突刺了（怒气条换颜色）。 */
  rageFull: boolean;
  thrust: string;
  kills: string;
  boss: { name: string; hp: number } | null;
  banner: string | null;
  end: { title: string; info: string; restart: string; won: boolean } | null;
  paused: string | null;
}

export function hudOf(g: Game, paused: boolean, t: T): Hud {
  const p = g.player;
  const boss = bossOf(g);
  const thrust = p.cooldown > 0 ? t("thrustWait", { seconds: Math.ceil(p.cooldown * 10) / 10 }) : p.rage >= THRUST.cost ? t("thrustReady") : t("thrustLow");
  let banner: string | null = null;
  if (g.time - g.stageAt < BANNER_SECONDS) banner = t("stage", { n: g.stage + 1, name: t(stageKey(g.stage)) });
  else if (g.clearedAt !== null && g.waves.active === null && g.time - g.clearedAt < BANNER_SECONDS) banner = t("go");
  const seconds = Math.round(g.time);
  const end =
    g.phase === "won"
      ? { title: t("won"), info: t("wonInfo", { count: g.kills, seconds }), restart: t("restart"), won: true }
      : g.phase === "lost"
        ? { title: t("lost"), info: t("lostInfo", { count: g.kills, n: g.stage + 1 }), restart: t("restart"), won: false }
        : null;
  return {
    hp: p.hp / PLAYER_HP,
    rage: p.rage / RAGE_MAX,
    rageFull: p.rage >= THRUST.cost,
    thrust,
    kills: t("kills", { count: g.kills }),
    boss: boss && !boss.gone ? { name: t("boss"), hp: boss.hp / boss.maxHp } : null,
    banner: end ? null : banner,
    end,
    paused: paused && !end ? t("paused") : null,
  };
}
