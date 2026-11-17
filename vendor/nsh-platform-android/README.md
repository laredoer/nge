# nsh-platform-android 0.4.0（改过的副本）

本目录是 crates.io 上 `nsh-platform-android` 0.4.0 的副本（许可证不变：Apache-2.0，见 `LICENSE-APACHE`），
仓库根目录的 `.cargo/config.toml` 用 `[patch.crates-io]` 指到这里。相对 0.4.0 的改动：

- `src/native.rs`：Rust 写的原生组件（`NativeComponent`、`ComponentContext`、`ComponentFactory`、`Surface`、`Key`、
  `register_component`）和它们的 JNI 入口 `Native.rustComponent*`；`src/registry.rs` 按名字登记工厂。
  - 每个 JNI 入口都包在 `src/guard.rs` 的 `guard` 里：Rust 里 panic 记一条 error、返回默认值，不越过 JNI 边界。
  - `Key` 带设备号（`device`）；手柄的轴（`AXES`）多了 `RX`、`RY`，设备没有的轴是 NaN。
  - `NativeComponent::input_lost`：手柄拔掉（只松开这个设备的）或窗口失焦（全部松开）。
  - `ComponentContext::new_android_context_ref`：单独给一个应用 `Context` 的全局引用（不释放），给 `ndk-context` 这类要一直拿着的。
- `src/host.rs`：`set_component_size`，组件能定自己的大小。
- `src/lib.rs`：公开上面这些。
- `java/.../RustComponent.java`（新加）：给 Rust 组件的 `SurfaceView`，把 surface、按键、手柄的轴转给 Rust。
  - 只给看得见（`isShown()`）的组件按键；「在打字」只算 `InputBox` 和 `onCheckIsTextEditor()` 的视图。
  - `InputDeviceListener` 收手柄拔掉；属性 `screenOrientation`（`"landscape"` / `"portrait"`）在这里锁 Activity 的方向，组件没了时放开。
- `java/.../Mounter.java`：Java 里没有这个组件时问 Rust 有没有登记。
- `java/.../NshView.java`：按键和手柄的轴先给 Rust 组件，它不要再走原来的路；窗口失焦时告诉 Rust 组件；`readAsset` 拒绝带 `..` 段的路径。
- `java/.../Native.java`：上面这些 JNI 方法的声明。
- `Cargo.toml`：加了 `tests/native` 测试（登记表和 `guard`），在本目录里 `cargo test --no-default-features --test native`；仓库的 `tools/check-android.sh` 会跑它。

改了 Java，打包要用改过的 `classes.dex`：仓库的 `tools/nsh-android.sh` 编好它再调 `nsh`。见仓库的 `docs/android.md`。

原来的 README 和文档见 crates.io 上的 `nsh-platform-android` 0.4.0 和 nsh 的网站。
