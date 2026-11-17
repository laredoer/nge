# nsh-platform-web 0.4.0（改过的副本）

本目录是 crates.io 上 `nsh-platform-web` 0.4.0 的副本（许可证不变：Apache-2.0，见 `LICENSE-APACHE`），
仓库根目录的 `.cargo/config.toml` 用 `[patch.crates-io]` 指到这里。相对 0.4.0 的改动：

- `src/native.rs`：原生组件（`NativeComponent`、`ComponentContext`、`ComponentFactory`、`register_component`），
  `src/lib.rs` 的 `flush` 在每批 DOM 建好以后交给组件，`nsh_stop` 时释放。
- `src/lib.rs`：公开 `asset_url`、`read_asset`。
- `src/encode.rs`：组件属性和 `command` 不再当作不支持。
- `web/nsh.js`：下载运行时 wasm 时发 `nsh-load-progress` 事件（加载页的进度条用）；`op_comp` 给组件容器加 `data-nsh-id`、`data-nsh-component`，容器里组件放的元素不吃指针事件；
  页面藏起来（`visibilitychange`）、窗口没了焦点（`blur`）时取消所有按着的指针（`cancelPointers`），收触摸的视图收到 touchcancel
  （浏览器这时可能不再发 pointerup，按着的虚拟摇杆、屏幕按钮会一直按着，见 `docs/touch-input.md`）。

见仓库的 `docs/web.md`。

原来的 README 和文档见 crates.io 上的 `nsh-platform-web` 0.4.0 和 nsh 的网站。
