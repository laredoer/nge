// 联网（见 docs/networking.md）：场景的 `net` 节点、`net` 事件、`netSend`，以及从事件维护房间状态的 `netRoom()`。
import { signal, type NativeComponentRef } from "nsh";

/** 场景的 `net`：写了就连中继，去掉就断，`url` / `room` / `name` 变了重连。 */
export interface NetNode {
  /** 中继地址，不写时原生用环境变量 `NGE_RELAY`，再没有就是 "ws://127.0.0.1:9100"。 */
  url?: string;
  /** 同一个房间的人互相看得见。 */
  room: string;
  /** 自己的名字（最多 32 个字），默认 "player"。 */
  name?: string;
  /** 把这些精灵的位置广播出去（对方按 `remoteId(你, 精灵 id)` 声明它们）。 */
  sync?: string[];
  /** 每秒发几次，默认 20。 */
  rate?: number;
  /** 远端显示的延迟（毫秒），默认 100：越大越平滑、越落后。 */
  delay?: number;
  /** 房间的模式：`relay`（默认，各自广播自己）或 `authority`（服务端跑模拟，只发输入，见 docs/net-more.md）。 */
  mode?: "relay" | "authority";
  /** 鉴权的凭据（`nge-token` 签的整串）；不写时原生用环境变量 `NGE_TOKEN`。中继要鉴权时没有它连不上。 */
  token?: string;
  /** 权威模式里自己的角色的精灵 id：位置由引擎预测，别人的角色用 `remoteId(成员, "hero")` 声明。 */
  predict?: string;
}

/** 权威模式的竞技场几何（`status` 事件带着，照着画平台）。 */
export interface ArenaLayout {
  width: number;
  platforms: { x0: number; x1: number; y: number }[];
  /** 角色的宽高。 */
  hero: [number, number];
}

/** 权威模式的统计：每秒两次。 */
export interface NetStats {
  /** 心跳量到的往返（毫秒），还没量到是 null。 */
  rtt: number | null;
  rollbacks: number;
  resets: number;
  /** 最近一次回滚的预测误差（point）。 */
  error: number;
  max_error: number;
  /** 超前服务端几个 tick。 */
  lead: number;
  tick: number;
  /** 自己被拍过几次。 */
  tagged: number;
  dropped: number;
}

/** 权威房间里远端角色的 key。 */
export const ARENA_KEY = "hero";

/** `full`：房间满了，引擎过一会儿（30 秒）再试。 */
export type NetStatus = "offline" | "connecting" | "online" | "full";

export interface NetPeer {
  id: number;
  name: string;
}

/** `net` 事件。`status` 带着在连的中继地址和当时的成员列表；断线时先给每个人发 `leave`。 */
export type NetEvent =
  | { kind: "status"; status: NetStatus; url: string | null; me: number | null; peers: NetPeer[]; arena?: ArenaLayout }
  | { kind: "join"; peer: NetPeer }
  | { kind: "leave"; peer: NetPeer }
  | { kind: "message"; from: number; type: string; data: unknown }
  | { kind: "ranked"; board: string; rank: number | null }
  | ({ kind: "stats" } & NetStats)
  | { kind: "error"; message: string };

/** 远端成员 `peer` 同步过来的精灵 `key` 的 id：场景里声明它，位置由引擎按插值写。 */
export function remoteId(peer: number, key: string): string {
  return `net:${peer}:${key}`;
}

/** 中继地址（`ws://` / `wss://`）对应的同一台服务器的 HTTP 地址（不带路径）；不是中继地址时是 null。 */
export function relayHttp(url: string | null): string | null {
  const m = /^(wss?):\/\/([^/?#]+)/i.exec(url ?? "");
  return m === null ? null : `${m[1].toLowerCase() === "wss" ? "https" : "http"}://${m[2]}`;
}

/** 发一条消息给房间里的人（`to` 指定时只给他）。每秒最多 30 条，数据编成 JSON 后最多 4 KB，超了丢掉。 */
export function netSend(game: NativeComponentRef | undefined, type: string, data?: unknown, to?: number) {
  game?.command("netSend", to === undefined ? { type, data } : { type, data, to });
}

/** 交成绩：走联机连接，记在这条连接的身份名下（鉴权时是 token 里的账号和名字）。结果是 `ranked` 事件。 */
export function netScore(game: NativeComponentRef | undefined, board: string, score: number) {
  game?.command("netScore", { board, score });
}

export interface NetRoom {
  status(): NetStatus;
  /** 在连的中继地址；断开时是 null。 */
  url(): string | null;
  /** 自己的成员 id；不在线时是 null。 */
  me(): number | null;
  /** 房间里的其他人（按 id 排）。 */
  peers(): NetPeer[];
  /** 最近一次断线 / 报错的原因。 */
  error(): string | null;
  /** 权威房间的几何；不在权威房间里时是 null。 */
  arena(): ArenaLayout | null;
  /** 最近的统计；不在权威房间里时是 null。 */
  stats(): NetStats | null;
  /** 接 `<GameView onNet={room.handle}>`。 */
  handle(e: NetEvent): void;
  /** 订阅某种消息，返回取消订阅的函数。 */
  onMessage(type: string, fn: (data: unknown, from: NetPeer) => void): () => void;
}

/** 从 `net` 事件维护的房间状态（signal）。 */
export function netRoom(): NetRoom {
  const status = signal<NetStatus>("offline");
  const url = signal<string | null>(null);
  const me = signal<number | null>(null);
  const peers = signal<NetPeer[]>([]);
  const error = signal<string | null>(null);
  const arena = signal<ArenaLayout | null>(null);
  const stats = signal<NetStats | null>(null);
  const listeners = new Map<string, Set<(data: unknown, from: NetPeer) => void>>();
  const sorted = (list: NetPeer[]) => [...list].sort((a, b) => a.id - b.id);
  return {
    status: () => status(),
    url: () => url(),
    me: () => me(),
    peers: () => peers(),
    error: () => error(),
    arena: () => arena(),
    stats: () => stats(),
    handle(e) {
      switch (e.kind) {
        case "status":
          status.set(e.status);
          url.set(e.url);
          me.set(e.me);
          peers.set(e.status === "online" ? sorted(e.peers.filter((p) => p.id !== e.me)) : []);
          if (e.status === "online") error.set(null);
          arena.set(e.status === "online" ? (e.arena ?? null) : null);
          if (e.status !== "online") stats.set(null);
          return;
        case "join":
          if (e.peer.id !== me() && !peers().some((p) => p.id === e.peer.id)) peers.set(sorted([...peers(), e.peer]));
          return;
        case "leave":
          peers.set(peers().filter((p) => p.id !== e.peer.id));
          return;
        case "message": {
          const from = peers().find((p) => p.id === e.from);
          if (from === undefined) return;
          for (const fn of listeners.get(e.type) ?? []) fn(e.data, from);
          return;
        }
        case "stats": {
          const { kind: _, ...rest } = e;
          if (arena() !== null) stats.set(rest);
          return;
        }
        case "ranked":
          return;
        case "error":
          error.set(e.message);
          return;
      }
    },
    onMessage(type, fn) {
      const set = listeners.get(type) ?? new Set();
      set.add(fn);
      listeners.set(type, set);
      return () => set.delete(fn);
    },
  };
}
