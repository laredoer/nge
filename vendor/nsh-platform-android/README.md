# nsh-platform-android 0.4.0（改过的副本）

本目录是 crates.io 上 `nsh-platform-android` 0.4.0 的副本（许可证不变：Apache-2.0，见 `LICENSE-APACHE`），
仓库根目录的 `.cargo/config.toml` 用 `[patch.crates-io]` 指到这里。相对 0.4.0 的改动：

- `src/native.rs`：Rust 写的原生组件（`NativeComponent`、`ComponentContext`、`ComponentFactory`、`Surface`、`Key`、
  `register_component`）和它们的 JNI 入口 `Native.rustComponent*`；`src/registry.rs` 按名字登记工厂。
- `src/host.rs`：`set_component_size`，组件能定自己的大小。
- `src/lib.rs`：公开上面这些。
- `java/.../RustComponent.java`（新加）：给 Rust 组件的 `SurfaceView`，把 surface、按键、手柄的轴转给 Rust。
- `java/.../Mounter.java`：Java 里没有这个组件时问 Rust 有没有登记。
- `java/.../NshView.java`：按键和手柄的轴先给 Rust 组件，它不要再走原来的路。
- `java/.../Native.java`：上面这些 JNI 方法的声明。
- `Cargo.toml`：加了 `tests/native` 测试。

改了 Java，打包要用改过的 `classes.dex`：仓库的 `tools/nsh-android.sh` 编好它再调 `nsh`。见仓库的 `docs/android.md`。

原来的 README 和文档见 crates.io 上的 `nsh-platform-android` 0.4.0 和 nsh 的网站。
