// HTTP 的 TSX 部分（见 docs/networking.md）：请求变成 `http` command，结果按 id 交回给对应的 Promise。
import { expect, test } from "nsh/test";
import { httpClient, json } from "../../ts/http";

function fakeGame() {
  const sent: any[] = [];
  return { sent, view: { command: (name: string, args: unknown) => sent.push([name, args]) } as any };
}

test("GET、POST 变成 http command，默认 text/plain，非字符串的体编成 JSON", () => {
  const g = fakeGame();
  const http = httpClient(() => g.view);
  http.get("http://x/scores?board=1", { timeout: 3 });
  http.post("http://x/scores", { board: "1", score: 2 }, { headers: { "X-A": "b" } });
  expect(g.sent).toEqual([
    ["http", { id: 1, method: "GET", url: "http://x/scores?board=1", headers: {}, body: undefined, timeout: 3 }],
    ["http", { id: 2, method: "POST", url: "http://x/scores", headers: { "Content-Type": "text/plain", "X-A": "b" }, body: '{"board":"1","score":2}', timeout: undefined }],
  ]);
  expect(http.pending()).toBe(2);
});

test("结果按 id 交回，晚到的、不认识的、重复的不乱交", async () => {
  const g = fakeGame();
  const http = httpClient(() => g.view);
  const a = http.get("http://x/a");
  const b = http.get("http://x/b");
  http.handle({ id: 2, ok: true, status: 200, body: "B", error: null });
  http.handle({ id: 99, ok: true, status: 200, body: "?", error: null });
  http.handle({ id: 1, ok: false, status: 0, body: "", error: "连不上" });
  http.handle({ id: 1, ok: true, status: 200, body: "again", error: null });
  expect(await b).toEqual({ ok: true, status: 200, body: "B" });
  expect(await a).toEqual({ ok: false, status: 0, body: "", error: "连不上" });
  expect(http.pending()).toBe(0);
});

test("游戏视图还没建好时直接出错", async () => {
  const http = httpClient(() => undefined);
  const r = await http.get("http://x/");
  expect(r.ok).toBe(false);
  expect(r.error).toBe("游戏视图还没建好");
  expect(http.pending()).toBe(0);
});

test("json() 只解 2xx 的合法 JSON", () => {
  expect(json<{ a: number }>({ ok: true, status: 200, body: '{"a":1}' })).toEqual({ a: 1 });
  expect(json({ ok: true, status: 200, body: "{" })).toBe(undefined);
  expect(json({ ok: false, status: 500, body: '{"a":1}' })).toBe(undefined);
});
