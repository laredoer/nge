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
}

export type NetStatus = "offline" | "connecting" | "online";

export interface NetPeer {
  id: number;
  name: string;
}

/** `net` 事件。`status` 带着在连的中继地址和当时的成员列表；断线时先给每个人发 `leave`。 */
export type NetEvent =
  | { kind: "status"; status: NetStatus; url: string | null; me: number | null; peers: NetPeer[] }
  | { kind: "join"; peer: NetPeer }
  | { kind: "leave"; peer: NetPeer }
  | { kind: "message"; from: number; type: string; data: unknown }
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
  const listeners = new Map<string, Set<(data: unknown, from: NetPeer) => void>>();
  const sorted = (list: NetPeer[]) => [...list].sort((a, b) => a.id - b.id);
  return {
    status: () => status(),
    url: () => url(),
    me: () => me(),
    peers: () => peers(),
    error: () => error(),
    handle(e) {
      switch (e.kind) {
        case "status":
          status.set(e.status);
          url.set(e.url);
          me.set(e.me);
          peers.set(e.status === "online" ? sorted(e.peers.filter((p) => p.id !== e.me)) : []);
          if (e.status === "online") error.set(null);
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
