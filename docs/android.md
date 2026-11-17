# Android：平台跳跃在 Android 上跑（Vulkan），加管线磁盘缓存

> 前置：生成的宿主（[native-components.md](native-components.md)）、Web 移植的平台层改法（[web.md](web.md)）、手柄（[gamepad.md](gamepad.md)）、触屏按钮（[touch-controls.md](touch-controls.md)）、管线预热（[pipeline-warmup.md](pipeline-warmup.md)）。

## 目标

`tools/nsh-android.sh package --target android --abi arm64-v8a examples/platformer` 打出一个 APK，装到 Android 9（API 28）以上的手机或模拟器上能玩：画面、触摸、声音、资源、存档都和 iPhone 上一样。wgpu 在 Android 上只走 Vulkan。

顺带做 [pipeline-warmup.md](pipeline-warmup.md) 里留下的管线磁盘缓存：wgpu 30 的 `PipelineCache` 只有 Vulkan 实现，Android 的驱动又多半不自己缓存，所以 Android 上第二次启动编管线会快很多。

**验收标准**：
- 模拟器（arm64，API 35）上平台跳跃出完整画面，HUD 是 NSH 的文字；按住屏幕上的 → 角色往右走，按跳会跳。
- logcat 里有 Vulkan 适配器、管线缓存的读写记录；切到后台再回来接着跑，不黑屏、不崩。
- 第二次启动时读到了上一次存的管线缓存；缓存文件坏了、版本不对时当没有，不崩，下次存一份好的。
- 存档（`saved()` / `saveSlots()`）重开 App 后还在。
- 硬件键盘、手柄（蓝牙或 USB）转成和 Mac 一样的键名。
- macOS、iOS、Web 不回归。

## 做法

### nsh 的 Android 平台（`vendor/nsh-platform-android`）

nsh 0.4 有 Android 平台（`nsh-platform-android`，Java 的 `NshView` 加 Rust 的 `libnsh.so`），但原生组件只能在 Java 里往 `NshView` 上注册（`registerComponent`），`#[native_components]` 也不认 `android`；`nsh package` 用的 `AppActivity` 是预编译的，应用没有地方插 Java 代码。所以照 Web 的做法放一份 0.4.0 的副本（版本号不变、保留 Apache-2.0 许可证），`.cargo/config.toml` 的 `[patch.crates-io]` 指过去，在里面加 Rust 写的原生组件：

- Rust（`src/native.rs`）：`register_component(name, factory)` 全局注册；`NativeComponent` 有 `set_prop`、`command`、`surface`（画面建好 / 尺寸变了 / 没了）、`key`、`motion`；`ComponentContext` 能 `emit`、`set_size`，读 `asset:/` 文件（走 `NshView.readAsset`：APK 里的资源或 `nsh run` 的开发服务器），给出 Java VM、应用的 `Context` 和 `codeCacheDir`。
  - 和 Web 一样，生成的宿主不调组件注册函数，由应用的 `#[native_module]` 在 Android 下注册（它在每个引擎启动时都会跑）。
- Java（`RustComponent.java`）：`Mounter` 找不到 Java 注册的工厂时，问 Rust 有没有这个名字，有就建一个 `RustComponent`。它的视图是一个 `SurfaceView`：
  - `surfaceChanged` 把 `Surface`、像素尺寸、密度交给 Rust；`surfaceDestroyed` 同步通知，Rust 返回之前渲染线程已经停下、不再碰这个画面。
  - 属性、`command` 转成 JSON 交给 Rust；`destroy` 时释放 Rust 那一半。
- 键盘和手柄：`NshView.dispatchKeyEvent` 在快捷键之前先给挂着的 `RustComponent` 看，`dispatchGenericMotionEvent` 把手柄的轴交给它们；吞了的不再往下传。
  - 只给看得见的组件（`isShown()`）：游戏视图藏起来（切到别的页面）时键交回 NSH。
  - 焦点在输入框里（`InputBox`，或者 `onCheckIsTextEditor()` 的视图）时告诉组件「在打字」；焦点在普通按钮上不算。
  - 按键带上设备号：同一个键（扳机）既当按钮又当轴报上来时合成一次。
  - 设备没有的轴（`getMotionRange` 是 null）传 NaN；右摇杆按设备有哪些轴选 `RX/RY` 或 `Z/RZ`。
  - 手柄拔掉（`InputDeviceListener.onInputDeviceRemoved`）、窗口失焦（`onWindowFocusChanged`）时通知 Rust，按着的都松开。
- 属性 `screenOrientation`（`"landscape"` / `"portrait"`）在 Java 里处理：`Activity.setRequestedOrientation` 锁方向（跟着传感器转 180°），组件没了时放开。nsh 的清单模板没有方向的选项，所以放在游戏视图的属性上。
- 所有 JNI 入口包 `catch_unwind`（`guard.rs`）：Rust 里 panic 记一条 error、返回默认值，不越过 JNI 边界把进程带走。
- `NshView.readAsset` 拒绝带 `..` 段的路径。
- 改动都在 README 里列出来；测试照 Web 的做法放在 vendor 里（Android 之外编不出 JNI 部分，测的是纯逻辑）。

### 打包（不用 Gradle）

`nsh package --target android` 用 SDK 的 build-tools 直接打 APK（aapt2、zipalign、apksigner），用的 Java 运行时是 nsh 安装目录里预编译的 `classes.dex`。我们改了 Java，所以：

- `tools/nsh-android.sh`：用 `javac --release 17` 和 `d8 --min-api 28` 把 vendor 里的 Java 编成 `classes.dex`（和 nsh 的 `cargo xtask android` 同样的步骤），放在 `target/nge-android/runtime/android/{release,dev}`，设 `NSH_RUNTIME` 指过去再跑 nsh。没变的不重编。
  - 找 SDK：`ANDROID_HOME`，没有就用 `~/Library/Android/sdk`，都没有时提示设 `ANDROID_HOME`；NDK、build-tools 用 SDK 里最新的一个（版本号按数字排，`check-android.sh` 同一个规则）。
  - 要重编时先查 `javac`、`jar` 在不在、`javac` 是不是 17 以上。
  - 什么时候重编：还没有 `classes.dex`、有比它新的 Java 文件，或者 Java 文件加了删了（和上次编的文件列表 `sources.txt` 比）。javac 的参数文件里路径加引号，目录里有空格也行。
  - 参数原样交给 nsh，比如 `tools/nsh-android.sh package --target android --abi arm64-v8a -o out examples/platformer`，加 `--run` 装到第一个设备上并启动。
- `libnsh.so` 由 nsh 生成的工作区编（`android/`），里面是 vendor 的 `nsh-platform-android`（Rust 部分）和游戏的 crate，16 KB 对齐。
- `tools/check-android.sh`：引擎全部 crate 和平台跳跃、长坂坡 `cargo check --target aarch64-linux-android`（设 `NGE_ANDROID_X86_64=1` 时 `x86_64-linux-android` 也查一遍，默认不查：多编一整遍），再编一次 Java 运行时（只编不打包），最后进 `vendor/nsh-platform-android` 跑它自己的测试（`cargo test --no-default-features --test native`），跑完删掉生成的 `Cargo.lock`（`.gitignore` 里也忽略了）。
- Rust 1.99.0 的 macOS 工具链里 `rust-objcopy` 按 `@rpath` 找 `libLLVM.dylib`，那个位置没有，一跑就 SIGABRT。rustc 在 macOS 上用它剥构建脚本（`build.rs`）的调试信息，剥不了只报一条警告（`stripping debug info with rust-objcopy failed`），构建脚本照样能用，检查照样通过。原来脚本里设的 `DYLD_FALLBACK_LIBRARY_PATH` 没用：报错里列的查找路径只有系统默认的几个，环境变量没传到 `rust-objcopy`，已经删掉。也试过 `CARGO_PROFILE_*_BUILD_OVERRIDE_STRIP=false`：`.cargo/config.toml` 的 `[profile.dev.package."*"]` 对构建脚本优先，rustc 还是收到 `-C strip=debuginfo`。要去掉警告只能改工具链（补上 `libLLVM.dylib`）或者给所有依赖改 profile（全部重编），都不值，留着这条警告。

### 平台层（`nge-platform` 的 `android` 模块）

- `component(factory)`：给 `register_component` 的工厂。建游戏视图时：
  - 把 `asset:/` 的来源设成这个视图的 `ComponentContext::read_asset`；视图没了时清掉（只清自己设的那个），不让全局的来源一直拿着视图、`NshView` 和 `Activity`。
  - 第一次时把 `codeCacheDir/nge` 设成管线缓存目录，初始化 `ndk-context`（cpal 的 AAudio 后端要用）：给它单独一个应用 `Context` 的全局引用（故意不释放），不借组件的那个。
- 画面：`surface` 回调拿到 `ANativeWindow`（`ndk` 的 `NativeWindow`，`wgpu::Surface` 自己拿着它，跟着 surface 一起释放）。
  - 第一次有画面时才起引擎（`Engine::spawn`），之前收到的属性和 `command` 先攒着，起来后按顺序发过去。
  - 画面没了（切后台、锁屏）：同步暂停渲染线程，发 `nge.suspend { suspended: true }` 让声音停下；回来时先发 `suspended: false`，用同一个 `wgpu::Instance` 建新的 surface，发 `Msg::Surface` 换上，再恢复。
  - 攒着的属性同名的只留最新的一个（和 `Gate::push` 一样），最多攒 256 条，多了丢最早的、记 warn。
  - 视口：像素尺寸来自 `surfaceChanged`，point 尺寸是像素除以密度。
- 触摸：游戏视图的盒子收触摸，和 iOS 一样走 NSH（`onTouch…`、`KeyButton`），平台层不用管。
- 键盘：Android 的键码按表换成 Web 的 `KeyboardEvent.code`（`android_keys.rs`，和 macOS 认识的键一一对应），然后和 macOS、浏览器共用 `input.rs` 的判定（`captureKeys`、按着 Ctrl / Meta 时不转发、修饰键自己照发、在打字时不转发）。
- 手柄：按钮是按键事件（`KEYCODE_BUTTON_A` 等），换成 `GamepadA`…；来自手柄的十字键键码换成 `GamepadUp`…，来自键盘的是方向键。摇杆、扳机、十字键的轴（`AXIS_X/Y/Z/RZ`、`AXIS_HAT_X/Y`、扳机）拼成 `pad::Reading`，和 Mac 共用 `pad.rs` 的合并、死区、比较：方向走 `key`，模拟量走 `gamepad` command。Android 的 y 轴朝下，换成朝上。
- 手柄的按钮和轴合在一起比（`PadInput`）：同一个键（L2 / R2 既是按钮又是轴）按钮按着或者轴过了阈值都算按下，只发一次按下、一次松开。
- 视图没了画面（后台）、窗口失焦时按着的键和手柄全部补发松开、摇杆归零；拔掉一个手柄只松开它的。

### 引擎（`nge-core`）

- Android 上 wgpu 只开 Vulkan 后端；拿不到 Vulkan 适配器时发 `ENGINE_ERROR`（不退到 GLES：3D 用到顶点阶段的 storage buffer、sRGB 贴图用 `view_formats`，GLES 上都不保证）。
- `Msg::Surface(wgpu::Surface)`：换画面，按原来的配置重新 `configure`，清掉「画面丢了」的标记。还在等 GPU 时收到的新画面换掉旧的。
- 起 GPU（`startup.rs`，纯逻辑，画面、GPU 都是类型参数）：等到有视口、没暂停、画面能用，拿着「窗口锁」用最新的画面请求 GPU（只请求一次）；请求期间到的消息再过一遍：暂停了就等回来，换了画面就换上，再配置。
  - 配置拿不到画面的能力（画面已经没了）时留着 GPU，等下一个画面再配，不报错退出。
  - 帧循环之前平台层暂停：等渲染线程放开窗口锁（最多 2 秒）才返回，之后渲染线程一定先看到暂停。
- 画面格式：`caps.formats` 里有 sRGB 的就用它；没有才用第一个格式、`view_formats` 加它的 sRGB 版本（`surface_formats`，纯函数）。
- Android 上 `get_current_texture` 报 `Lost`（等暂停确认超过 250 ms 以后画面已经没了）只记下来，等新的画面，不发 `ENGINE_ERROR`。
- 内置 command `nge.suspend`：`App::on_suspend` 的钩子当场跑（渲染线程暂停着也跑）；`nge-audio` 用它暂停各总线的轨道、主轨道静音，回来时恢复。
- `Gfx::pipeline_cache`：所有建管线的地方（2D 精灵和预热线程、后处理、bloom、3D、阴影、粒子）都传它，没有就是 `None`。

### 管线磁盘缓存（`nge-core` 的 `pipeline_cache.rs`）

- 平台层设了缓存目录、适配器支持 `PIPELINE_CACHE`（现在只有 Vulkan）时才开：请求这个特性，从文件读出旧数据建 `PipelineCache`（`fallback: true`：wgpu 发现数据和驱动对不上时当空的）。
- 文件名：`wgpu::util::pipeline_cache_key`（Vulkan 是 `wgpu_pipeline_cache_vulkan_<vendor>_<device>`），换了 GPU 就是另一个文件；Metal、GLES 没有名字，不开。
- 文件格式：我们自己包一层，wgpu 的数据原样放在后面：
  - 魔数 `NGEPCAC1`；
  - 引擎版本（`nge-core` 的版本号）：对不上当没有（wgpu 自己的数据头另外核对 wgpu 和驱动的版本）；
  - 数据长度、FNV-1a 64 校验和：截断、改坏的数据不交给驱动（有的驱动拿到坏数据会崩，不能全靠 wgpu 和驱动自己查）。
- 读：文件不在、读不了当没有；坏了或版本不对记一次 warn、删掉文件，当没有。
- 写：先写 `<名字>.<pid>.<序号>.tmp` 再改名，写到一半被杀不会留下半截文件，两个进程、两次写也不会写到同一个临时文件；写失败删掉临时文件。数据比上次存的没变就不写。
- 时机：开始跑以后 10 秒存一次（预热编完的管线，不等切后台），之后每 60 秒一次；渲染线程暂停（切后台）、退出时也存。暂停时先确认暂停再存，不拖住平台层。
- 渲染线程上只取数据（`get_data`），写文件在后台线程。
- 数据比上次取到的少了一半以上时记一条 warn（驱动丢了缓存）。
- 存在 `codeCacheDir` 下：系统在 App 更新后会清掉这个目录，正好对应换了着色器。

### 示例

- `platformer`、`sanguo` 的 `#[native_module]` 在 Android 下调 `nge_platform::android::register(factory())`。示例的 `Cargo.toml` 不用改：Android 的依赖都在 `nge-platform` 里。
- 平台跳跃在 Android 上 `platform.desktop` 是 `false`：显示触屏按钮、短的说明，和 iPhone 一样。横竖屏都能玩，不锁方向。
- 长坂坡锁成横屏（`<GameView screenOrientation="landscape">`）。
- 安全区：两个示例都用 nsh 的 `safeArea()` 让开状态栏和刘海。平台跳跃的根视图加上上、左、右的边距（最上面那行字不再被状态栏盖住），触屏按钮离边的距离加上安全区（`padPlace`）；长坂坡的 HUD 整层缩进安全区。
- 平台跳跃没有中继时：HUD 原来一直是「连接中…」。现在连过、出了错、还在退避重连时显示「连不上中继，稍后重试」（`netLabel` 看 `room.error()`）。联网和桌面上一样默认开着。

## 不做

- GLES 兜底（理由见上）；Android 9 以下。
- `armeabi-v7a`、x86（32 位）：只编 `arm64-v8a` 和模拟器的 `x86_64`。
- `nsh run --target android` 的热更新：要编带开发支持的运行时，没试；现在用 `package --run`。
- 发布签名、图标、商店包（AAB）：nsh 的 `--keystore` 照常能用，没试。
- 触屏上的虚拟摇杆：用已有的 `KeyButton`。
- iOS、macOS 上的管线磁盘缓存：Metal 没实现 `PIPELINE_CACHE`，系统自己缓存。

## 验收

- `cargo test --workspace`、`cargo test -p nge-scene --no-default-features`、每个 `nsh test`、`nsh check`、`cargo check --target aarch64-apple-ios -p platformer -p sanguo`、`tools/check-wasm.sh` 不回归。
- `tools/check-android.sh` 通过。
- 纯逻辑的单元测试（在 Mac 上跑）：
  - `nge-core` `pipeline_cache`：编码再解码原样回来；截断、改了一个字节、魔数不对、版本不对、空文件都当没有；读到坏文件会删掉；写是原子的（不留 `.tmp`）、没变不重写、目录不在时建；文件名只有 Vulkan 有。
  - `nge-platform` `android_keys`：键码到键名，和 macOS 的键名表一样多；手柄的按钮、来自手柄的十字键；轴拼成 `Reading`（y 翻过来、扳机取大的）。
  - `nge-platform` `android`（纯逻辑部分）：画面来之前攒着的消息按顺序交出去；视口按密度算。
  - vendor `native`：没注册的名字只报一次；注册了的能建出来；同名再注册换掉；`guard` 接住 panic、返回默认值。
  - `nge-core` `startup`：第一个画面就能起来；请求 GPU 时暂停了，回来之前不配置、暂停当场确认；请求期间换来的画面是配置用的那个；配置失败留着 GPU（只请求一次），等新画面再配，等的时候退出不算错；什么都没开始就退出不请求 GPU；帧循环之前暂停等渲染线程用完画面，一直在用时超时。
  - `nge-core` `frame`：`Gate` 标了画面不能用以后，新画面来之前不开始；`surface_formats` 先挑 sRGB；`SaveClock` 10 秒一次、之后每 60 秒。
  - `nge-core` `pipeline_cache`：临时文件名按进程和每次写不一样；小了一半以上才算缩水。
  - `nge-ecs`：`nge.suspend` 当场跑钩子，不等帧。`nge-audio`：暂停时主轨道是 0、游戏设的音量不变，回来恢复；暂停着总线上的音乐不往前走，回来接着放。
  - `nge-assets`：只清自己设的资源来源。
  - `nge-platform` `android_keys`：右摇杆按设备有的轴选 `Z/RZ` 或 `RX/RY`；扳机既是按键又是轴时只按下一次；拔掉的手柄松开它的键和摇杆。`android_state`：同名属性只留最新的，消息最多 256 条。
  - 平台跳跃（`nsh test`）：出过错还没连上时显示「连不上中继」；触屏按钮的边距加上安全区。
- 下面这些测试都做过变异：把对应的逻辑改坏，测试会失败，再改回来。

| 测试 | 改坏（变异） |
| --- | --- |
| `pipeline_cache` 坏文件当没有 | `decode` 不比版本 |
| 同上 | 数据长度只查「多了」不查「少了」 |
| 同上 | 不比校验和 |
| 同上 | 魔数不对也往下读 |
| 坏文件删掉 | `load` 不删坏文件 |
| 没变不重写 | `save` 不比上次存的 |
| 同上 | `load` 读到以后不记下存着的是什么 |
| 目录不在时建 | `save` 不建目录 |
| 不留 `.tmp` | 改名换成复制 |
| 只有 Vulkan 有文件名 | 文件名不看后端 |
| `android_keys` 手柄的十字键 | 不看来源，十字键都当方向键 |
| 和 Mac 的键名一样多 | 空格的键码写错 |
| 手柄的来源 | 来源只要碰到一位就算手柄 |
| 快捷键 | 只认 Ctrl，不认 Meta |
| 轴拼成 `Reading` | 左摇杆的 y 不翻 |
| 同上 | 扳机不取 BRAKE / GAS |
| 画面没了全部松开 | `Pads::clear` 不清 |
| `android_state` 视口 | 密度是无穷大时不按 1 算 |
| 攒着的消息按顺序交出去 | 新的插到最前面 |
| vendor `registry` 同名再注册换掉 | 同名的留旧的 |
| 没注册的名字只报一次 | 注册了的也报 |
| 同上 | 每次都报 |
| `startup` 请求时暂停 | 请求 GPU 之后不再看一遍消息 |
| 配置失败等新画面 | 失败后不标「画面不能用」 |
| 同上 | 失败后丢掉 GPU |
| 帧循环之前暂停要等 | `wait_unlocked` 不等就返回 |
| `surface_formats` 先挑 sRGB | 只看第一个格式是不是 sRGB |
| `SaveClock` | 到点以后不排下一次 |
| `Gate` 画面不能用 | `ready` 不看这个标记 |
| 同上 | 换上新画面不清标记 |
| 临时文件名 | 不带序号 |
| 缩水 | 少一点就算 |
| `nge.suspend` 当场跑 | 钩子不跑 |
| 暂停时主轨道是 0 | 只看静音 |
| 暂停着音乐不往前走 | 暂停时总线轨道照放 |
| 扳机只按下一次 | 不合按钮，只看轴 |
| 右摇杆选轴 | 有 `RX/RY` 就用，不看 `Z/RZ` 和扳机 |
| 拔掉的手柄 | 不松它的按钮 |
| 同上 | 不清它的轴 |
| 同名属性只留最新的 | 不去重 |
| 最多 256 条 | 不设上限 |
| 只清自己设的来源 | 不看是不是自己设的 |
| `guard` 接住 panic | 不 `catch_unwind` |
| 「连不上中继」 | `netLabel` 不看错误 |
| 触屏按钮让开安全区 | 下边距不加安全区 |

- 模拟器（`system-images;android-35;google_apis;arm64-v8a`，`-gpu swiftshader_indirect`，无窗口；`-cores 2 -memory 2048` 就够，别和编译同时跑，机器会卡）：`tools/nsh-android.sh package --target android --abi arm64-v8a -o out examples/platformer` 打出 APK，`adb install` 装上：
  - logcat：`GPU：SwiftShader Device (LLVM 10.0.0)（Vulkan…）`；第一次 `管线缓存 …/code_cache/nge/wgpu_pipeline_cache_vulkan_6880_49374：没有存过`，10 秒后 `管线缓存存了 96 字节`；`am force-stop` 再启动是 `读到 96 字节`。SwiftShader 的缓存只有个头，真机上才有意义。
  - 截图：完整场景、NSH 的 HUD 文字和触屏按钮；`adb shell input swipe` 按住 → 角色往右走；`input keyevent --duration` 按住方向键、`A` 能走；`input gamepad keyevent` 按住十字键能走、按 `BUTTON_A` 会跳（还吃到一个金币）。
  - 按 Home 再回来：画面接着跑，不崩。SwiftShader 一帧要两三百毫秒，logcat 里有一条「渲染线程 250ms 内没有确认暂停」。
  - 存档：`files/nsh/<包名>/storage.json` 里有 `nge:save:platformer:0`，重启后 HUD 还是「Save 1」。
  - 声音：AAudio 流开起来了（`AudioStreamBuilder … OUTPUT`），模拟器没开声音。
- 长坂坡同样打包装上：3D 场景、阴影、火堆粒子和 HUD 都画出来了，管线缓存照样存了；没有在模拟器上玩一局。
- 修完以后在模拟器上又跑了一遍（两个 APK 重新打包）：
  - 平台跳跃：竖屏时最上面那行字在状态栏下面，没有中继时写「can't reach the relay, retrying」；管线缓存 10 秒后由后台线程存了 96 字节；按 Home 再回来是同一个进程，接着画，没有 `ENGINE_ERROR`；把模拟器转成横屏也能玩（触屏按钮在两边；带刘海的 AVD 上左边让开了刘海的位置）。
  - 长坂坡：竖着拿也是横屏，HUD 在状态栏下面；按 Home 再回来接着跑；`input gamepad keyevent` 按住十字键右，角色往右走。

## 修过的问题

- 刚启动就切后台（请求 GPU、配置画面期间画面没了）：原来渲染线程拿着旧画面配置，失败就整个引擎起不来。现在请求 GPU 之后再看一遍到了的消息，暂停了等回来、换了画面就换上；配置失败留着 GPU 等下一个画面；帧循环之前的暂停要等渲染线程放开画面才返回（`startup.rs`）。
- 画面格式：原来总用第一个格式再加 sRGB 的 `view_formats`；能力里本来就有 sRGB 格式时现在直接用它。
- 管线缓存：原来暂停时先存再确认，渲染线程上写文件，拖住切后台；文档说的频率和实现也对不上。现在先确认暂停，渲染线程只取数据、后台线程写文件；开始 10 秒存一次、之后每 60 秒，没变不写。
- 临时文件名带上进程号和序号，两个写不会撞上；缓存一下小了一半以上记 warn。
- 第一个 `NshView` / `Activity` 泄漏：`ndk-context` 借的是组件的 `Context` 引用，资源来源也一直拿着视图。现在 `ndk-context` 单独拿一个应用 `Context` 的全局引用；游戏视图没了时清掉自己设的资源来源。
- L2 / R2 既报按键又报轴，按一次发两次按下：按钮和轴合在一起再比。
- 手柄拔掉、窗口失焦时按着的键和摇杆一直按着：现在补发松开。
- 切后台时声音还在放：加了 `nge.suspend`，`Audio` 暂停总线轨道、主轨道静音，回来恢复。
- JNI 入口里的 panic 会越过 JNI 边界把进程带走：都包了 `catch_unwind`。
- Android 上等暂停确认超时以后，`get_current_texture` 报 `Lost` 会发 `ENGINE_ERROR`：现在只记下来，等新的画面。
- 焦点在普通按钮上也算「在打字」，键不交给游戏：只认 `InputBox` 和文本编辑器。
- 游戏视图看不见（切到别的页面）时手柄键还被吞掉：只给看得见的组件。
- 右摇杆只认 `Z/RZ`：按设备有哪些轴选 `RX/RY` 或 `Z/RZ`，设备没有的轴传 NaN。
- 画面来之前攒的属性没有去重、没有上限：同名的只留最新，最多 256 条。
- 脚本：javac 参数文件的路径没加引号；删了 Java 文件不重编；两个脚本选 NDK 的规则不一样；SDK 不在时没说设 `ANDROID_HOME`；没查 `javac`、`jar` 和 JDK 版本；压 `rust-objcopy` 警告的 `DYLD_FALLBACK_LIBRARY_PATH` 根本没生效，删掉并写明原因（见上）。
- vendor 的测试只能进目录手动跑：`check-android.sh` 会跑它，生成的 `Cargo.lock` 删掉并忽略。
- `NshView.readAsset` 能用 `..` 读到资源目录外面：拒绝。
- 文档说示例的 `Cargo.toml` 加了 Android 依赖，其实没有（都在 `nge-platform` 里）；缓存的频率写错了。
- 长坂坡在手机上是竖屏：锁成横屏。
- 状态栏盖住 HUD（长坂坡的血条、平台跳跃最上面那行字）：两个示例都让开安全区。
- 平台跳跃没有中继时一直显示「连接中…」：出过错、还在重连时显示「连不上中继，稍后重试」。

## 还没手测

- 真机（只在模拟器上跑过，模拟器的 Vulkan 是 SwiftShader 软件实现）；真机上的帧率、管线缓存省了多少时间。
- 声音：模拟器里看到 AAudio 流开起来了，没有用耳朵听。
- 硬件键盘、蓝牙 / USB 手柄（模拟器没有手柄）。
- 分屏、折叠屏；带刘海的真机上安全区对不对（模拟器只看了状态栏和横屏时的左边）。
- 切后台时声音真的停了：模拟器没开声音，靠单元测试。
- 手柄的 L2 / R2、右摇杆（`RX/RY`）、拔掉手柄：模拟器没有真手柄，靠单元测试。
- 长坂坡在 Android 上的完整一局（它没有触屏按钮，要接键盘或手柄）。
