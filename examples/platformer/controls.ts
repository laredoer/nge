// 平台跳跃的操作（见 docs/input-actions.md）：默认绑定在 controls.json 里，Rust 的角色控制读的是同一份。
import type { Bindings, Slot, SpriteNode } from "../../ts/nge";
import controls from "./controls.json";

/** 两个玩家的默认绑定：第一个玩家用键盘和第一个手柄，第二个用 F / H / T 和第二个手柄。 */
export const DEFAULT_CONTROLS = controls.players as Bindings[];

/** 第二个角色的精灵 id（Rust 那边按这个名字找）。 */
export const GUEST = "player2";
/** 两个角色的碰撞分组：同一组的互相穿过，不会把对方堵在窄路上、踩在头上。 */
export const PLAYER_GROUP = 1;

/** 键名在面板上显示的样子：去掉 Key / Digit / Gamepad 前缀，方向键换成箭头。 */
export function keyLabel(code: string): string {
  const arrows: Record<string, string> = { ArrowLeft: "←", ArrowRight: "→", ArrowUp: "↑", ArrowDown: "↓" };
  if (arrows[code] !== undefined) return arrows[code];
  if (code.startsWith("Gamepad")) return `Pad ${code.slice(7)}`;
  return code.replace(/^(Key|Digit)/, "");
}

/** 位置的 testID：`bind-<动作>-<部分>-<第几个>`（面板只改第一个玩家的）。 */
export function slotId(slot: Slot): string {
  return `bind-${slot.action}-${slot.part}-${slot.index}`;
}

/** 第二个角色：声明在起点右边一点，Rust 一见到它就挪到第一个角色身边；没有 `collide`，金币、终点不认它。 */
export function guestSprite(spawn: [number, number]): SpriteNode {
  return { id: GUEST, x: spawn[0] + 36, y: spawn[1], w: 24, h: 32, z: 5, color: "#4fc3f7", body: { kind: "character", friction: 0, group: PLAYER_GROUP } };
}
