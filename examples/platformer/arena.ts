// 权威联机竞技场（按 M 进出，桌面上，见 docs/net-more.md）：服务端跑模拟，自己的角色由引擎预测、收到快照对账回滚，
// 别人的角色按快照插值；HUD 显示延迟、回滚次数、预测误差。平台的几何从 `net` 事件里来，和服务端算的是同一份。
import type { ArenaLayout, BubbleNode, NetNode, NetPeer, NetStats, Scene, SpriteNode } from "../../ts/nge";
import { ARENA_KEY, remoteId } from "../../ts/nge";

/** 进出竞技场的键。 */
export const ARENA_KEY_CODE = "KeyM";
/** 场景的 `level`：进出都整个重建。 */
export const ARENA_LEVEL = "arena";
/** 自己的角色的精灵 id。 */
export const ARENA_HERO = "hero";
/** 所有人进同一个房间。 */
export const ARENA_ROOM = "platformer-arena";

/** 场景的 `net`：权威模式；`token` 没给时原生读环境变量 `NGE_TOKEN`。 */
export function arenaNode(name: string, token?: string): NetNode {
  return { room: ARENA_ROOM, name, mode: "authority", predict: ARENA_HERO, ...(token === undefined ? {} : { token }) };
}

/** 竞技场的场景：地面、平台（服务端给的几何，还没连上时空着）、自己和别人的角色、别人头顶的名字。 */
export function arenaScene(layout: ArenaLayout | null, peers: NetPeer[], net: NetNode | undefined): Scene {
  const width = layout?.width ?? 960;
  const [w, h] = layout?.hero ?? [24, 32];
  const floor: SpriteNode = { id: "arena-floor", x: width / 2, y: -8, w: width, h: 16, color: "#3d5a80" };
  const platforms = (layout?.platforms ?? []).map((p, i): SpriteNode => ({ id: `arena-platform-${i}`, x: (p.x0 + p.x1) / 2, y: p.y - 4, w: p.x1 - p.x0, h: 8, color: "#98c1d9" }));
  const others = peers.map((p): SpriteNode => ({ id: remoteId(p.id, ARENA_KEY), x: width / 2, y: h / 2, w, h, z: 4, color: "#5dade2c0" }));
  const tags = peers.map((p): BubbleNode => ({ id: `arena-name-${p.id}`, text: p.name, follow: remoteId(p.id, ARENA_KEY), y: h * 0.7, size: 11, back: "#00000080" }));
  return {
    level: ARENA_LEVEL,
    background: "#1b2233",
    camera: { x: width / 2, y: 160 },
    ...(net === undefined ? {} : { net }),
    bubbles: tags,
    sprites: [floor, ...platforms, ...others, { id: ARENA_HERO, x: width / 2, y: h / 2, w, h, z: 5, color: "#ee6c4d" }],
  };
}

type T = (key: "arenaStats", args: { rtt: string; rollbacks: number; error: string; tagged: number }) => string;

/** HUD 上的统计：延迟、回滚次数、最近一次的预测误差、被拍几次；还没有统计时是空的。 */
export function arenaStats(t: T, stats: NetStats | null): string {
  if (stats === null) return "";
  return t("arenaStats", { rtt: stats.rtt === null ? "?" : String(Math.round(stats.rtt)), rollbacks: stats.rollbacks, error: stats.error.toFixed(1), tagged: stats.tagged });
}
