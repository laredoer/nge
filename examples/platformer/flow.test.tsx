// 预加载、换关过场的 TSX 部分和平台跳跃的预制体（见 docs/scene-flow.md）；展开、过场本身在 Rust 那边测。
import { expect, test } from "nsh/test";
import { preloader, transitions, type PreloadEvent } from "../../ts/nge";
import { GROVE, GROVES, levelTransition, TEXTURES, TREE } from "./prefabs";

function fakeGame() {
  const sent: any[] = [];
  return { sent, view: { command: (name: string, args: unknown) => sent.push([name, args]) } as any };
}

const event = (id: string, done: number, total: number, failed: PreloadEvent["failed"] = []): PreloadEvent => ({ id, done, total, failed, finished: done === total });

test("load 发 preload 命令，进度进 signal，全部完成时 ready", async () => {
  const g = fakeGame();
  const loads = preloader(() => g.view);
  const a = loads.load(["asset:/@g/a.png", "asset:/@g/b.wav"]);
  const b = loads.load(["asset:/@g/c.png"]);
  expect(g.sent).toEqual([
    ["preload", { id: a.id, uris: ["asset:/@g/a.png", "asset:/@g/b.wav"] }],
    ["preload", { id: b.id, uris: ["asset:/@g/c.png"] }],
  ]);
  expect(a.id).not.toBe(b.id);
  expect(a.progress()).toBe(undefined);
  loads.handle(event(a.id, 1, 2));
  expect(a.progress()!.done).toBe(1);
  expect(b.progress()).toBe(undefined);
  expect(loads.pending()).toBe(2);
  const failed = [{ uri: "asset:/@g/b.wav", error: "没有这个文件" }];
  loads.handle(event(a.id, 2, 2, failed));
  expect((await a.ready).failed).toEqual(failed);
  expect(loads.pending()).toBe(1);
  loads.handle(event("preload-from-someone-else", 1, 1));
  loads.handle(event(a.id, 1, 2));
  expect(a.progress()!.done).toBe(2);
  loads.reset();
});

test("视图还没建好时直接完成", async () => {
  const loads = preloader(() => undefined);
  const l = loads.load(["asset:/@g/a.png"]);
  expect(await l.ready).toEqual({ id: l.id, done: 0, total: 0, failed: [], finished: true });
  expect(loads.pending()).toBe(0);
});

test("过场：盖上、揭开时 busy，揭开时留着载入进度，揭完不 busy", () => {
  const fade = transitions();
  expect(fade.busy()).toBe(false);
  expect(fade.phase()).toBe(undefined);
  fade.handle({ phase: "out", level: "2", done: 0, total: 3, failed: [] });
  expect(fade.busy()).toBe(true);
  fade.handle({ phase: "out", level: "2", done: 3, total: 3, failed: [] });
  expect(fade.progress()).toEqual({ done: 3, total: 3 });
  fade.handle({ phase: "in", level: "2", done: 0, total: 0, failed: [] });
  expect(fade.busy()).toBe(true);
  expect(fade.progress()).toEqual({ done: 3, total: 3 });
  fade.handle({ phase: "done", level: "2", done: 0, total: 0, failed: [] });
  expect([fade.busy(), fade.phase()]).toEqual([false, "done"]);
});

test("引擎一直不回、视图重建：按没回结束，不一直挂着", async () => {
  const g = fakeGame();
  const loads = preloader(() => g.view, { waitMs: 20 });
  const a = loads.load(["asset:/@g/a.png", "asset:/@g/b.png"]);
  loads.handle(event(a.id, 1, 2, [{ uri: "asset:/@g/b.png", error: "坏了" }]));
  const done = await a.ready;
  expect([done.finished, done.done, done.total]).toEqual([true, 2, 2]);
  expect(done.failed.map((f) => f.uri)).toEqual(["asset:/@g/b.png", "asset:/@g/a.png"]);
  expect(done.failed[0].error).toBe("坏了");
  const b = loads.load(["asset:/@g/c.png"]);
  loads.reset();
  expect((await b.ready).failed.length).toBe(1);
  expect(loads.pending()).toBe(0);
});

test("进度条的比例只涨不退：载了 JSON 文件后总数变大也不倒回去", () => {
  const g = fakeGame();
  const loads = preloader(() => g.view);
  const a = loads.load(["asset:/@g/level.json"]);
  expect(a.fraction()).toBe(0);
  loads.handle(event(a.id, 1, 2));
  expect(a.fraction()).toBe(0.5);
  loads.handle(event(a.id, 1, 5));
  expect(a.fraction()).toBe(0.5);
  loads.handle(event(a.id, 5, 5));
  expect(a.fraction()).toBe(1);
  loads.reset();
});

test("开场的揭开、没东西要等的过场不算「载入中」", () => {
  const fade = transitions();
  fade.handle({ phase: "in", level: "1", done: 0, total: 0, failed: [] });
  expect([fade.busy(), fade.loading()]).toEqual([true, false]);
  fade.handle({ phase: "done", level: "1", done: 0, total: 0, failed: [] });
  fade.handle({ phase: "out", level: "2", done: 1, total: 4, failed: [] });
  expect(fade.loading()).toBe(true);
  fade.handle({ phase: "in", level: "2", done: 0, total: 0, failed: [] });
  expect(fade.loading()).toBe(true);
  fade.handle({ phase: "done", level: "2", done: 0, total: 0, failed: [] });
  expect(fade.loading()).toBe(false);
});

test("树丛摆出来和以前一棵棵摆的 24 棵树一样", () => {
  const before = Array.from({ length: 24 }, (_, i) => `${-600 + i * 110 + (i % 4) * 17},${90 + (i % 2) * 14}`);
  const crownY = (inst: { set?: Record<string, Record<string, unknown>> }) => (inst.set?.crown?.y as number | undefined) ?? 90;
  const after = GROVES.flatMap((g) => GROVE.instances!.map((t) => `${g.x! + (t.x ?? 0)},${crownY(t)}`));
  expect(after).toEqual(before);
  expect(GROVE.instances!.every((t) => t.prefab === TREE)).toBe(true);
  expect(new Set(GROVES.map((g) => g.id)).size).toBe(12);
});

test("两种过场轮着用，过场里先载好音效；主角图集和瓦片集不要 mipmap", () => {
  expect([0, 1, 2].map((i) => levelTransition(i, []).kind)).toEqual(["fade", "wipe", "fade"]);
  expect(levelTransition(0, ["asset:/@platformer/coin.wav"]).preload).toEqual(["asset:/@platformer/coin.wav"]);
  expect(levelTransition(0, []).min!).toBeGreaterThan(levelTransition(0, []).out!);
  expect(TEXTURES["asset:/@platformer/hero.png"]).toEqual({ mipmaps: false });
  expect(TEXTURES["asset:/@platformer/tiles.png"]).toEqual({ mipmaps: false });
});
