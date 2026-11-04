# effects

M5 验收示例：贴图、字体、glTF 全部用 `asset:/` 异步加载；同一张贴图用在 2D 精灵和 3D 立方体上；点击炸出粒子并飘字；「压力测试」开一个 10k 粒子的发射器。

```bash
cargo run -p effects --release
```

`assets/Inter-Bold.ttf`：Inter 字体（Rasmus Andersson），SIL Open Font License 1.1，取自 glyphon 0.12 的 examples 目录。只含拉丁字母，所以画面里的文字用英文和数字；中文提示由 NSH 的 HUD 显示。
