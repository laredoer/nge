// 3D 空间查询的 TSX 部分（见 docs/gameplay3d.md）：查询变成 `query3d` command，结果按 id 交回给对应的 Promise。
import { expect, test } from "nsh/test";
import { queries3d } from "../../ts/physics3d";

function fakeGame() {
  const sent: any[] = [];
  return { sent, view: { command: (name: string, args: unknown) => sent.push([name, args]) } as any };
}

test("各种查询变成 query3d command，没写的字段不发", () => {
  const g = fakeGame();
  const q = queries3d(() => g.view);
  q.ray([0, 100, 0], [0, -1, 0], 500, { exclude: "hero" });
  q.overlap({ ball: 80 }, [1, 2, 3], { only: [0, 2] });
  q.closest([0, 0, 0]);
  expect(g.sent).toEqual([
    ["query3d", { id: 1, kind: "ray", from: [0, 100, 0], dir: [0, -1, 0], distance: 500, exclude: "hero" }],
    ["query3d", { id: 2, kind: "overlap", shape: { ball: 80 }, from: [1, 2, 3], only: [0, 2] }],
    ["query3d", { id: 3, kind: "closest", from: [0, 0, 0] }],
  ]);
  expect(q.pending()).toBe(3);
});

test("结果按 id 交回：射线取第一个，重叠给全部，不认识的、重复的不乱交", async () => {
  const g = fakeGame();
  const q = queries3d(() => g.view);
  const ray = q.ray([0, 0, 0], [1, 0, 0]);
  const near = q.overlap({ box: [1, 1, 1] }, [0, 0, 0]);
  q.handle({ id: 2, hits: [{ id: "a" }, { id: null }] });
  q.handle({ id: 99, hits: [{ id: "x" }] });
  q.handle({ id: 1, hits: [] });
  q.handle({ id: 1, hits: [{ id: "late" }] });
  expect(await ray).toBe(undefined);
  expect(await near).toEqual([{ id: "a" }, { id: null }]);
  expect(q.pending()).toBe(0);
});

test("引擎不回时按空结果结束；视图没建好直接是空的", async () => {
  const g = fakeGame();
  const q = queries3d(() => g.view, { waitMs: 5 });
  expect(await q.point([0, 0, 0])).toEqual([]);
  expect(q.pending()).toBe(0);
  expect(await queries3d(() => undefined).ray([0, 0, 0], [0, 1, 0])).toBe(undefined);
});
