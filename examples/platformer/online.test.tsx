// 平台跳跃的联网演示（见 docs/networking.md）：房间、远端角色、头顶名字、排行榜的地址和解析。
import { expect, test } from "nsh/test";
import { remoteId } from "../../ts/net";
import { boardOf, leaderOf, nameTags, netLabel, netNode, NET_KEY, playerName, remotePlayers, scoresUrl } from "./online";

const bob = { id: 2, name: "bob" };
const carol = { id: 3, name: "carol" };

test("开着联网时每关一个房间，只同步自己的角色；关了就没有 net", () => {
  expect(netNode(true, 0, "甲")).toEqual({ room: "platformer-1", name: "甲", sync: ["player"] });
  expect(netNode(true, 2, "甲")!.room).toBe("platformer-3");
  expect(netNode(false, 0, "甲")).toBe(undefined);
  expect(NET_KEY).toBe("KeyN");
});

test("远端角色半透明、没有刚体，id 是 remoteId；头顶名字跟着它", () => {
  const sprites = remotePlayers([bob, carol]);
  expect(sprites.map((s) => s.id)).toEqual([remoteId(2, "player"), remoteId(3, "player")]);
  for (const s of sprites) {
    expect(s.body).toBe(undefined);
    // 颜色带 alpha，不是全不透明
    expect(s.color!.length).toBe(9);
    expect(s.color!.slice(7)).not.toBe("ff");
  }
  const tags = nameTags([bob]);
  expect(tags.length).toBe(1);
  expect([tags[0].follow, tags[0].text]).toEqual([remoteId(2, "player"), "bob"]);
  expect(tags[0].y!).toBeGreaterThan(0);
  expect(new Set([...nameTags([bob, carol]).map((b) => b.id)]).size).toBe(2);
});

test("HUD：关着、连接中、在线 N 人（算上自己）", () => {
  const t = (key: string, args?: Record<string, unknown>) => `${key}${args ? JSON.stringify(args) : ""}`;
  expect(netLabel(t, false, "online", 3)).toBe("netOff");
  expect(netLabel(t, true, "offline", 0)).toBe("netConnecting");
  expect(netLabel(t, true, "connecting", 0)).toBe("netConnecting");
  expect(netLabel(t, true, "online", 2)).toBe('netOnline{"count":3}');
});

test("名字是随机的，最多 32 个字", () => {
  const a = playerName(() => 0);
  const b = playerName(() => 0.999);
  expect(a).not.toBe(b);
  expect(a.length).toBeLessThan(32);
});

test("排行榜的地址从中继地址推出来，按关分榜", () => {
  expect(boardOf(0)).toBe("platformer-1");
  expect(scoresUrl("ws://127.0.0.1:9100", 1)).toBe("http://127.0.0.1:9100/scores?board=platformer-2");
  expect(scoresUrl(null, 1)).toBe(null);
});

test("榜首：拿不到、格式不对、空榜都是 null，不信对面的数据", () => {
  const ok = (body: string) => ({ ok: true, status: 200, body });
  expect(leaderOf(ok('{"board":"b","top":[{"name":"bob","score":12.3},{"name":"c","score":20}]}'))).toEqual({ name: "bob", seconds: 12.3 });
  expect(leaderOf(ok('{"top":[]}'))).toBe(null);
  expect(leaderOf(ok("{"))).toBe(null);
  expect(leaderOf(ok('{"top":[{"name":1,"score":2}]}'))).toBe(null);
  expect(leaderOf(ok('{"top":[{"name":"x","score":"NaN"}]}'))).toBe(null);
  expect(leaderOf(ok(`{"top":[{"name":"${"x".repeat(100)}","score":1}]}`))).toEqual({ name: "x".repeat(32), seconds: 1 });
  expect(leaderOf({ ok: false, status: 0, body: "", error: "连不上" })).toBe(null);
});
