# bounce

最早的验收示例：Rust 写的根组件（`src/lib.rs` 的 `App`，`nsh.json` 里 `"root": "App"`），游戏视图上叠一层 NSH 的 HUD。

```bash
nsh run examples/bounce                # Mac
nsh run examples/bounce --target ios   # iOS 模拟器
```

改 Rust 代码后 `nsh run` 会重新编译并重启。
