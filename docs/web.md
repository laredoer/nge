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
- `wasm-bindgen` 命令行，版本必须和锁文件里的 `wasm-bindgen` 一致。`nge-platform` 在 wasm 下把它锁在 `=0.2.129`（nsh 0.4 的胶水按这个版本写），装法：`cargo install wasm-bindgen-cli --version 0.2.129 --locked`。
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
- `nsh.js` 给组件容器加 `data-nsh-id`、`data-nsh-component`，容器里的子元素不吃指针事件（触摸照常落到 NSH 的 view 上，`onTouch*` 才收得到）。播放器的根元素带 `data-nsh-player`（页面上第几个播放器），并通过 `nsh_set_player` 告诉 wasm；`ComponentContext::selector` 限在这个根下，同一页面有几个播放器时找不错容器。
- 不支持 `ComponentPropSlots`（属性里放一棵 TSX 子树，macOS 平台有）：这种属性组件收不到。游戏视图用不到。
- 测试在宿主机上跑（`cd vendor/nsh-platform-web && cargo test --no-default-features --test native`）：和上游的 `text` 测试一样直接拿源文件编。

### 引擎在浏览器主线程上跑

wasm32 上没有线程、`std::time::Instant::now` 会 panic、主线程不能阻塞，所以：

- `nge-core`：「一帧」（处理消息、更新、画）抽成和驱动无关的部分；原生照旧由渲染线程驱动，wasm 由 `requestAnimationFrame` 每帧调一次。GPU 初始化（`request_adapter` / `request_device`）在 wasm 下是异步的，初始化期间收到的属性和 `command` 先攒着：同名的属性只留最后一次的值（放在最后一次的位置），`command` 按顺序全留。阶段（等 GPU → 等视口 → 跑 → 停）是纯逻辑（`phase.rs`），原生上也测：暂停或画布藏起来（视口是空的）时不更新不画，回来时不把中间这段算进 `dt`。GPU 设备丢了（不是引擎自己销毁的）发 `ENGINE_ERROR`。WebGPU 画布没有 sRGB 格式：画布配成 `Bgra8Unorm` / `Rgba8Unorm`，加 sRGB 的 view format，引擎画到 sRGB 的 view 上，颜色和原生一致。引擎每帧把画面的纹理放进 `Gfx::screen`，`Gfx::target_format` 只对这一张按 sRGB view 的格式算，别的纹理按自己的格式，不按格式猜。计时统一用 `web-time`（原生上就是 `std::time::Instant`）。
- 只用 WebGPU 后端：3D 的蒙皮和变形用顶点阶段的 storage buffer，WebGL2 没有。拿不到适配器时发 `ENGINE_ERROR`，TSX 显示提示。
- wgpu 开 `fragile-send-sync-non-atomic-wasm`：单线程下 wgpu 对象也当 `Send + Sync`，ECS 的资源才放得进去。`nge-render2d` 直接依赖 `naga` 校验着色器（纯 WebGPU 后端的 wgpu 不再导出 naga）。
- `nge-assets`：wasm 下不起加载线程，任务每帧在主线程上同步处理。
- `nge-render2d` 的管线预热、`nge-text` 扫系统字体：wasm 下不起线程。

### 平台层（`nge-platform` 的 `web` 模块）

- `register(factory, assets)`：把游戏视图注册成 `nge.game` 组件。
- 组件在容器里建一个 `<canvas>`（铺满、不吃指针事件），每帧按 `clientWidth / clientHeight × devicePixelRatio` 算视口，变了才发给引擎。
- 页面隐藏（`visibilitychange`）时暂停、挂起音频设备，回来接着跑、恢复声音。画布藏起来（大小是 0）时发空视口，引擎不更新也不画。
- 资源：初始化时把应用登记的 `asset:/` 文件全部 `fetch` 进内存（`MemorySource`），再建游戏；取不到的发 `ENGINE_ERROR`（JS 的 `Error` 显示它的 `message`）。没压缩的响应核对 `Content-Length`，对不上（连接断了）算取不到。每取完一个（按完成的先后）在 `window` 上发 `nge-progress`（`{done, total}`），出错发 `nge-error`（`{message}`），给加载页用。
- 第一帧画出来时引擎发 `ready` 事件（原生也发）：TSX 的 `onReady` 收到，平台层同时在 `window` 上发 `nge-ready`。长坂坡在 `ready` 之前不推进游戏（`pace.ts`），加载期间不会已经打起来了。
- 解码出错：`panic = "abort"`，web 上解码器 panic 会让整页停掉（不像原生只丢一个资源）。所以取资源时先核对长度，半截的文件不交给解码器；文件本身坏了的情况没有兜底。
- 键盘：`window` 上在捕获阶段监听 `keydown` / `keyup`（比 NSH 自己的处理先拿到），`KeyboardEvent.code` 就是引擎的键名，只转发 macOS 也认识的那组键；游戏要的键（`captureKeys`，默认字母、数字、方向键、空格、回车）`preventDefault` 并不再往下传，所以方向键不滚页面；焦点在输入框、可编辑区域、NSH 的输入框或 iframe 里，焦点在按钮、链接、`[role=button]`、`summary` 上时的回车和空格，或者按着 ⌘ / Ctrl（AltGr 报成的 Ctrl+Alt 不算）、正在输入法组字时不转发；修饰键自己的按下和松开照常转发。窗口失焦、页面隐藏时补发松开；macOS 的浏览器按着 ⌘ 时别的键收不到 `keyup`，所以松开 ⌘ 时把按着的键全部补发松开。判定逻辑（`input.rs`）和 macOS 共用。
- 声音：浏览器不让页面在用户操作前出声。wasm 下 `nge-audio` 先不建混音器，平台层在第一次算作用户激活的事件（除 Esc 外的 `keydown`、鼠标的 `pointerdown`、触摸和笔的 `pointerup`）的处理函数里同步调 `nge_audio::unlock()` 建（Safari 要求在手势的调用栈里），引擎下一帧接上，之前设的音量和分组设置补上去。解锁前的音效丢掉，循环的（音乐）排着，解锁后开始（解锁前就停掉的不会开始）。只认用户真按的事件（`isTrusted`）。游戏视图删掉再建（新的 `Audio`）后还能再解锁；之后每次手势都把没在跑的音频设备拉起来（`web_audio.rs` 在 `AudioContext.prototype.resume` 上记下 cpal 建的设备）。

### 文字和字体

长坂坡的 HUD、标题、提示都是 NSH 的 `<Text>`，web 上是 DOM 文字，用浏览器的字体，中文不用打包字体。引擎里画的文字只有预警的「!」和性能面板的数字；wasm 下 `nge-text` 不扫系统字体，不指定字体的文字用仓库里已有的 Inter Bold（`crates/nge-text/fonts`，SIL OFL 1.1）。它没有中文：性能面板的中文标签在 web 上画成方框，数字正常。

### 加载页

`nsh.json` 的 `web.host` 指向 `examples/sanguo/web/loading.js`：nsh 打包时原样带上，和 NSH 运行时一起最早加载。它一开始就盖住整个页面（深色底、「长坂坡」、暖金色进度条），手机宽度下照样排得开：

- 下载引擎：改过的 `nsh.js` 用 `ReadableStream` 数 wasm 收到的字节，发 `nsh-load-progress`（`{loaded, length, encoding, done}`）。压缩传输时 `Content-Length` 是压缩后的大小，按常见压缩比估原始大小；拿不到大小时按收到的字节慢慢逼近，不到头。显示「下载引擎 5.6 / 18.0 MB」，占进度条的 85%。
- 加载资源：引擎的 `nge-progress`，显示「加载资源 12 / 18」。
- 两段进度只增不减：压缩比估小了、收到的比估的多时把总数往上调；`identity` 按原始大小算；不认识的编码按保守的压缩比（4）估。
- 第一帧画出来（`nge-ready`）后淡出、藏起来（不删）。
- 失败（`nge-error`，或 NSH 页面自己的错误框）：显示原因和「重试」按钮（重新加载页面）。加载页退场以后出的错（比如 GPU 设备丢了）也会把它重新显示出来。

### 包体

- `nsh package --target web` 用生成的工作区里的 `profile.web`：`opt-level = "z"`、`lto = "fat"`、`codegen-units = 1`、`strip = true`、`panic = "abort"`，调试信息和符号都去掉了。nsh 0.4 不跑 `wasm-opt`，也没有开关。
- `nge-scene` 加了 `physics3d`、`ai` 两个 feature（默认开）：关掉时不加 3D 物理和追击的插件、系统，链接时 rapier3d 和寻路的代码就去掉了（wasm 原始少 1.0 MB，brotli 少约 0.26 MB）；场景里还声明了它们时记一次 warn。工作区里 `nge-scene` 默认不带 feature，用到的示例（`platformer`、`scene`、`physics`、`scene3d`）自己开；长坂坡不用，不开。
- 长坂坡的包（2026 年 11 月）：wasm 原始 17.95 MB、gzip 6.55 MB、brotli 4.73 MB；首屏要下的（wasm、JS、资源）原始 18.64 MB、gzip 7.07 MB、brotli 5.24 MB，其中资源 0.44 MB（wav、png、glb，nsh 不预压缩）；整个目录原始 19.9 MB、brotli 6.3 MB（含长坂坡用不到的 `nsh-syntaxes`、`nsh-math` 数据，不会被下载）。
- 试过、没做的：wasm 下跳过 `naga` 校验自定义着色器只少约 37 KB（brotli），还会丢掉友好的报错；`nge-audio` 只留 wav 解码器少约 100 KB（brotli），要给依赖它的每个 crate 加 feature，以后真要压时再做。剩下的大头是 NSH 运行时本身（QuickJS、布局、语法高亮、zstd、它自己的 naga 等），不在这个仓库里。

### 示例

- `sanguo` 的 `#[native_module]` 在 wasm 下调 `nge_platform::web::register`，`App.tsx` 加 `import "sanguo/native"`（打包时才保留注册函数）。
- 本地看：`tools/nsh-web.sh package --target web -o out examples/sanguo`，然后在 `out/sanguo` 里起任意静态服务器（比如 `python3 -m http.server`），用新版 Chrome 或 Edge 打开。线上部署时让服务器按 `Accept-Encoding` 发打包时生成的 `.br` / `.gz`。

## 不做

- WebGL2 兜底（要换一套不用 storage buffer 的 3D 布局，wasm 也会大很多）。
- Web Worker + OffscreenCanvas（要 SharedArrayBuffer 和 COOP / COEP 响应头，nsh 的 web 运行时不支持）；现在 NSH 布局、TSX、ECS、渲染、WebAudio 都在主线程上。
- 资源按需加载：现在启动时一次取完，长坂坡全部资源不到 0.5 MB。
- 给 DOM 的 HUD 打包中文字体。
- 触屏按钮。手柄后来加了（`navigator.getGamepads()`，标准布局），见 [input-actions.md](input-actions.md)。
- `nsh run --target web` 的热更新。

## 验收

- `tools/check-wasm.sh`：引擎全部 crate 和 `sanguo` 能编成 wasm；再单独编一次 `sanguo`（只开它自己选的 feature）。
- `cargo test --workspace`、`cargo test -p nge-scene --no-default-features`、`nsh test`、`nsh check`、iOS 的 `cargo check` 不回归。
- 纯逻辑的单元测试：资源清单和进度、按键判定和键名、`captureKeys` 解析、声音解锁前后（mock 后端）、内置字体排得出「!」和数字、`nge-scene` 的 feature 关掉时不加插件。
- JS 的测试：`node --test examples/sanguo/web/loading.test.mjs vendor/nsh-platform-web/web/load-progress.test.mjs`。
- vendor 的原生组件：`cd vendor/nsh-platform-web && cargo test --no-default-features --test native`。
- 下面这些测试都做过变异：把对应的改动换回原来的写法（或删掉），测试会失败。

| 测试 | 换回去（变异） |
| --- | --- |
| `loading.test.mjs`「压缩比估小了也不倒退」 | 去掉和上一次比取大 |
| `loading.test.mjs`「超过估计时总数跟着调大」 | 去掉超过时调大总数 |
| `loading.test.mjs`「没有编码或 identity 按原始大小算」 | identity 也按压缩比估 |
| `loading.test.mjs`「不认识的编码按保守的压缩比估」 | 不认识的编码不估 |
| `load-progress.test.mjs`「数收到的字节，最后发 done」等 | 去掉最后的 `done` |
| `nge-core` `frame::tests::gate_keeps_only_the_latest_value_of_each_prop` | 属性不去重 |
| `nge-core` `frame::tests::dt_after_resuming_does_not_count_the_gap` | 恢复时不重置计时 |
| `nge-core` `frame::tests::device_loss_is_reported_once_and_not_when_we_destroy_it` | 引擎自己销毁也报 |
| `nge-core` `phase::tests::hidden_canvas_skips_steps_and_coming_back_zeroes_the_gap` | 空视口照常画；回来不调 `resumed` |
| `nge-core` `phase::tests::pause_stops_stepping_and_resume_zeroes_the_gap` | 回来不调 `resumed` |
| `nge-audio` `later_gestures_do_not_make_another_device`、`a_new_audio_after_the_old_one_is_gone_can_unlock_again` | `Audio` 释放时不减计数；不看有没有活着的 `Audio` |
| `nge-platform` `input::tests::releasing_command_releases_everything` | 松开 ⌘ 不全部松开 |
| `nge-platform` `input::tests::modifier_keys_themselves_are_forwarded_even_with_command_down` | 按着 ⌘ 时修饰键也不转发 |
| `nge-platform` `input::tests::enter_and_space_on_a_focused_control_are_left_to_it` | 回车、空格不让给按钮 |
| `nge-platform` `loading::tests::length_is_checked_only_when_it_means_the_body_size` | 压缩时也比长度；不比长度 |
| `nge-render2d` `post::tests::only_the_screen_texture_is_drawn_through_its_srgb_view` | 按格式猜（原来的写法） |
| `nge-scene`（`--no-default-features`）`disabled::tests::a_disabled_feature_the_scene_uses_is_reported_once` | 报过不记下 |
| vendor `native::tests::an_unregistered_name_warns_once` | 每次都报 |
| vendor `native::tests::the_selector_is_scoped_to_the_player` | 选择器不限在播放器的根下 |
| vendor `native::tests::create_props_command_and_delete_run_in_order`、`the_same_id_deleted_and_made_again_is_a_new_component` | 删除不生效 |
| `sanguo` `pace.test.tsx`「引擎没画出第一帧之前不推进」 | 不看 `ready` |
- `nsh package --target web` 成功；用静态服务器起起来，无头 Chrome（开 WebGPU）：出完整场景、改大小画布跟着变；按住 → 角色走、J 出招，游戏用的键不再往下传，焦点在输入框里时按键照常给输入框；第一次按键后建了 WebAudio 并且有非零的采样；不指定字体的「!」和性能数字画得出来；限速时加载页显示下载进度，下载失败显示「重试」，手机宽度（375）不横向滚动。

## 还没手测

- Safari、Firefox；手机浏览器（触屏没有按钮，玩不了）。
- 真机上听声音、按键手感、掉帧时 WebAudio 爆不爆音。
- 切走标签页再回来（无头浏览器里没测）。

## 修过的问题

web 移植做完后又找出、改掉的：

1. 加载页的下载进度会往回跳：压缩比估小了时收到的超过了估计的总数，`identity` 和不认识的编码总数是 0。现在只增不减，超过时调大总数，`identity` 按原始大小，不认识的按保守的比例估。
2. 加载期间属性越攒越多：等 GPU 时同名属性只留最后一次。引擎画出第一帧发 `ready`，长坂坡之前不推进游戏，加载完时不会已经打起来了。
3. 游戏视图删掉再建后声音解不了锁：现在跟着活着的 `Audio` 判断；之后的手势会把没在跑的音频设备拉起来；脚本发的假事件不算手势。
4. macOS 的浏览器里按着 ⌘ 再按别的键，那个键会一直按着（收不到 `keyup`）：松开 ⌘ 时全部补发松开。
5. 切走标签页时声音还在放：页面隐藏时挂起音频设备，回来恢复。
6. 连接断了时可能拿到半截的资源交给解码器：没压缩的响应核对 `Content-Length`。web 上解码 panic 会让整页停掉，写进了文档。
7. 按着 ⌘ 时修饰键自己的按下也被挡掉了；AltGr（Ctrl+Alt）被当成快捷键。
8. 焦点在按钮、链接上时回车、空格被游戏吃掉，按不了按钮。
9. 取资源出错时错误信息是 `{}`：现在用 `Error.message`。
10. 资源进度按发请求的顺序数，前面一个慢后面全卡着：现在按完成的先后。
11. `Gfx::target_format` 按格式猜哪张是画面，格式碰巧对得上的普通纹理会被当成画面：现在引擎每帧告诉它画面是哪张（`Gfx::screen`）。
12. GPU 设备丢了时静悄悄地不画了：现在发 `ENGINE_ERROR`。
13. 加载页退场后再出错看不到：现在退场只是藏起来，出错时重新显示。
14. `nsh.js` 里 `createPlayer` 的注释被挤开了，放回原处。
15. `ComponentPropSlots` 不支持，写进了文档。
16. 组件容器的选择器没限在播放器的根下，一个页面有几个播放器时会找错。
17. `tools/check-wasm.sh` 单独编一次 `sanguo`；验收加上 `cargo test -p nge-scene --no-default-features`，并测 feature 关掉时只报一次。
18. 画布藏起来（大小是 0）时还在更新：现在不更新不画，回来时不把中间这段算进 `dt`。
