// 平台跳跃的操作和改键面板（见 docs/input-actions.md）。
import { expect, open, test } from "nsh/test";
import { conflictsOf, inputMap, slotsOf, withKey, type InputStore } from "../../ts/nge";
import { DEFAULT_CONTROLS, guestSprite, GUEST, keyLabel, slotId } from "./controls";
import { NET_KEY } from "./online";
import { remapStatus } from "./remap";
import { strings } from "./strings";

test("默认绑定：两个玩家，键盘的键互不冲突", () => {
  expect(DEFAULT_CONTROLS).toHaveLength(2);
  expect(DEFAULT_CONTROLS[0].jump).toEqual({ kind: "button", keys: ["Space", "ArrowUp", "KeyW", "GamepadA"] });
  for (const [player, bindings] of DEFAULT_CONTROLS.entries()) {
    for (const action of Object.keys(bindings)) {
      for (const slot of slotsOf(DEFAULT_CONTROLS, player, action)) {
        const part = (bindings[action] as unknown as Record<string, string[]>)[slot.part];
        expect(conflictsOf(DEFAULT_CONTROLS, slot, part[slot.index])).toEqual([]);
      }
    }
  }
});

test("键名显示得短一点", () => {
  expect(["Space", "KeyW", "Digit1", "ArrowUp", "ArrowLeft", "GamepadA", "GamepadLeftStickLeft"].map(keyLabel)).toEqual(["Space", "W", "1", "↑", "←", "Pad A", "Pad LeftStickLeft"]);
  expect(slotId({ player: 0, action: "jump", part: "keys", index: 2 })).toBe("bind-jump-keys-2");
});

test("角色控制要的动作都有，联网还是 N", () => {
  for (const action of ["move", "jump", "restart", "slot", "lang", "net", "remap"]) expect(action in DEFAULT_CONTROLS[0]).toBe(true);
  expect(Object.keys(DEFAULT_CONTROLS[1]).sort()).toEqual(["jump", "move"]);
  expect(DEFAULT_CONTROLS[0].net).toEqual({ kind: "button", keys: [NET_KEY] });
});

test("第二个角色出现在起点右边，金币和终点不认它", () => {
  const guest = guestSprite([64, 48]);
  expect(guest).toMatchObject({ id: GUEST, x: 100, y: 48, body: { kind: "character" } });
  expect(guest.body?.collide).toBeUndefined();
});

const memory = (): InputStore => ({ getItem: async () => null, setItem: async () => {} });

test("面板的提示：平时、等按键、有冲突", () => {
  const t = strings(() => "zh-CN").t;
  const input = inputMap({ key: "t", defaults: DEFAULT_CONTROLS, store: memory() });
  expect(remapStatus(input, t)).toBe("点一个键，再按要换成的键");
  input.capture({ player: 0, action: "jump", part: "keys", index: 0 });
  expect(remapStatus(input, t)).toBe("按任意键（Esc 取消）…");
  input.handleKey({ code: "KeyR", down: true, repeat: false });
  expect(remapStatus(input, t)).toBe("R 已经用在「重来」上");
  input.cancel();
  // 第二个玩家的键也算冲突，标出是谁的
  input.capture({ player: 0, action: "jump", part: "keys", index: 0 });
  input.handleKey({ code: "KeyT", down: true, repeat: false });
  expect(remapStatus(input, t)).toBe("T 已经用在「跳 (P2)」上");
});

const JUMP_CHIPS = ["id:bind-jump-keys-0", "id:bind-jump-keys-1", "id:bind-jump-keys-2", "id:bind-jump-keys-3"];

test("改键面板：点按钮打开，列出跳跃的键，点一个等按键", async () => {
  const app = await open(".", { platform: "macos", locale: "zh-CN" });
  expect(app.count("id:remap")).toBe(0);
  await app.press("id:remap-open");
  expect(app.count("id:remap")).toBe(1);
  for (const chip of JUMP_CHIPS) expect(app.count(chip)).toBe(1);
  expect(app.count("id:bind-jump-keys-4")).toBe(0);
  for (const label of ["Space", "↑", "W", "Pad A", "点一个键，再按要换成的键"]) expect(app.texts()).toContain(label);
  expect(app.count("id:remap-swap")).toBe(0);
  await app.press("id:bind-jump-keys-0");
  expect(app.texts()).toContain("按任意键（Esc 取消）…");
  await app.press("id:remap-close");
  expect(app.count("id:remap")).toBe(0);
  // 关掉就不等了
  await app.press("id:remap-open");
  expect(app.texts()).toContain("点一个键，再按要换成的键");
});

test("改过的键存在设备上：重开后面板里是改过的，恢复默认后存的也是默认的", async () => {
  const players = withKey(DEFAULT_CONTROLS, { player: 0, action: "jump", part: "keys", index: 0 }, "KeyX");
  const app = await open(".", { platform: "macos", locale: "zh-CN", storage: { "nge:input:platformer": JSON.stringify({ v: 1, players }) } });
  await app.press("id:remap-open");
  await app.waitUntil(() => app.texts().includes("X"), { timeout: 1000 });
  expect(app.texts()).not.toContain("Space");
  await app.press("id:remap-reset");
  expect(app.texts()).toContain("Space");
  await app.waitUntil(() => JSON.parse(app.storage()["nge:input:platformer"]).players[0].jump.keys[0] === "Space", { timeout: 1000 });
});

test("手机上也有改键按钮", async () => {
  const app = await open(".", { platform: "ios", locale: "en" });
  await app.press("id:remap-open");
  expect(app.texts()).toContain("Key bindings");
});
