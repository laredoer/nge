// Rust 写的根组件，在 TypeScript 里测：`nsh test` 用带 crate 的无头 runner 跑这个文件，
// `open(".")` 启动根组件（`nsh.json` 的 "root"）。无头 runner 里没有游戏视图，只测 HUD。
import { expect, open, test } from "nsh/test";

test("HUD 显示标题和按钮", async () => {
  const app = await open(".");
  const texts = app.texts();
  expect(texts).toContain("nge 弹球示例");
  for (const label of ["加一个球", "重置", "卡主线程 200ms", "减速"]) {
    expect(texts).toContain(label);
  }
});
