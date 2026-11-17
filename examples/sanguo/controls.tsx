// 长坂坡的触屏操作和结束画面（docs/touch-input.md）。
// 摇杆、按钮、结束画面都有状态（按着的手指、出现的时间）：显示与否先用 computed 算成布尔，
// 每帧变的 HUD 对象不会让它们每帧重建（见 ts/touch.tsx 的 VirtualStick）。
import { computed, type NativeComponentRef, platform, safeArea, Text, View } from "nsh";
import { ActionButton, VirtualStick } from "../../ts/nge";
import type { Hud } from "./hud";
import { strings } from "./strings";
import { ATTACK_REPEAT, END_GUARD_MS } from "./touch";

const t = strings().t;

/** 触屏设备上、没结束时显示 `TouchControls`。 */
export function TouchLayer(props: { hud: () => Hud; game: () => NativeComponentRef | undefined; cooldown: () => number }) {
  const show = computed(() => !platform.desktop && props.hud().end === null);
  return <View style={{ pointerEvents: "box-none" }} class="absolute top-0 left-0 right-0 bottom-0">{show() ? <TouchControls game={props.game} cooldown={props.cooldown} dim={() => !props.hud().rageFull} /> : null}</View>;
}

/**
 * 触屏设备上的操作：左半边浮动摇杆（八方向）驱动 `move`，右下攻击（按住连发）、突刺（显示冷却，怒气不够时变暗）、跳。
 * 都按动作发给引擎，引擎发回 `action` 事件。
 */
export function TouchControls(props: { game: () => NativeComponentRef | undefined; cooldown: () => number; dim: () => boolean }) {
  return (
    <View style={{ pointerEvents: "box-none" }} class="absolute top-0 left-0 right-0 bottom-0">
      <VirtualStick game={props.game} action="move" directions="8" class="absolute" style={{ left: safeArea().left, bottom: safeArea().bottom, width: "45%", height: "60%" }} />
      <View class="absolute flex-row items-end gap-4" style={{ pointerEvents: "box-none", right: 24 + safeArea().right, bottom: 24 + safeArea().bottom }}>
        <ActionButton game={props.game} action="thrust" label={t("thrustKey")} cooldown={props.cooldown} dim={props.dim} class="w-14 h-14" />
        <ActionButton game={props.game} action="jump" label={t("jumpKey")} class="w-16 h-16" />
        <ActionButton game={props.game} action="attack" label={t("attackKey")} repeat={ATTACK_REPEAT} repeatDelay={ATTACK_REPEAT} class="w-20 h-20" />
      </View>
    </View>
  );
}

/** 结束画面：有 `end` 时显示，点一下（桌面上点一下鼠标）重开。 */
export function EndScreen(props: { end: () => Hud["end"]; onRestart: () => void; now?: () => number }) {
  const shown = computed(() => props.end() !== null);
  return <View style={{ pointerEvents: "box-none" }} class="absolute top-0 left-0 right-0 bottom-0">{shown() ? <EndOverlay end={props.end} onRestart={props.onRestart} now={props.now ?? (() => performance.now())} /> : null}</View>;
}

function EndOverlay(props: { end: () => Hud["end"]; onRestart: () => void; now: () => number }) {
  const since = props.now();
  /** 这次按下的时间：出现之前就按着的手指没有按下，不算。 */
  let downAt: number | undefined;
  const e = () => props.end()!;
  return (
    <View
      testID="end"
      class={`absolute top-0 left-0 right-0 bottom-0 items-center justify-center gap-3 ${e().won ? "bg-amber-900/50" : "bg-black/60"}`}
      onPressDown={() => (downAt = props.now())}
      onPress={() => {
        // 刚出现时还在连按攻击的手指不算：出现 0.8 秒以后按下的才重开
        if (downAt !== undefined && downAt - since >= END_GUARD_MS) props.onRestart();
        downAt = undefined;
      }}
    >
      <Text class={`text-4xl font-bold ${e().won ? "text-amber-200" : "text-red-300"}`}>{e().title}</Text>
      <Text class="text-white text-lg">{e().info}</Text>
      <Text class="text-white/80 text-base">{platform.desktop ? e().restart : t("restartTouch")}</Text>
    </View>
  );
}
