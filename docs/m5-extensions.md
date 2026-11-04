# M5：资源层、文字、粒子

> 前置：M4 已完成。

## 目标

1. **资源层 `nge-assets`**：按 `asset:/` 路径读取 App 自带的文件，在后台线程解码；2D 和 3D 共用同一份贴图（CPU 数据和 GPU 纹理都只存一份）。
2. **文字 `nge-text`**：在游戏画面里画文字（分数飘字、名字牌、提示），字体从资源层加载。
3. **粒子**：2D 粒子发射器（爆炸、拖尾、火花），走现有的精灵批量绘制。

**验收标准**：新示例 `examples/effects`：
- 贴图、字体、glTF 模型全部用 `asset:/` 异步加载，加载完之前画面显示「加载中」，同一张贴图同时用在 2D 精灵和 3D 模型上；
- 点击画面炸出粒子，并在点击处飘出一行文字；
- Mac 上 10k 个粒子同时存活时保持 120 FPS（release）。

## 资源层 `nge-assets`

### 文件从哪来

和 NSH 的约定一致：游戏 crate 的文件放在自己的 `assets/` 目录，用 NSH 的 `asset!("ships.png")` 拿到 `asset:/@<包名>/ships.png`，用 `assets!()` 交给 NSH 的 `Registry::assets`（`nsh run` 会托管这些文件，`nsh package` 会把它们打进包里）。引擎按下面的顺序找文件：

| 运行方式 | 根目录 |
|---|---|
| 打包后的 Mac App | `<App>.app/Contents/Resources/nsh/assets` |
| iOS App | `<bundle>/assets`（和 nsh 的 iOS host 一致） |
| `nsh dev` + 自写 dev host | 开发服务器的 `http://<host>:<port>/assets/`（用 `ureq`，不开 TLS） |
| `cargo run` | `NSH_APP_DIR/assets`；没设时回退到 `assets!()` 记下的源文件路径（编译时的绝对路径） |

平台层（`nge-platform`）决定根目录，交给引擎一个 `AssetSource`（`fn read(&self, uri: &str) -> Result<Vec<u8>, String>`）。nsh 0.3 的 `AssetReader` 是 crate 私有的，没法直接复用，所以照它的规则自己实现一份。

### 加载和句柄

```rust
let ships: Handle<Image> = server.load("asset:/@effects/ships.png");
let font: Handle<Font> = server.load(FONT.uri);
let crate_model: Handle<Gltf> = server.load("asset:/@effects/crate.glb");
```

- `AssetServer`（资源）：`load` 立即返回句柄，读文件和解码在一个后台线程上做；每帧 `PreUpdate` 收回结果，放进 `Assets<T>`。同一个 uri 只加载一次，后面拿到同一个句柄。
- 状态：`server.state(&handle)` → `Loading` / `Loaded` / `Failed(String)`；另外发 `AssetEvent<T>` 消息（`Loaded` / `Failed`），游戏可以用 `MessageReader` 等加载完成。
- 句柄就是编号（`Copy`），不做引用计数，资源加载后一直留着（和 M4 一样只增不减）；卸载放到后面。
- 同步接口保留：`Assets<Image>::add_rgba`、`load_png(bytes)`，测试和 `include_bytes!` 照常用。

### 2D / 3D 共用贴图

- `Image`（CPU 上的 RGBA8 sRGB）放在 `Assets<Image>` 里；`ImageId` 统一成一个类型，`nge-render2d` 的 `Assets` 和 `nge-render3d` 的 `Assets3d` 不再各自存贴图。
- GPU 纹理由 `nge-assets` 的 `GpuImages` 统一创建和缓存（每张图一个 `TextureView`），两个渲染器各自用自己的 bind group 引用同一个视图。
- 加载中的贴图画成白色纹理（2D）或材质的 base color（3D），加载完后自动换上。
- glTF 也走资源层：`.glb` 和 `.gltf`（外部 `.bin` / 贴图按相对 uri 解析）都可以，`Handle<Gltf>` 加载完后得到 `ModelId`。

## 文字 `nge-text`

- 用 `glyphon` 0.12（cosmic-text 排版 + 字形图集，支持 wgpu 30），作为一个独立的渲染器，画在 2D 和 3D 之上（插件最后加）。
- 组件 `Text2d { text, font: Handle<Font>, size, color, anchor, max_width }`，位置取 `Transform2d`（世界坐标，跟着 2D 相机走）；`ScreenText` 标记表示不跟相机，直接用屏幕坐标（HUD 风格）。
- 字体加载完之前这段文字不画；没指定字体时用 glyphon 的系统字体。
- 中文：字体文件由游戏自带（系统字体在 iOS 上不保证能找到中文）。原计划示例带一份开源中文字体的子集，但本机没有字体裁剪工具（fonttools），完整的中文字体又太大，所以示例改用 Inter（只有拉丁字母和数字），画面里的文字用英文，中文提示交给 NSH 的 HUD。
- 文字内容、字体、字号（含相机缩放）、最大宽度都没变就不重新排版。

## 粒子

- 组件 `ParticleEmitter { rate, burst, lifetime, speed, angle, gravity, size: (start, end), color: (start, end), texture }`，发射器跟着实体的 `Transform2d`。
- 粒子在 CPU 上模拟，存在发射器自己的数组里（不是一个粒子一个实体），每帧打成精灵实例交给 2D 批量绘制，所以要给 `nge-render2d` 加一个「额外实例」的入口。
- `burst` 用于爆炸：发射一次后，等所有粒子死完自动删除实体（可关）。
- 粒子数有上限（每个发射器默认 10k），超出时不再发射并记一次 warn。

实际的「额外实例」入口是 `nge-render2d` 的 `SpriteInstances` 资源：每帧 PreUpdate 开头清空，系统在 Update / PostUpdate 里 `push` / `extend`，渲染器把它们和实体精灵放在一起按 (z, 纹理) 合批。

## 任务拆分

1. `nge-assets`：`AssetSource`（目录 / HTTP / 源文件回退）、`AssetServer`、后台加载、`Assets<T>`、`AssetEvent`（单测：同一 uri 去重、失败状态、目录解析）
2. 平台层：Mac / iOS / dev host 决定根目录并交给引擎
3. 贴图统一：`Image` 和 `GpuImages` 下沉到 `nge-assets`，`nge-render2d` / `nge-render3d` 改用它；现有测试全部保持通过（离屏测试：同一张图同时用在 2D 和 3D，加载中显示白色、加载后换上）
4. glTF 改走资源层（单测：外部 `.bin` 和贴图的相对路径）
5. `nge-text`：`Text2d`、`ScreenText`、字体资源（离屏测试：文字画出来、位置跟着相机、字体未加载时不画）
6. 粒子：发射器、模拟、`burst` 自动删除（单测：数量、寿命、上限；离屏测试：粒子画出来）
7. `examples/effects` + 验收（10k 粒子帧率）

## 范围外

- 资源热更新、卸载和引用计数
- 音频、物理、TSX 里声明 3D 节点和文字 / 粒子节点
- 3D 空间里的文字（贴在模型上的名字牌）、3D 粒子
- 富文本（一段里混用多种样式）、文字描边和阴影

## 验收记录（2026-11-04，Mac）

- **单元测试**：`nge-assets` 17 个（uri 校验、来源链、目录 / HTTP / 包目录解析、HTTP 路径转义、去重和同一 uri 的不同写法、失败状态、事件、相对路径里的 `..` 和越出包、解码 panic 不拖垮后台线程、释放时不等排队的加载）；`nge-text` 4 个（锚点、颜色换算、字体文件头、同名字体按字重区分）；`nge-particles` 10 个（随机数范围、默认发射器不自动删除、缺 2D 插件时报错、爆炸只发一次、持续发射攒零头、寿命到了删除爆炸发射器 / 关掉时保留、上限、运动和插值、发射位置）；`nge-render2d` 新增 `world_to_screen` 1 个；`nge-render3d` 新增 glTF 走资源层的若干个（外部 `.bin` 和贴图的相对路径、百分号编码、`load_gltf` 拒收外部文件、加载完后 `GltfModel` 自动展开、缓冲区不够长和 bufferView 越界时报错而不是 panic、`data:` 内嵌贴图）；`nge-platform` 新增 dev 服务器地址换算 1 个；`examples/effects` 4 个（全部加载完且立方体和 2D 精灵用同一张贴图、2D 精灵跟着转、点击炸粒子和飘字、压力测试卡在 10k）。
- **离屏渲染测试**：2D / 3D 共用一张加载完的贴图（加载中画白色 / base color）；`SpriteInstances` 和实体精灵按 z 交错绘制、每帧清空；文字在字体加载完前不画、世界坐标跟着相机平移和缩放、`ScreenText` 不跟；粒子画在发射器的位置上。
- **iOS**：`cargo check --workspace --target aarch64-apple-ios` 通过；没上真机（没连手机）。
- **实跑**：`cargo run -p effects --release`。日志里的帧率：平时 120 FPS；打开压力测试（10k 粒子同时存活，发射器到上限的 warn 出现）后保持 120 FPS，最长帧 8–9 ms。日志里有两次 1–9 秒的停顿，是手动操作窗口造成的，不是引擎卡顿。「加载中」、2D 精灵和 3D 立方体共用贴图、点击炸粒子和飘字，画面都正常。

## 已知限制 / 后续

- 资源只增不减，没有卸载和热更新；句柄没有引用计数。
- `nsh dev` 是否托管本 crate 的 `assets/` 没有实测；`cargo run` 时走 `assets!()` 记下的源文件路径。
- 文字用 glyphon 自己的字体库，和 NSH 界面注册的字体互不相通；没指定字体时第一次会扫描系统字体，有一点耗时。
- 粒子只在世界坐标里模拟，不旋转，大小不乘 `Transform2d.scale`；每帧所有精灵（含粒子）整体稳定排序一次（有一块临时缓冲），10k 量级没问题，再多要按发射器分段。
- 文字只用 `Transform2d` 的位置，不旋转、不缩放。
- glTF 展开后，解码好的贴图在 `Assets<Gltf>` 和 `Assets<Image>` 里各留一份；glTF 引用的贴图和另外用 `server.load` 加载的同一张图不去重。
- 实体的 `GltfModel` 展开后再换成别的句柄不会重新展开，要换模型就换实体。
- 解码时 panic 只让那一个资源失败（后台线程用 `catch_unwind` 兜住），前提是不用 `panic = "abort"` 编译。
