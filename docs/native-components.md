# 用 nsh 生成的宿主跑游戏（`#[native_components]`）

> 前置：nsh 0.4（[nsh-0.4.md](nsh-0.4.md)）、TSX 绑定（[m3-tsx.md](m3-tsx.md)）、平台层（[engine-design.md](engine-design.md)）。

## 目标

nsh 0.3 自带的宿主不能注册应用自己的原生组件，所以引擎自己写了两个宿主：macOS 的 dev host（`nge_platform::macos::dev_host`，配合 `nsh dev --server-only`），以及 iOS 的 `ios_app::run_app`。这带来几个麻烦：

- 开发要开两个终端：一个跑 dev server，一个跑 `cargo run`。
- 用不了 `nsh run --target ios`、`nsh package`、`nsh test --target`。
- 两个宿主都要跟着 nsh 的版本维护。

0.4 的 `#[native_components(<platform>)]` 让应用的 crate 把原生组件注册进 nsh 生成的宿主。改成用生成的宿主以后，一个 `nsh run` 就能开发：改 TSX 热更新，改 Rust 或资源会重新编译、重启应用。

**验收标准**：
- `nsh run examples/platformer` 在 Mac 上跑起来，能玩，键盘和手柄都能用；改 `App.tsx` 热更新，改 Rust 自动重启。
- `nsh run examples/platformer --target ios` 能在模拟器里跑起来。
- `cargo test --workspace` 和 iOS 检查照常通过。

## 做法

### 引擎（`nge-platform`）

- 不加新的注册函数：应用在 `#[native_components]` 里直接调 `components.register(GAME_COMPONENT, nge_platform::<平台>::component(factory()))`，和自写的宿主用的是同一个 `component`。
- 生成的宿主里没有人设置资源来源，引擎用 `assets!()` 的资源表（uri 对应文件路径，不把资源编进程序）：先在包里找（`bundle_dir`），找不到再读编译时的源文件路径。dev host 里那套从 dev server 读的 `HttpSource` 只给自写的宿主用。

### 示例的 crate

每个用生成宿主的示例：

- `nsh.json` 加 `"rust": "."`，示例的 crate 就是应用的 crate，它的 `lib` 有：
  - `#[native_module("<示例名>/native")]`：生成的工程要求有它。注册表里放 `assets!()` 的资源，没有别的导出。
  - `#[native_components(macos)]` 和 `#[native_components(ios)]`：调用上面的注册函数。
- `Cargo.toml` 按平台依赖 `nsh-platform-macos` 和 `nsh-platform-ios`（`default-features = false`，跟着工作区）。
- `main.rs` 的 dev host 先留着，`cargo run` 照样能用，等新的路走通再决定删不删。

先只改平台跳跃（演示都放在它里面），走通以后再改其他示例，再删掉自写的宿主。

## 要先确认的

- 生成的工作区在 `examples/platformer/.nsh/rust`，编译产物在 `examples/platformer/target`（第一次要整套重新编译）。`.nsh/` 已经被 git 忽略，`target/` 也是。
- 生成的宿主怎么设置窗口大小和性能浮层（dev host 的 `--window-size`、`--fps`）。
- 键盘、手柄、独立渲染线程在生成的宿主里是否照常工作。这些都在组件里面（NSView 和 UIView），按理和宿主无关。

## 不做

- Android 和 web：Android 的原生组件是 Java，web 没有原生组件，跨平台以后再做。
- `nsh test` 的端到端测试：等走通以后再看。

## 实测发现

- **补丁要写在 `.cargo/config.toml`**：生成的工作区（`.nsh/rust`）不认根 `Cargo.toml` 的 `[patch]`，iOS 宿主又从 crates.io 取了编不过的 `nsh-platform-ios`（见 [nsh-0.4.md](nsh-0.4.md)）。
- **iOS 最低版本**：`nsh package --target ios` 不设 `IPHONEOS_DEPLOYMENT_TARGET` 时按 iOS 10 链接，QuickJS 的 `___chkstk_darwin` 找不到。`.cargo/config.toml` 的 `[env]` 设成 15.0，和 nsh 要求的最低版本一致。
- **GPU 上限**：iOS 模拟器的 `max_inter_stage_shader_variables` 是 15，比 WebGPU 默认的 16 少，引擎按默认上限建设备时渲染线程直接退出。改成默认上限和硬件上限取较差的（`device_limits`），自写的宿主也受益。
- **窗口大小、性能浮层**：生成的宿主用 `nsh run --fps` 打开性能浮层。
- **真机签名**：nsh 0.4.0 的 `nsh package --target ios` 把描述文件里的通配钥匙串组（`TEAM.*`）原样签进应用，iOS 以「签名无效或开发者未被信任」拒绝启动，信任了开发者也一样。打完包用 `tools/resign-ios.sh <ipa> <证书 SHA-1> <输出 ipa>` 重签：权限取描述文件里的，钥匙串组换成 `<前缀>.<bundle id>`，再加 `--generate-entitlement-der`。两处是一起改的，没分开验证是哪一处起作用。上游修好以后删掉这个脚本。
- **包里的资源**：`nsh package` 把 `assets!()` 的资源放在 `<App>.app/nsh/assets/` 下，引擎原来只找 `Contents/Resources/nsh/assets`（Mac 的 .app）和程序旁边的 `assets`；iOS 的 .app 没有 `Contents/Resources`，真机上就回头去读编译时 Mac 上的路径。`bundle_dir` 加上程序旁边的 `nsh/assets`。
- **iOS 的系统字体**：fontdb 在 iOS 上按 Linux 的目录找系统字体，一个也找不到；字体库是空的时候 cosmic-text 排版直接 panic（「no default font found」），渲染线程退出。改成 iOS 上读 `/System/Library/Fonts`（应用能读），并且字体库是空的时候不排不指定字体的文字，不再 panic。fontdb 默认的 sans-serif 是 Arial，cosmic-text 在 iOS 上按 Linux 的 Noto 系列回退，都不一定有，所以 iOS 上不指定字体时依次用 PingFang SC、Helvetica Neue、Helvetica 里第一个有的。

## 验收记录

- `nsh run examples/platformer`：生成的 macOS 宿主里有引擎的 `nge-render`、`nge-assets` 线程（游戏组件注册上了）；改 `App.tsx` 热更新推过去。还没手测画面和键盘、手柄。
- `nsh run examples/platformer --target ios`（模拟器）：宿主连上 dev server，游戏组件挂上；修 GPU 上限之前渲染线程退出，修完还没在模拟器里重跑。
- 真机（iPhone 16 Plus）：`nsh package --target ios --sign … --profile …` 打的包启动被拦，重签以后能启动；启动后资源路径不对、渲染线程因为没有字体 panic，修完重新打包、重签，在真机上跑起来，控制台没有资源和字体的报错。真机上没法操作：键盘、手柄只在 Mac 上，平台跳跃又不读触摸；加了触屏按钮以后能玩（[touch-controls.md](touch-controls.md)）。
- `cargo test --workspace`、iOS 检查通过；新增 `device_limits`、包里的 `nsh/assets`、空字体库时跳过不指定字体的文字、默认字体家族选择的测试。
