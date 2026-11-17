// 平台跳跃的联网演示（见 docs/networking.md）：每关一个房间，别人的角色半透明、头顶写名字；排行榜走中继同一台服务器的 HTTP。
import { signal, Storage } from "nsh";
import type { BubbleNode, HttpResult, NetNode, NetPeer, NetStatus, SpriteNode } from "../../ts/nge";
import { json, relayHttp, remoteId } from "../../ts/nge";
import { TILE } from "./levels";

/** 按 N 开关联网。 */
export const NET_KEY = "KeyN";
/** 同步出去的精灵：自己的角色。 */
const SYNCED = "player";
/** 名字最多几个字（和中继一样）。 */
const MAX_NAME = 32;

/** 第 `level` 关（从 0 数）的房间名和排行榜名。 */
export function boardOf(level: number): string {
  return `platformer-${level + 1}`;
}

/** 场景的 `net`：关着、还没读完开关时没有。地址不写，原生用环境变量 `NGE_RELAY`，默认 ws://127.0.0.1:9100。 */
export function netNode(on: boolean | null, level: number, name: string): NetNode | undefined {
  return on === true ? { room: boardOf(level), name, sync: [SYNCED] } : undefined;
}

/** 存开关的地方（nsh 的 `Storage` 就是）。 */
export interface NetStore {
  getItem(key: string): Promise<string | null>;
  setItem(key: string, value: string): Promise<void>;
}

const NET_STORAGE_KEY = "nge:net";

/** 联网开关，记在设备上（默认开）。读完存档之前是 `null`：免得存的是关、却先连上了中继。存的不是布尔值时按默认。 */
export function netSetting(store: NetStore = Storage) {
  const on = signal<boolean | null>(null);
  let changed = false;
  const parse = (raw: string | null) => {
    try {
      const v: unknown = raw === null ? null : JSON.parse(raw);
      return typeof v === "boolean" ? v : true;
    } catch {
      return true;
    }
  };
  store.getItem(NET_STORAGE_KEY).then(
    (raw) => {
      if (!changed) on.set(parse(raw));
    },
    () => {
      if (!changed) on.set(true);
    },
  );
  return {
    on: () => on(),
    toggle() {
      changed = true;
      const next = on() === false;
      on.set(next);
      store.setItem(NET_STORAGE_KEY, JSON.stringify(next)).catch((e) => console.warn(`写存档 ${NET_STORAGE_KEY} 失败: ${e}`));
    },
  };
}

/** 随机的名字：两个窗口一眼分得出来。 */
export function playerName(random: () => number = Math.random): string {
  return `P${100 + Math.floor(random() * 900)}`;
}

/** 别人的角色：半透明、没有刚体（不挡路、不触发碰撞），位置由引擎按插值写。 */
export function remotePlayers(peers: NetPeer[]): SpriteNode[] {
  return peers.map((p) => ({ id: remoteId(p.id, SYNCED), x: 3 * TILE, y: 16, w: 24, h: 32, z: 4, color: "#5dade280" }));
}

/** 别人头顶的名字。 */
export function nameTags(peers: NetPeer[]): BubbleNode[] {
  return peers.map((p) => ({ id: `name-${p.id}`, text: p.name, follow: remoteId(p.id, SYNCED), y: 22, size: 11, back: "#00000080" }));
}

/** HUD 上的联网状态：关着、连接中（连不上时一直在重试）、房间满了、在线 N 人（算上自己）。 */
export function netLabel(t: (key: "netOff" | "netConnecting" | "netOnline" | "netFull", args?: { count: number }) => string, on: boolean | null, status: NetStatus, peers: number): string {
  if (on !== true) return t("netOff");
  if (status === "full") return t("netFull");
  return status === "online" ? t("netOnline", { count: peers + 1 }) : t("netConnecting");
}

/** 这关排行榜的地址；不知道中继在哪时是 null。 */
export function scoresUrl(relay: string | null, level: number): string | null {
  const base = relayHttp(relay);
  return base === null ? null : `${base}/scores?board=${encodeURIComponent(boardOf(level))}`;
}

/** 交成绩的地址。 */
export function submitUrl(relay: string | null): string | null {
  const base = relayHttp(relay);
  return base === null ? null : `${base}/scores`;
}

export interface Leader {
  name: string;
  seconds: number;
}

/** 从排行榜的响应里取榜首；对面给的东西都不信：类型不对、不是有限数的都不要，名字截短。 */
export function leaderOf(result: HttpResult): Leader | null {
  const top = json<{ top?: unknown }>(result)?.top;
  const first = Array.isArray(top) ? top[0] : undefined;
  if (typeof first !== "object" || first === null) return null;
  const { name, score } = first as { name?: unknown; score?: unknown };
  if (typeof name !== "string" || typeof score !== "number" || !Number.isFinite(score) || score < 0) return null;
  return { name: [...name].slice(0, MAX_NAME).join(""), seconds: score };
}
