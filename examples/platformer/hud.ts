// 平台跳跃里世界中的 UI（见 docs/world-ui.md）：角色头顶的血条、每关开始时的关卡名气泡。
import type { BarNode, BubbleNode } from "../../ts/nge";

/** 失手几次整关重来。 */
export const HEARTS = 3;
/** 关卡名打完后留多久（毫秒）。 */
export const TITLE_HOLD_MS = 1500;

/** 角色头顶的血条：失手一次少一格。 */
export function hpBar(falls: number): BarNode {
  const value = Math.max(0, HEARTS - falls) / HEARTS;
  return { id: "hp", follow: "player", y: 24, w: 30, h: 4, value, color: value > 0.34 ? "#4ade80" : "#ef4444", smooth: 0.25 };
}

/** 失手这么多次就整关重来。 */
export function outOfHearts(falls: number): boolean {
  return falls >= HEARTS;
}

/** 关卡名气泡：在血条上面，一个字一个字打出来。 */
export function titleBubble(name: string): BubbleNode {
  return { id: "title", text: name, follow: "player", y: 32, typing: 10, size: 13 };
}

/** 手机上触屏按钮离屏幕边的距离：原来的边距（下 32、左右 24）加上安全区（系统栏、刘海）。 */
export function padPlace(safe: { top: number; right: number; bottom: number; left: number }): { bottom: number; left: number; right: number } {
  return { bottom: 32 + safe.bottom, left: 24 + safe.left, right: 24 + safe.right };
}
