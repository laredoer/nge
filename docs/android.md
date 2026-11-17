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
- 键盘和手柄：`NshView.dispatchKeyEvent` 在快捷键之前先给挂着的 `RustComponent` 看，`dispatchGenericMotionEvent` 把手柄的轴交给它们；吞了的不再往下传。焦点在输入框里时告诉组件「在打字」。
- 改动都在 README 里列出来；测试照 Web 的做法放在 vendor 里（Android 之外编不出 JNI 部分，测的是纯逻辑）。

### 打包（不用 Gradle）

`nsh package --target android` 用 SDK 的 build-tools 直接打 APK（aapt2、zipalign、apksigner），用的 Java 运行时是 nsh 安装目录里预编译的 `classes.dex`。我们改了 Java，所以：

- `tools/nsh-android.sh`：用 `javac --release 17` 和 `d8 --min-api 28` 把 vendor 里的 Java 编成 `classes.dex`（和 nsh 的 `cargo xtask android` 同样的步骤），放在 `target/nge-android/runtime/android/{release,dev}`，设 `NSH_RUNTIME` 指过去再跑 nsh。没变的不重编。
  - 找 SDK：`ANDROID_HOME`，没有就用 `~/Library/Android/sdk`；NDK 用 SDK 里最新的一个。
  - 参数原样交给 nsh，比如 `tools/nsh-android.sh package --target android --abi arm64-v8a -o out examples/platformer`，加 `--run` 装到第一个设备上并启动。
- `libnsh.so` 由 nsh 生成的工作区编（`android/`），里面是 vendor 的 `nsh-platform-android`（Rust 部分）和游戏的 crate，16 KB 对齐。
- `tools/check-android.sh`：引擎全部 crate 和平台跳跃、长坂坡 `cargo check --target aarch64-linux-android`（`x86_64-linux-android` 也查一遍），再编一次 Java 运行时（只编不打包）。

### 平台层（`nge-platform` 的 `android` 模块）

- `component(factory)`：给 `register_component` 的工厂。第一次建游戏视图时：
  - 把 `asset:/` 的来源设成 `ComponentContext::read_asset`；
  - 把 `codeCacheDir/nge` 设成管线缓存目录；
  - 初始化 `ndk-context`（cpal 的 AAudio 后端要用）。
- 画面：`surface` 回调拿到 `ANativeWindow`（`ndk` 的 `NativeWindow`，`wgpu::Surface` 自己拿着它，跟着 surface 一起释放）。
  - 第一次有画面时才起引擎（`Engine::spawn`），之前收到的属性和 `command` 先攒着，起来后按顺序发过去。
  - 画面没了（切后台、锁屏）：同步暂停渲染线程；回来时用同一个 `wgpu::Instance` 建新的 surface，发 `Msg::Surface` 换上，再恢复。
  - 视口：像素尺寸来自 `surfaceChanged`，point 尺寸是像素除以密度。
- 触摸：游戏视图的盒子收触摸，和 iOS 一样走 NSH（`onTouch…`、`KeyButton`），平台层不用管。
- 键盘：Android 的键码按表换成 Web 的 `KeyboardEvent.code`（`android_keys.rs`，和 macOS 认识的键一一对应），然后和 macOS、浏览器共用 `input.rs` 的判定（`captureKeys`、按着 Ctrl / Meta 时不转发、修饰键自己照发、在打字时不转发）。
- 手柄：按钮是按键事件（`KEYCODE_BUTTON_A` 等），换成 `GamepadA`…；来自手柄的十字键键码换成 `GamepadUp`…，来自键盘的是方向键。摇杆、扳机、十字键的轴（`AXIS_X/Y/Z/RZ`、`AXIS_HAT_X/Y`、扳机）拼成 `pad::Reading`，和 Mac 共用 `pad.rs` 的合并、死区、比较：方向走 `key`，模拟量走 `gamepad` command。Android 的 y 轴朝下，换成朝上。
- 视图没了画面（后台）时按着的键和手柄全部补发松开、摇杆归零。

### 引擎（`nge-core`）

- Android 上 wgpu 只开 Vulkan 后端；拿不到 Vulkan 适配器时发 `ENGINE_ERROR`（不退到 GLES：3D 用到顶点阶段的 storage buffer、sRGB 贴图用 `view_formats`，GLES 上都不保证）。
- `Msg::Surface(wgpu::Surface)`：换画面，按原来的配置重新 `configure`，清掉「画面丢了」的标记。还在等 GPU 时收到的新画面换掉旧的。
- `Gfx::pipeline_cache`：所有建管线的地方（2D 精灵和预热线程、后处理、bloom、3D、阴影、粒子）都传它，没有就是 `None`。

### 管线磁盘缓存（`nge-core` 的 `pipeline_cache.rs`）

- 平台层设了缓存目录、适配器支持 `PIPELINE_CACHE`（现在只有 Vulkan）时才开：请求这个特性，从文件读出旧数据建 `PipelineCache`（`fallback: true`：wgpu 发现数据和驱动对不上时当空的）。
- 文件名：`wgpu::util::pipeline_cache_key`（Vulkan 是 `wgpu_pipeline_cache_vulkan_<vendor>_<device>`），换了 GPU 就是另一个文件；Metal、GLES 没有名字，不开。
- 文件格式：我们自己包一层，wgpu 的数据原样放在后面：
  - 魔数 `NGEPCAC1`；
  - 引擎版本（`nge-core` 的版本号）：对不上当没有（wgpu 自己的数据头另外核对 wgpu 和驱动的版本）；
  - 数据长度、FNV-1a 64 校验和：截断、改坏的数据不交给驱动（有的驱动拿到坏数据会崩，不能全靠 wgpu 和驱动自己查）。
- 读：文件不在、读不了当没有；坏了或版本不对记一次 warn、删掉文件，当没有。
- 写：先写 `<名字>.tmp` 再改名，写到一半被杀不会留下半截文件；数据比上次存的没变就不写。时机：渲染线程暂停（切后台）、退出，以及开始跑以后 10 秒存一次（预热编完的管线，不等切后台）。
- 存在 `codeCacheDir` 下：系统在 App 更新后会清掉这个目录，正好对应换了着色器。

### 示例

- `platformer`、`sanguo` 的 `#[native_module]` 在 Android 下调 `nge_platform::android::register(factory())`；`Cargo.toml` 加 Android 的依赖。
- 平台跳跃在 Android 上 `platform.desktop` 是 `false`：显示触屏按钮、短的说明，和 iPhone 一样。

## 不做

- GLES 兜底（理由见上）；Android 9 以下。
- `armeabi-v7a`、x86（32 位）：只编 `arm64-v8a` 和模拟器的 `x86_64`。
- `nsh run --target android` 的热更新：要编带开发支持的运行时，没试；现在用 `package --run`。
- 切后台时停掉音乐：渲染线程停了，但 kira 的音频线程还在放。以后在平台层加一个让 `Audio` 暂停主轨道的 command。
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
  - vendor `native`：没注册的名字只报一次；注册了的能建出来；同名再注册换掉。
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

- 模拟器（`system-images;android-35;google_apis;arm64-v8a`，`-gpu swiftshader_indirect`，无窗口）：`tools/nsh-android.sh package --target android --abi arm64-v8a -o out examples/platformer` 打出 APK，`adb install` 装上：
  - logcat：`GPU：SwiftShader Device (LLVM 10.0.0)（Vulkan…）`；第一次 `管线缓存 …/code_cache/nge/wgpu_pipeline_cache_vulkan_6880_49374：没有存过`，10 秒后 `管线缓存存了 96 字节`；`am force-stop` 再启动是 `读到 96 字节`。SwiftShader 的缓存只有个头，真机上才有意义。
  - 截图：完整场景、NSH 的 HUD 文字和触屏按钮；`adb shell input swipe` 按住 → 角色往右走；`input keyevent --duration` 按住方向键、`A` 能走；`input gamepad keyevent` 按住十字键能走、按 `BUTTON_A` 会跳（还吃到一个金币）。
  - 按 Home 再回来：画面接着跑，不崩。SwiftShader 一帧要两三百毫秒，logcat 里有一条「渲染线程 250ms 内没有确认暂停」。
  - 存档：`files/nsh/<包名>/storage.json` 里有 `nge:save:platformer:0`，重启后 HUD 还是「Save 1」。
  - 声音：AAudio 流开起来了（`AudioStreamBuilder … OUTPUT`），模拟器没开声音。
- 长坂坡同样打包装上：3D 场景、阴影、火堆粒子和 HUD 都画出来了，管线缓存照样存了；没有在模拟器上玩一局。

## 还没手测

- 真机（只在模拟器上跑过，模拟器的 Vulkan 是 SwiftShader 软件实现）；真机上的帧率、管线缓存省了多少时间。
- 声音：模拟器里看到 AAudio 流开起来了，没有用耳朵听。
- 硬件键盘、蓝牙 / USB 手柄（模拟器没有手柄）。
- 横竖屏切换、分屏、折叠屏。
- 长坂坡在 Android 上的完整一局（它没有触屏按钮，要接键盘或手柄）。
