// 下载 wasm 的进度（本仓库加的 `counted`）：`node --test vendor/nsh-platform-web/web/load-progress.test.mjs`。

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// node 的 globalThis 不是 EventTarget：借一个，`counted` 发的事件才收得到
if (typeof globalThis.addEventListener !== "function") {
  const target = new EventTarget();
  for (const m of ["addEventListener", "removeEventListener", "dispatchEvent"]) globalThis[m] = target[m].bind(target);
}

const source = await readFile(new URL("nsh.js", import.meta.url), "utf8");
const { counted } = await import(`data:text/javascript,${encodeURIComponent(`${source}\nexport { counted };`)}`);

/** 收集这次测试里发出的 `nsh-load-progress`。 */
function listen() {
  const seen = [];
  const on = (e) => seen.push(e.detail);
  globalThis.addEventListener("nsh-load-progress", on);
  return { seen, stop: () => globalThis.removeEventListener("nsh-load-progress", on) };
}

/** 分几块吐出数据的响应；`fail` 时吐完这些块后读出错。 */
function response(chunks, headers = {}, fail = false) {
  const body = new ReadableStream({
    start(c) {
      for (const n of chunks) c.enqueue(new Uint8Array(n));
      if (fail) c.error(new Error("断了"));
      else c.close();
    },
  });
  return new Response(body, { headers });
}

test("数收到的字节，最后发 done", async () => {
  const { seen, stop } = listen();
  const out = counted(response([3, 4], { "Content-Length": "7" }));
  assert.equal((await out.arrayBuffer()).byteLength, 7, "内容原样");
  stop();
  assert.deepEqual(seen.map((d) => [d.loaded, d.length, d.done]), [[0, 7, false], [3, 7, false], [7, 7, false], [7, 7, true]]);
  assert.deepEqual(globalThis.nshLoadProgress, { loaded: 7, length: 7, encoding: "", done: true });
});

test("没有 Content-Length 时 length 是 0，照样数", async () => {
  const { seen, stop } = listen();
  await counted(response([5])).arrayBuffer();
  stop();
  assert.deepEqual(seen.map((d) => [d.loaded, d.length, d.done]), [[0, 0, false], [5, 0, false], [5, 0, true]]);
});

test("压缩传输时带上编码，length 是压缩后的大小", async () => {
  const { seen, stop } = listen();
  await counted(response([2], { "Content-Length": "2", "Content-Encoding": "br" })).arrayBuffer();
  stop();
  assert.equal(seen.at(-1).encoding, "br");
  assert.equal(seen.at(-1).length, 2);
});

test("读出错时报错给读的人，不发 done", async () => {
  const { seen, stop } = listen();
  await assert.rejects(counted(response([1], {}, true)).arrayBuffer());
  stop();
  assert.ok(seen.every((d) => !d.done));
});

test("失败的响应原样返回，不数", () => {
  const failed = new Response("x", { status: 404 });
  assert.equal(counted(failed), failed);
});
