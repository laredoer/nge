// 加载页的进度计算：`node --test examples/sanguo/web/loading.test.mjs`（没有 document 时不建页面）。

import assert from "node:assert/strict";
import test from "node:test";

import { engineProgress } from "./loading.js";

/** 按顺序喂一串进度，返回每一步的 part。 */
function run(details) {
  let prev;
  return details.map((d) => (prev = engineProgress(d, prev)).part);
}

test("知道大小时按比例走，到不了头", () => {
  const p = engineProgress({ loaded: 50, length: 100 });
  assert.equal(p.part, 0.5);
  assert.equal(p.text, "下载引擎 0.0 / 0.0 MB");
  assert.equal(engineProgress({ loaded: 100, length: 100 }).part, 0.99);
  assert.equal(engineProgress({ loaded: 100, length: 100, done: true }).part, 1);
});

test("没有编码或 identity 按原始大小算", () => {
  for (const encoding of ["", "identity", " Identity "]) {
    assert.equal(engineProgress({ loaded: 25, length: 100, encoding }).part, 0.25, encoding);
  }
});

test("压缩比估小了也不倒退：超过估计就把总数往上调", () => {
  // 原始 100、brotli 后 20：按 3.7 估成 74，收到 80 时已经超过
  const parts = run([30, 60, 74, 80, 90, 100].map((loaded) => ({ loaded, length: 20, encoding: "br" })));
  for (let i = 1; i < parts.length; i++) assert.ok(parts[i] >= parts[i - 1], `第 ${i} 步倒退了：${parts}`);
  assert.ok(parts.at(-1) > 0.9, `最后接近满：${parts}`);
});

test("超过估计时总数跟着调大，不直接顶到头", () => {
  const p = engineProgress({ loaded: 80, length: 20, encoding: "br" });
  assert.ok(p.total > 80, `${p.total}`);
  assert.ok(p.part < 0.99, `${p.part}`);
  assert.match(p.text, /\//, "有总数");
});

test("不认识的编码按保守的压缩比估，不会是 0", () => {
  const p = engineProgress({ loaded: 10, length: 10, encoding: "zstd" });
  assert.ok(p.part > 0 && p.part < 0.5, `${p.part}`);
  assert.match(p.text, /\//, "有总数");
});

test("拿不到大小时慢慢逼近，只增不减", () => {
  const parts = run([1e6, 5e6, 20e6, 60e6].map((loaded) => ({ loaded })));
  for (let i = 1; i < parts.length; i++) assert.ok(parts[i] > parts[i - 1]);
  assert.ok(parts.at(-1) <= 0.95);
});

test("中途换了大小信息也不倒退", () => {
  const parts = run([{ loaded: 90, length: 100 }, { loaded: 95 }]);
  assert.ok(parts[1] >= parts[0], `${parts}`);
});
