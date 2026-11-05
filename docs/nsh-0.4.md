# 升级到 nsh 0.4

> 前置：TSX 绑定（[m3-tsx.md](m3-tsx.md)）、平台层（[engine-design.md](engine-design.md)）。

## 改了什么

- 依赖：`nsh-core` / `nsh-base` / `nsh-engine` / `nsh-platform-*` 从 0.3.0 升到 0.4.0。引擎用到的 Rust API（原生组件、`run_app`、dev host、`EngineControl`）没有不兼容的改动，Rust 代码不用改。
- TSX：0.4 去掉了 `YNode`，每种元素有自己的 ref 类型。游戏视图的 ref 改成 `NativeComponentRef`（`ts/nge.tsx` 的 `burst` / `push` / `tween` / `shake` / `sound` / `floatText` 和四个示例的 `App.tsx`），用法不变（`ref.command(name, args)`）。
- 0.4 的其他不兼容改动（样式进 `style`、事件参数是一个对象、函数返回对象）示例里都没用到，`nsh check` 通过。
- 开发用的 CLI 也要换成 0.4：`nsh dev --server-only` 起的 dev server 和引擎自写的 dev host 能连上，热更新正常。

## crates.io 上的 `nsh-platform-ios` 0.4.0 编不过

它用 `#[path = "../../nsh-platform-macos/src/speech.rs"]` 引了 monorepo 里另一个 crate 的文件，发布的包里没有这个文件（上游 longbridge/nsh#940）。

临时办法：根 `Cargo.toml` 的 `[patch.crates-io]` 把所有 nsh crate 都指到 git 的 `v0.4.0` tag（和 crates.io 上的 0.4.0 是同一个提交）。只换 `nsh-platform-ios` 不行：它从 git 取时依赖走上游 workspace 的路径，会多出一份 `nsh-core`，类型对不上。

上游发了修好的版本（0.4.1 之类）以后，删掉这段 `[patch.crates-io]`，版本号改过去。

## 以后可以做

- 0.4 的 `#[native_components(<platform>)]`：应用的 crate 能把原生组件注册进 nsh 生成的宿主（`nsh run`、`nsh package`、`nsh test --target`）。这样可能用不着自写的 macOS dev host 和 iOS 宿主（`ios_app.rs`，见 engine-design.md 的「已知缺口」），还能用 `nsh test` 做端到端测试。要看生成的宿主能不能满足引擎的需要（独立渲染线程、键盘和手柄监听）。
- `nsh/keymap` 是否有按键松开、TSX `hidden` 的视图能否检测到（keyboard.md、gamepad.md 里 0.3 的限制），要重新看。

## 验收记录

- `cargo test --workspace`、`cargo check --workspace --target aarch64-apple-ios` 通过。
- nsh 0.4 CLI 的 `nsh check` 通过 `examples/physics`、`platformer`、`scene`、`scene3d`。
- 平台跳跃 release 版连 0.4 的 `nsh dev --server-only`，进游戏、改 `App.tsx` 后热更新推送过去，游戏日志没有报错。还没手测玩法。
- 调试构建链接时多了一条提示：`__eh_frame` 段超过 16MB（0.4 的依赖变大了），只影响异常展开表的编码，不影响运行。
