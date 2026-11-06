# 所有示例换成 nsh 生成的宿主，删掉自写宿主

> 前置：用 nsh 生成的宿主跑游戏（[native-components.md](native-components.md)）。

## 目标

平台跳跃已经改用 `#[native_components]`。其余示例还靠引擎自写的两个宿主：

- macOS 的 dev host（`nge_platform::macos::dev_host`，`dev` feature）：physics、scene、scene3d 用，要先开 `nsh dev --server-only` 再 `cargo run`。
- iOS 的 `nge_platform::ios_app::run_app`：bounce、dodge、effects、model、sprites 的 `main.rs` 在 iOS 上用，配合 `examples/bounce/scripts/ios-device.sh` 装到真机。

两个宿主都要跟着 nsh 的版本维护。全部换成生成的宿主以后删掉它们。

**验收标准**：
- 每个示例都能 `nsh run examples/<名字>` 在 Mac 上跑起来（游戏组件注册上、没有报错），`nsh package --target ios` 能打包。
- 仓库里不再有 `dev_host`、`ios_app`、`dev` feature 和 `ios-device.sh`。
- `cargo test --workspace`、iOS 检查、每个示例的 `nsh check` 通过。

## 做法

每个示例照平台跳跃改：

- `lib.rs`：`#[native_components(macos)]` 和 `#[native_components(ios)]` 注册游戏视图；没有 `#[native_module]` 的加一个（生成的工程要求有它）；用 `asset:/` 读资源的示例在注册表里放 `assets!()`。
- `nsh.json`：都有 `"rust": "."`；Rust 根组件的示例有 `"root": "App"`。effects 和 model 原来没有 `nsh.json`，补上。
- `Cargo.toml`：按平台依赖 `nsh-platform-macos` / `nsh-platform-ios`；去掉只有 `main.rs` 用的依赖（`env_logger`，bounce 的 Linux / Windows 后端）。
- 删掉 `main.rs`：开发都用 `nsh run`，不再有 `cargo run` 这条路。Rust 根组件的示例原来在 Mac 上 `cargo run` 用的是 nsh 自己的 `run_app`，不是自写宿主，但留着它就要在两处注册组件，所以一起删。

引擎里：

- 删 `nge_platform::macos::dev_host` 和它的参数解析、`nge-platform` 的 `dev` feature、`ios_app.rs`。
- 删 `nge_assets::HttpSource`（只有 dev host 用它从 dev server 读资源）和 `ureq` 依赖。生成的宿主里资源按 `bundle_dir` → `assets!()` 的源文件路径读。
- 重签脚本从 `examples/platformer/tools/` 挪到根目录 `tools/resign-ios.sh`，所有示例共用。

窗口大小：生成的宿主用 `nsh run --window-size 480x800`，写在根目录的 README 里。

## 不做

- Linux / Windows：bounce 原来的 `main.rs` 能在 Linux / Windows 上跑 HUD（没有游戏视图），去掉。以后做跨平台时用生成的宿主。
- iOS 上的调试开关：原来 iOS 的 `main.rs` 打开 debug 日志，`ios-device.sh` 把 `NGE_SPRITES`、`NGE_DEBUG_REMOUNT_MS`、`NGE_DEBUG_STALL_MS` 传给真机。生成的宿主日志只到 info；要传环境变量用 `xcrun devicectl device process launch --environment-variables`。
- iOS 前后台：原来监听 scene 的进后台 / 回前台，现在监听 App 的。iPhone 上只有一个 scene，两者一样；iPad 多窗口时只有一个窗口进后台不会暂停。
- 以前文档里的验收记录（`cargo run`、`ios-device.sh`）是当时的事实，不改。

## 验收记录

- `cargo build --workspace`、`cargo test --workspace` 通过，没有新的 warning；`nge-platform` 的 iOS 检查（`aarch64-apple-ios`）通过。
- 9 个示例的 `nsh check` 都通过。
- 冒烟：9 个示例都能 `nsh run examples/<名字>` 在 Mac 上起来，宿主进程里有 `nge-render` 渲染线程，日志没有报错。还没手测画面和操作。
- 编译目录：生成的宿主默认在每个示例下各编一份 `target/`（5–21 GB），9 个示例加起来把磁盘占满过。设 `CARGO_TARGET_DIR` 共用一个目录后，scene、scene3d、platformer 三个一共 13 GB（各自编时 35 GB）。写进了根目录 README。
- 还没做：iPhone 上打包运行、切后台再回来是否暂停。

**修过的问题**：
- 第一个游戏视图在 App 已经进后台后才挂载时，按前台起步，后台提交 GPU 命令会被系统杀掉；装监听时先读 `applicationState`。
- bounce 收到平台的按键 / 手柄命令后照样发 `state`，摇杆变化时每帧白发一次；改成直接返回。
- 删宿主后的残留：nge-assets、bounce 里指向开发服务器和已删 README 小节的注释，工作区里没人用的 `nsh-engine`、`env_logger`。

