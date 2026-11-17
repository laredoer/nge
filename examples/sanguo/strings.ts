// 长坂坡的文案（见 docs/localization.md）：中文是默认语言，英文一套，跟系统语言。
import { i18n, type Messages } from "../../ts/nge";

export const ZH = {
  title: "长坂坡",
  hero: "白袍将",
  hp: "体力",
  rage: "怒气",
  thrustReady: "突刺 可用",
  thrustWait: "突刺 {seconds} 秒",
  thrustLow: "突刺 怒气不足",
  kills: { other: "斩敌 {count}" },
  stage: "第{n}段 · {name}",
  stage1: "当阳郊野",
  stage2: "粮道",
  stage3: "长坂桥",
  go: "前进 →",
  boss: "虎豹骑将",
  won: "突围成功",
  wonInfo: "斩敌 {count} · 用时 {seconds} 秒",
  lost: "力竭倒下",
  lostInfo: "斩敌 {count} · 打到第{n}段",
  restart: "按 R 重新开始",
  restartTouch: "点一下重新开始",
  attackKey: "攻",
  thrustKey: "刺",
  jumpKey: "跳",
  paused: "暂停 · 按 P 继续",
  help: "←→↑↓ / WASD 移动 · J 攻击（连按三段） · K 突刺 · L 跳（空中按 J 下劈） · P 暂停 · R 重开",
};

export const EN: Messages<typeof ZH> = {
  title: "Changban Slope",
  hero: "White-Robed Spear",
  hp: "HP",
  rage: "Rage",
  thrustReady: "Lunge ready",
  thrustWait: "Lunge in {seconds} s",
  thrustLow: "Lunge: not enough rage",
  kills: { one: "{count} kill", other: "{count} kills" },
  stage: "Stage {n} · {name}",
  stage1: "Outskirts of Dangyang",
  stage2: "Supply Road",
  stage3: "Changban Bridge",
  go: "Go →",
  boss: "Tiger Cavalry Captain",
  won: "Broke Through!",
  wonInfo: "{count} kills · {seconds} s",
  lost: "Fallen",
  lostInfo: "{count} kills · reached stage {n}",
  restart: "Press R to restart",
  restartTouch: "Tap to restart",
  attackKey: "Atk",
  thrustKey: "Lunge",
  jumpKey: "Jump",
  paused: "Paused · press P to resume",
  help: "←→↑↓ / WASD move · J attack (press up to 3 times) · K lunge · L jump (J in the air: plunge) · P pause · R restart",
};

export const MESSAGES = { zh: ZH, en: EN };

export type Key = keyof typeof ZH;

/** 第 index 段（从 0 数）的名字的文案键。 */
export function stageKey(index: number): Key {
  return `stage${index + 1}` as Key;
}

export function strings(system?: () => string) {
  return i18n({ messages: MESSAGES, fallback: "zh", system });
}
