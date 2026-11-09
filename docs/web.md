# Web：长坂坡在浏览器里玩（WebGPU）

> 前置：生成的宿主（[native-components.md](native-components.md)）、资源层（[m5-extensions.md](m5-extensions.md)）、键盘（[keyboard.md](keyboard.md)）、音频（[audio.md](audio.md)）、示例游戏长坂坡（[sanguo.md](sanguo.md)）。

## 目标

`nsh package --target web examples/sanguo` 打出一个静态站点，用任意静态服务器起起来，在支持 WebGPU 的桌面浏览器里能玩完一局：画面、键盘、音效、HUD 都和 macOS 上一样。

**验收标准**：
- 引擎全部 crate 和 `sanguo` 能编成 `wasm32-unknown-unknown`；macOS、iOS 不回归。
- 打包出的页面里看到游戏画面，HUD 是 NSH 的 DOM 文字；改窗口大小画面跟着变，切走标签页再回来接着跑。
- 方向键 / WASD 移动，J / K / L 出招，P / R 暂停、重开；方向键不滚页面。
- 第一次按键或点按之后有音效。
- 不支持 WebGPU 的浏览器显示一行提示，不白屏。

## 做法

### 工具链

- `wasm32-unknown-unknown` target（`rustup target add`）。
- `wasm-bindgen` 命令行，版本必须和锁文件里的 `wasm-bindgen` 一致。`nge-core` 在 wasm 下把它锁在 `=0.2.129`（nsh 0.4 的胶水按这个版本写），装法：`cargo install wasm-bindgen-cli --version 0.2.129 --locked`。
- `llvm-ar`：`zstd-sys` 的构建脚本要用它打 wasm 的静态库，Apple 自带的工具链里没有。装 Homebrew 的 `llvm@20`，构建时把它的 `bin` 放进 `PATH`（只在这条命令里），或者设 `AR_wasm32_unknown_unknown` 指过去：

  ```sh
  PATH="$(brew --prefix llvm@20)/bin:$PATH" nsh package --target web -o out examples/sanguo
  ```

- Rust 1.99.0 的 macOS 工具链里，`rust-lld` 按自己旁边的 `../lib` 找 `libLLVM.dylib`，那里没有，链接 wasm 时直接崩（`Library not loaded: @rpath/libLLVM.dylib`）。不改工具链，构建时设 `DYLD_FALLBACK_LIBRARY_PATH="$(rustc --print sysroot)/lib"` 让它去工具链的 `lib` 里找。
- `tools/nsh-web.sh` 把上面两件事都做了（只对这一条命令生效），参数原样交给 nsh：`tools/nsh-web.sh package --target web -o out examples/sanguo`。`tools/check-wasm.sh` 按同样的方式找 `llvm-ar`，检查全部引擎 crate 和示例能编成 wasm。

### nsh 的 web 平台（`vendor/nsh-platform-web`）

nsh 0.4 的 web 平台不支持原生组件：组件的属性、`command` 都被丢掉，`#[native_components]` 也不认 `web`。仓库里放一份 `nsh-platform-web` 0.4.0（版本号不变、保留原来的许可证），`.cargo/config.toml` 的 `[patch.crates-io]` 指过去；nsh 打包时从解析出的这个 crate 拷 `web/nsh.js`，所以改过的 JS 也会带上。

加的东西照 macOS 平台的形状（`src/native.rs`）：
- `NativeComponent`（`set_prop`、`command`），`ComponentContext`（`emit`、`set_size`、容器元素的选择器），`ComponentFactory`。
- `register_component(name, factory)`：全局注册。生成的 web 运行时不调用组件注册函数，所以由应用的 `#[native_module]` 在 wasm 下注册；这个函数只有 TSX `import "<应用>/native"` 时才会被打包保留。
- 每次提交给 JS 之后，按顺序处理这一批里的组件创建、属性、`command`、删除。
- 公开 `asset_url`（`asset:/…` 换成发布后的地址）和 `read_asset`（页面预加载过的文件，即字体）。
- `nsh.js` 给组件容器加 `data-nsh-id`、`data-nsh-component`，容器里的子元素不吃指针事件（触摸照常落到 NSH 的 view 上，`onTouch*` 才收得到）。

### 引擎在浏览器主线程上跑

wasm32 上没有线程、`std::time::Instant::now` 会 panic、主线程不能阻塞，所以：

- `nge-core`：「一帧」（处理消息、更新、画）抽成和驱动无关的部分；原生照旧由渲染线程驱动，wasm 由 `requestAnimationFrame` 每帧调一次。GPU 初始化（`request_adapter` / `request_device`）在 wasm 下是异步的，初始化期间收到的属性和 `command` 先攒着。WebGPU 画布没有 sRGB 格式：画布配成 `Bgra8Unorm` / `Rgba8Unorm`，加 sRGB 的 view format，引擎画到 sRGB 的 view 上，颜色和原生一致。计时统一用 `web-time`（原生上就是 `std::time::Instant`）。
- 只用 WebGPU 后端：3D 的蒙皮和变形用顶点阶段的 storage buffer，WebGL2 没有。拿不到适配器时发 `ENGINE_ERROR`，TSX 显示提示。
- wgpu 开 `fragile-send-sync-non-atomic-wasm`：单线程下 wgpu 对象也当 `Send + Sync`，ECS 的资源才放得进去。`nge-render2d` 直接依赖 `naga` 校验着色器（纯 WebGPU 后端的 wgpu 不再导出 naga）。
- `nge-assets`：wasm 下不起加载线程，任务每帧在主线程上同步处理。
- `nge-render2d` 的管线预热、`nge-text` 扫系统字体：wasm 下不起线程。

### 平台层（`nge-platform` 的 `web` 模块）

- `register(factory, assets)`：把游戏视图注册成 `nge.game` 组件。
- 组件在容器里建一个 `<canvas>`（铺满、不吃指针事件），每帧按 `clientWidth / clientHeight × devicePixelRatio` 算视口，变了才发给引擎。
- 页面隐藏（`visibilitychange`）时暂停，回来接着跑。
- 资源：初始化时把应用登记的 `asset:/` 文件全部 `fetch` 进内存（`MemorySource`），再建游戏；取不到的发 `ENGINE_ERROR`。
- 键盘：`window` 上监听 `keydown` / `keyup`，`KeyboardEvent.code` 就是引擎的键名；`captureKeys` 里的键 `preventDefault`；焦点在输入框里时不转发；失焦、页面隐藏时补发松开。按键判定和 macOS 共用。
- 声音：浏览器不让页面在用户操作前出声。wasm 下 `nge-audio` 先不建混音器，平台层在第一次 `keydown` / `pointerdown` 的处理函数里同步通知引擎建（Safari 要求在手势的调用栈里）。之前要播的声音丢掉。

### 文字和字体

长坂坡的 HUD、标题、提示都是 NSH 的 `<Text>`，web 上是 DOM 文字，用浏览器的字体，中文不用打包字体。引擎里画的文字只有预警的「!」和性能面板的数字；wasm 下 `nge-text` 不扫系统字体，用仓库里已有的字体当默认字体。

### 示例

- `sanguo` 的 `#[native_module]` 在 wasm 下调 `nge_platform::web::register`，`App.tsx` 加 `import "sanguo/native"`（打包时才保留注册函数）。

## 不做

- WebGL2 兜底（要换一套不用 storage buffer 的 3D 布局，wasm 也会大很多）。
- Web Worker + OffscreenCanvas（要 SharedArrayBuffer 和 COOP / COEP 响应头，nsh 的 web 运行时不支持）；现在 NSH 布局、TSX、ECS、渲染、WebAudio 都在主线程上。
- 资源按需加载：现在启动时一次取完，长坂坡全部资源不到 0.5 MB。
- 给 DOM 的 HUD 打包中文字体。
- 手柄、触屏按钮。
- `nsh run --target web` 的热更新。

## 验收

- `tools/check-wasm.sh`：引擎全部 crate 和 `sanguo` 能编成 wasm。
- `cargo test --workspace`、`nsh test`、`nsh check`、iOS 的 `cargo check` 不回归。
- 纯逻辑的单元测试：资源清单、按键判定。
- `nsh package --target web` 成功；用静态服务器起起来，无头 Chrome（开 WebGPU）能出画面、控制台没有 panic。

## 还没手测

- Safari、Firefox；手机浏览器。
- 有声音、按键手感、掉帧时 WebAudio 爆不爆音。
