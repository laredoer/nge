// 平台跳跃的文案（见 docs/localization.md）：中文是默认语言，英文按复数写。
import { i18n, type Messages } from "../../ts/nge";

export const ZH = {
  help: "← → / A D / 手柄 移动 · 空格 / ↑ / A 跳 · R 重来 · 数字键选关（解锁了 {count} 关）· S 换存档 · L 语言 · N 联网 · K 改键 · Q 调试 · P 性能 · ",
  slot: "存档 {n}",
  slotBroken: "（坏了）",
  slotNewer: "（新版本存的，不覆盖）",
  level: "第 {n} 关 {name}",
  coins: "金币 {got} / {all}",
  falls: { other: "失手 {count} 次" },
  best: " · 最好 {seconds} 秒",
  won: " · 通关！用时 {seconds} 秒",
  stomp: "踩！",
  netOff: "离线",
  netConnecting: "连接中…",
  netFailed: "连不上中继，稍后重试",
  netFull: "房间满了，稍后再试",
  netOnline: { other: "在线 {count} 人" },
  leader: " · 榜首 {name} {seconds} 秒",
  jump: "跳",
  actionMove: "移动",
  actionRestart: "重来",
  actionSlot: "换存档",
  actionLang: "语言",
  actionNet: "联网",
  actionRemap: "改键",
  remapOpen: "改键",
  remapTitle: "改键",
  remapHint: "点一个键，再按要换成的键",
  remapPress: "按任意键（Esc 取消）…",
  remapConflict: "{key} 已经用在「{action}」上",
  remapSwap: "换过来",
  remapCancel: "取消",
  remapReset: "恢复默认",
  remapClose: "关闭",
  level1: "草原",
  level2: "石阶",
  level3: "云端",
};

export const EN: Messages<typeof ZH> = {
  help: "← → / A D / pad move · Space / ↑ / A jump · R restart · digits pick a level ({count} unlocked) · S save slot · L language · N online · K keys · Q debug · P perf · ",
  slot: "Save {n}",
  slotBroken: " (broken)",
  slotNewer: " (from a newer version, kept)",
  level: "Level {n} {name}",
  coins: { one: "{got} / {all} coin", other: "{got} / {all} coins" },
  falls: { one: "{count} fall", other: "{count} falls" },
  best: " · best {seconds} s",
  won: " · cleared in {seconds} s!",
  stomp: "Stomp!",
  netOff: "offline",
  netConnecting: "connecting…",
  netFailed: "can't reach the relay, retrying",
  netFull: "room full, retrying later",
  netOnline: { one: "online, {count} player", other: "online, {count} players" },
  leader: " · top {name} {seconds} s",
  jump: "Jump",
  actionMove: "Move",
  actionRestart: "Restart",
  actionSlot: "Save slot",
  actionLang: "Language",
  actionNet: "Online",
  actionRemap: "Keys",
  remapOpen: "Keys",
  remapTitle: "Key bindings",
  remapHint: "Pick a key, then press the new one",
  remapPress: "Press any key (Esc cancels)…",
  remapConflict: "{key} is already used for \"{action}\"",
  remapSwap: "Swap",
  remapCancel: "Cancel",
  remapReset: "Reset to defaults",
  remapClose: "Close",
  level1: "Meadow",
  level2: "Steps",
  level3: "Clouds",
};

export const MESSAGES = { zh: ZH, en: EN };

export type Key = keyof typeof ZH;

/** 第 `index` 关（从 0 数）的名字的文案键。 */
export function levelKey(index: number): Key {
  return `level${index + 1}` as Key;
}

/** L 键：中文和英文之间切。 */
export function otherLocale(current: string): string {
  return current === "zh" ? "en" : "zh";
}

export function strings(system?: () => string) {
  return i18n({ messages: MESSAGES, fallback: "zh", system });
}
