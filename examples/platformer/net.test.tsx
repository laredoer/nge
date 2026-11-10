// 联网的 TSX 部分（见 docs/networking.md）：从 `net` 事件维护的房间状态、远端精灵的 id。
import { expect, test } from "nsh/test";
import { netRoom, netSend, relayHttp, remoteId, type NetEvent } from "../../ts/net";

const bob = { id: 2, name: "bob" };
const carol = { id: 3, name: "carol" };
const online: NetEvent = { kind: "status", status: "online", url: "ws://relay:9100", me: 1, peers: [carol, bob] };

test("进了房间：状态、自己、按 id 排的成员", () => {
  const room = netRoom();
  expect(room.status()).toBe("offline");
  room.handle({ kind: "status", status: "connecting", url: "ws://relay:9100", me: null, peers: [] });
  expect(room.status()).toBe("connecting");
  room.handle(online);
  expect(room.status()).toBe("online");
  expect(room.me()).toBe(1);
  expect(room.peers()).toEqual([bob, carol]);
  expect(room.url()).toBe("ws://relay:9100");
  room.handle({ kind: "status", status: "offline", url: null, me: null, peers: [] });
  expect(room.url()).toBe(null);
});

test("进出房间：重复的 join 只算一次，自己不算", () => {
  const room = netRoom();
  room.handle({ kind: "status", status: "online", url: "ws://relay:9100", me: 1, peers: [] });
  room.handle({ kind: "join", peer: bob });
  room.handle({ kind: "join", peer: bob });
  room.handle({ kind: "join", peer: { id: 1, name: "我" } });
  expect(room.peers()).toEqual([bob]);
  room.handle({ kind: "leave", peer: bob });
  expect(room.peers()).toEqual([]);
});

test("断线：成员清空，记下原因，连回来时清掉原因", () => {
  const room = netRoom();
  room.handle(online);
  room.handle({ kind: "error", message: "连接断了" });
  room.handle({ kind: "status", status: "connecting", url: "ws://relay:9100", me: null, peers: [] });
  expect(room.peers()).toEqual([]);
  expect(room.me()).toBe(null);
  expect(room.error()).toBe("连接断了");
  room.handle(online);
  expect(room.error()).toBe(null);
});

test("消息按类型分给订阅的，不在房间里的人发的不要", () => {
  const room = netRoom();
  room.handle(online);
  const got: unknown[] = [];
  const off = room.onMessage("wave", (data, from) => got.push([data, from.name]));
  room.handle({ kind: "message", from: 2, type: "wave", data: { n: 1 } });
  room.handle({ kind: "message", from: 2, type: "other", data: 0 });
  room.handle({ kind: "message", from: 9, type: "wave", data: 0 });
  expect(got).toEqual([[{ n: 1 }, "bob"]]);
  off();
  room.handle({ kind: "message", from: 2, type: "wave", data: 2 });
  expect(got.length).toBe(1);
});

test("远端精灵的 id 和发消息的 command", () => {
  expect(remoteId(12, "player")).toBe("net:12:player");
  const sent: [string, unknown][] = [];
  const game = { command: (name: string, args: unknown) => sent.push([name, args]) } as any;
  netSend(game, "wave", { n: 1 });
  netSend(game, "poke", undefined, 2);
  netSend(undefined, "lost");
  expect(sent).toEqual([
    ["netSend", { type: "wave", data: { n: 1 } }],
    ["netSend", { type: "poke", data: undefined, to: 2 }],
  ]);
});

test("中继地址换成同一台服务器的 HTTP 地址", () => {
  expect(relayHttp("ws://127.0.0.1:9100")).toBe("http://127.0.0.1:9100");
  expect(relayHttp("wss://relay.example/room?x=1")).toBe("https://relay.example");
  expect(relayHttp("WS://Host:1/")).toBe("http://Host:1");
  for (const bad of [null, "", "http://x", "ws://", "ftp://x"]) expect(relayHttp(bad)).toBe(null);
});
