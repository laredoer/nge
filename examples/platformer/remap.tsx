// 改键面板（见 docs/input-actions.md）：列出跳跃的键，点一个再按任意键换掉它；冲突时先问；能恢复默认。
import { Text, View } from "nsh";
import { Button } from "nsh/base";
import type { InputMap, Slot } from "../../ts/nge";
import { keyLabel, slotId } from "./controls";
import type { Key } from "./strings";

type T = (key: Key, params?: Record<string, string | number>) => string;

/** 面板里能改的动作。 */
const ACTION = "jump";

function Btn(p: { id: string; label: string; onPress: () => void; selected?: boolean }) {
  return (
    <Button testID={p.id} class="px-2 py-1 rounded bg-neutral-700 hover:bg-neutral-600 active:bg-neutral-500 selected:bg-sky-700" selected={p.selected} onPress={p.onPress}>
      <Text class="text-white text-sm">{p.label}</Text>
    </Button>
  );
}

/** 冲突的位置是哪个动作的：用文案里的动作名，没有就用动作的 id。 */
function actionName(t: T, slot: Slot): string {
  const names: Record<string, Key> = { jump: "jump", move: "actionMove", restart: "actionRestart", slot: "actionSlot", lang: "actionLang", net: "actionNet", remap: "actionRemap", lab: "actionLab" };
  const name = names[slot.action];
  return (name === undefined ? slot.action : t(name)) + (slot.player === 0 ? "" : ` (P${slot.player + 1})`);
}

/** 改键按钮和面板放哪：都在右上角，面板在按钮下面，让开系统栏。 */
export function remapPlace(safe: { top: number; right: number; bottom: number; left: number }) {
  const button = { top: safe.top + 40, right: 16 + safe.right, height: 28 };
  return { button, panel: { top: button.top + button.height + 8, right: button.right } };
}

/** 面板下面那行提示：在等按键、有冲突（能换 / 换不了）、存不上，或者平时的说明。 */
export function remapStatus(input: InputMap, t: T): string {
  const pending = input.pending();
  if (pending !== null && pending.blocked.length > 0) return t("remapBlocked", { key: keyLabel(pending.code), action: pending.blocked.map((c) => actionName(t, c)).join(" / ") });
  if (pending !== null) return t("remapConflict", { key: keyLabel(pending.code), action: pending.conflicts.map((c) => actionName(t, c)).join(" / ") });
  if (input.capturing() !== null) return t("remapPress");
  return input.saveError() === null ? t("remapHint") : t("remapSaveFailed");
}

export function RemapPanel(props: { input: InputMap; t: T; onClose: () => void; place: { top: number; right: number } }) {
  const { input, t } = props;
  const same = (a: Slot | null, b: Slot) => a !== null && a.player === b.player && a.action === b.action && a.part === b.part && a.index === b.index;
  /** 加一个键的位置：排在最后一个后面。 */
  const fresh = (): Slot => ({ player: 0, action: ACTION, part: "keys", index: input.slots(0, ACTION).length });
  return (
    <View testID="remap" class="absolute w-80 rounded-lg bg-neutral-900/90 p-3 gap-2" style={props.place}>
      <Text class="text-white text-base">{t("remapTitle")}</Text>
      <View class="flex-row flex-wrap items-center gap-1">
        <Text class="text-white text-sm pr-1">{t("jump")}</Text>
        {input.slots(0, ACTION).map((slot) => (
          <Btn id={slotId(slot)} label={keyLabel(input.key(slot) ?? "")} selected={same(input.capturing(), slot)} onPress={() => input.capture(slot)} />
        ))}
        <Btn id="bind-add" label="+" selected={same(input.capturing(), fresh())} onPress={() => input.capture(fresh())} />
      </View>
      <Text testID="remap-status" class="text-neutral-300 text-sm">{remapStatus(input, t)}</Text>
      {input.pending() === null ? null : (
        <View class="flex-row gap-1">
          {input.pending()!.blocked.length > 0 ? null : <Btn id="remap-swap" label={t("remapSwap")} onPress={() => void input.confirm()} />}
          <Btn id="remap-cancel" label={t("remapCancel")} onPress={input.cancel} />
        </View>
      )}
      <View class="flex-row gap-1">
        <Btn id="remap-reset" label={t("remapReset")} onPress={input.reset} />
        <Btn id="remap-close" label={t("remapClose")} onPress={props.onClose} />
      </View>
    </View>
  );
}
