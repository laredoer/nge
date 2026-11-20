// 权威联机竞技场的纯函数（见 docs/net-more.md）。
import { expect, test } from "nsh/test";
import { ARENA_HERO, ARENA_ROOM, arenaNode, arenaScene, arenaStats } from "./arena";

const layout = { width: 960, platforms: [{ x0: 160, x1: 352, y: 120 }], hero: [24, 32] as [number, number] };

test("竞技场的 net：权威模式、预测自己的角色，token 有才带", () => {
  expect(arenaNode("P123")).toEqual({ room: ARENA_ROOM, name: "P123", mode: "authority", predict: ARENA_HERO });
  expect(arenaNode("P123", "v1.a.b.c").token).toBe("v1.a.b.c");
});

test("竞技场的场景：平台按服务端给的几何画，别人的角色用 hero 这个 key", () => {
  const scene = arenaScene(layout, [{ id: 7, name: "bob" }], arenaNode("me"));
  const ids = (scene.sprites ?? []).map((s) => s.id);
  expect(ids).toEqual(["arena-floor", "arena-platform-0", "net:7:hero", ARENA_HERO]);
  const platform = scene.sprites!.find((s) => s.id === "arena-platform-0")!;
  expect([platform.x, platform.y, platform.w]).toEqual([256, 116, 192]);
  expect(scene.bubbles![0].follow).toBe("net:7:hero");
  expect(scene.net!.mode).toBe("authority");
  // 还没连上：只有地面和自己，不写 net 时不连
  const offline = arenaScene(null, [], undefined);
  expect((offline.sprites ?? []).map((s) => s.id)).toEqual(["arena-floor", ARENA_HERO]);
  expect(offline.net).toBe(undefined);
});

test("HUD 的统计：没有时是空的，误差保留一位", () => {
  const t = (_: string, a: { rtt: string; rollbacks: number; error: string; tagged: number }) => `${a.rtt}|${a.rollbacks}|${a.error}|${a.tagged}`;
  expect(arenaStats(t, null)).toBe("");
  const stats = { rtt: 41.6, rollbacks: 3, resets: 0, error: 2.345, max_error: 5, lead: 5, tick: 9, tagged: 1, dropped: 0 };
  expect(arenaStats(t, stats)).toBe("42|3|2.3|1");
  expect(arenaStats(t, { ...stats, rtt: null })).toBe("?|3|2.3|1");
});
