# NSH 原生游戏引擎：设计草案

> 状态：草案（2026-11-03）。代号暂定 `nge`（NSH Game Engine），正式命名待定。

## 定位

- **NSH 的一等公民**：游戏画面就是 NSH 里的一个元素（Rust 的 `game()`、TSX 的 `<GameView>`），和其他 NSH 元素一样参与布局；菜单、HUD 直接用 NSH 写。
- **双语言**：引擎核心和性能敏感的逻辑（system）用 Rust 写；玩法逻辑可以用 TSX 写，复用 NSH 的热重载和签名模块下发（`.nshpkg`）。
- **渲染范围**：第一阶段是 2D 加基础 3D，底层统一用 wgpu（Apple 上走 Metal，Android 上走 Vulkan，Web 上走 WebGPU/WebGL2）。

## 原型已经验证的部分（prototype/ 现状）

| 能力 | 结论 |
|---|---|
| NSH 原生组件嵌入 GPU View | macOS（NSView）和 iOS（UIView）上都能用 wgpu 渲染 |
| 帧驱动 | CADisplayLink 跟随屏幕刷新率：macOS 上 120，iPhone 16 Plus 上 60 |
| NSH → 引擎 | command 和 prop 都可用（JSON），大约有一帧延迟 |
| 引擎 → NSH | `ComponentContext::emit`，可以在任意线程调用 |
| 输入 | NSH 的 `on_touch` 接收全部触摸阶段，macOS 的鼠标也会映射成触摸 |
| 宿主 | 所有示例都用 nsh 0.4 生成的宿主（`#[native_components]`，`nsh run` / `nsh package`）；原来绕过 nsh 0.3 限制的自写宿主（macOS dev host、iOS `ios_app.rs`）已删掉（[generated-hosts.md](generated-hosts.md)） |

## 分层

```
┌───────────────────────────────────────────────┐
│ 游戏（examples/*）    Rust systems / TSX 场景     │
├───────────────────────────────────────────────┤
│ nge-nsh     game() / <GameView> 元素、场景绑定、事件 │  ← 对外 API
│ nge-script  TSX 场景声明 → ECS 的协调器（reconciler）│
├───────────────────────────────────────────────┤
│ nge-core    App、帧循环、时间、ECS、输入抽象          │
│ nge-render  2D 批量绘制、相机、网格/材质/光照、glTF    │
│ nge-assets  资源加载（asset:/）、缓存、热重载         │
│ nge-audio   音效/音乐（kira）                       │
├───────────────────────────────────────────────┤
│ nge-platform  NSView / UIView / SurfaceView、       │
│               display link、渲染线程、surface 生命周期 │
└───────────────────────────────────────────────┘
```

尽量复用成熟的库：ECS 用 `bevy_ecs`（单独使用，支持动态组件，方便 TSX 桥接），数学库用 `glam`，模型加载用 `gltf`，文字用 `cosmic-text`/`glyphon`，音频用 `kira`，物理可选 `rapier`。

## 关键设计决策

1. **线程模型**：引擎跑在独立的渲染线程上，主线程只负责转发帧信号、视口尺寸和输入，避免和 NSH 界面互相拖慢。
2. **TSX 不跑每帧逻辑**：NSH 和引擎之间是异步 JSON 通信，每帧往返代价太高。TSX 只用**声明式**的方式描述场景（`<Sprite>`、`<Mesh>`、`<Camera>`，属性绑定到 signal 或动画值），引擎协调成 ECS 实体；每帧的逻辑放在 Rust system 里。TSX 处理的是事件（`onTap`、`onCollide`），以及关卡、状态机这类高层逻辑。思路类似 react-three-fiber。
3. **批量协议**：场景变更按帧合并成一条 command（只传差异），不按单个实体逐条发送。
4. **输入**：统一成 Pointer 事件（触摸、鼠标、笔），之后再加键盘和手柄；短期内继续走 NSH 的 `on_touch`。

## 里程碑

| 阶段 | 内容 | 验收标准 |
|---|---|---|
| **M0 重构** | 改成 workspace，拆出 `nge-platform` 和 `nge-core`，弹球改为 example，渲染挪到独立线程 | 主线程故意卡 200ms，游戏帧率不受影响 |
| **M1 2D 核心** | ECS、精灵批量绘制、纹理/图集、2D 相机、Pointer 输入，提供 Rust API | 10k 个精灵在 iPhone 上 60fps，在 Mac 上 120fps |
| **M2 NSH 绑定（Rust）** | `game()` 元素、场景和 NSH signal 绑定、事件回传 | 用纯 Rust 写的示例游戏，HUD 用 NSH 写 |
| **M3 TSX** | `<GameView>`（`gameView()`）、声明式场景（`nge-scene`）、TS 类型、热更新（自写 dev host） | 改 TSX 后热更新场景，不重启 |
| **M4 基础 3D** | 网格、深度缓冲、3D 相机、简化 PBR 材质、方向光/点光源、glTF（`nge-render3d`） | 加载并显示一个带贴图的 glTF 模型 |
| **M5 扩展** | 资源层（`nge-assets`，`asset:/` 异步加载，2D / 3D 共用贴图）、文字（`nge-text`）、2D 粒子（`nge-particles`）；音频、物理放到之后 | 贴图 / 字体 / glTF 异步加载，点击炸粒子并飘字，Mac 上 10k 粒子 120fps |
| **TSX 场景节点** | `scene` 里声明 3D 模型、灯光、文字、粒子发射器，`burst` / `floatText` 一次性命令，点击模型（射线拾取）发 `tap`；见 [scene-nodes.md](scene-nodes.md) | `examples/scene3d`：场景全写在 `App.tsx` 里，点箱子炸粒子、飘字、加分 |
| **2D 物理** | `nge-physics2d`（rapier2d）：刚体、碰撞体、碰撞消息，射线和重叠查询；精灵节点的 `body`、`collide` 事件、`push` command；见 [physics2d.md](physics2d.md) | `examples/physics`：箱子堆叠、点击生成弹球、碰撞计数，1000 个刚体一步 < 2 ms |
| **音频** | `nge-audio`（kira）：`asset:/` 加载音效 / 音乐，播放、循环、音量、同时发声上限；`scene.audio` 声明背景音乐，`sound` command；见 [audio.md](audio.md) | `examples/physics` 有背景音乐、撞击声、点击声和静音按钮 |
| **动画** | 精灵的帧动画（`anim`）和补间（`tween` command，缓动、delay、repeat / yoyo、接管属性），`animEnd` / `tweenEnd` 事件；见 [animation.md](animation.md) | `examples/scene`：石头按帧换颜色，点飞船补间移动并缩放弹一下，到达后 HUD 计数 |
| **键盘（Mac）** | AppKit 本地事件监听，键名用 Web `KeyboardEvent.code`；`Input` 的按着 / 刚按下 / 刚松开 / `axis`，TSX 的 `onKey`，`captureKeys`；见 [keyboard.md](keyboard.md) | `examples/dodge` 用 ← → / A D 控制，`examples/scene` 用方向键移动飞船；输入框打字、切走窗口时不卡键 |
| **相机（2D）** | `scene.camera` 的 `follow` / `smooth` / `bounds`，`shake` command；见 [camera.md](camera.md) | `examples/scene`：相机平滑跟着飞船、到边上停住，点飞船时震屏 |
| **暂停和时间缩放** | `Time.scale` / `real_delta`，`scene.timeScale`；见 [time.md](time.md) | `examples/scene`：空格暂停 / 继续，暂停时补间、动画、旋转都停住 |
| **瓦片地图** | `scene.tilemaps`：按图集格子画，实心格子合并成静态碰撞体，`collide` 报地图 id；见 [tilemap.md](tilemap.md) | `examples/physics`：地面、墙和两块平台是瓦片地图，球和箱子落在上面 |
| **平台跳跃示例** | 物理加接触查询（`Physics::contacts`）和锁旋转（`fixedRotation`）；Rust 角色控制 + TSX 关卡，合成的音效，按 R 重新开始，巡逻的敌人（踩头消灭）；见 [platformer.md](platformer.md) | `examples/platformer`：键盘跑跳、相机跟随、吃金币、踩敌人、掉坑重来、到终点通关，有音效、R 重新开始 |
| **手柄（Mac）** | GameController 每帧读一遍：按钮和摇杆方向当成 `Gamepad*` 键名走 `key`，摇杆模拟量走 `gamepad` command 进 `Input::stick`；见 [gamepad.md](gamepad.md) | `examples/platformer` 用十字键 / 左摇杆跑、A 跳；`examples/dodge` 按摇杆推的力度移动 |
| **移动平台和渲染插值** | `DrawAt`：动态 / kinematic 刚体画在插值过的位置，`Transform2d` 保持真实值；`Physics::velocity`；平台跳跃的横移 / 升降平台；见 [moving-platforms.md](moving-platforms.md) | `examples/platformer`：站在移动平台上跟着走、不陷不抖，120 Hz 下每帧都动 |
| **调试绘制** | `Gizmos`（即时模式画线、框、圆，放进 `SpriteInstances`）；`PhysicsDebug` 画出所有碰撞体轮廓；见 [gizmos.md](gizmos.md) | `examples/physics` 的「查询」和平台跳跃的 Q 调试视图改用 `Gizmos`，Q 同时画出所有碰撞体 |
| **nsh 0.4** | 依赖升到 0.4，TSX 的 `YNode` 换成 `NativeComponentRef`；crates.io 上 `nsh-platform-ios` 编不过，暂时从 git tag 取；见 [nsh-0.4.md](nsh-0.4.md) | 测试、iOS 检查、`nsh check` 通过，示例连 0.4 的 dev server 能热更新 |
| **生成的宿主** | 0.4 的 `#[native_components]` 把游戏视图注册进 `nsh run` / `nsh package` 生成的宿主；iOS 字体、包内资源、GPU 上限、重签脚本；见 [native-components.md](native-components.md) | 平台跳跃用 `nsh run` 在 Mac 上跑、用 `nsh package` 在 iPhone 真机上跑 |
| **触屏按钮** | `KeyButton` 按住发 `key` 命令，和键盘走同一条路；见 [touch-controls.md](touch-controls.md) | 平台跳跃在 iPhone 上能边跑边跳 |
| **2.5D 相机** | `Camera3d::ortho` 正交投影，`camera3d.match2d` 让 3D 相机跟着 2D 相机；见 [camera25d.md](camera25d.md) | 平台跳跃终点前的 3D 箱子随相机跟随、震屏始终和关卡对齐 |
| **示例换成生成的宿主** | 其余 8 个示例照平台跳跃改用 `#[native_components]`，删掉 dev host、`ios_app`、`HttpSource`；iOS 的前后台暂停改成组件自己监听通知；见 [generated-hosts.md](generated-hosts.md) | 每个示例 `nsh run` 能跑、`nsh check` 通过 |
| **视差背景** | 精灵和瓦片地图的 `parallax`（一个数或 `[x, y]`），渲染和点击都按画的位置；z < 0 的精灵画在 3D 后面；见 [parallax.md](parallax.md) | 平台跳跃的远山和树按不同速度跟着相机，3D 箱子在背景前面 |
| **关卡切换和存档** | 场景的 `level` 换了值时实体全部重建、清掉一次性效果、相机直接对准，Rust 收到 `LevelStart`；TSX 的 `saved()` 把 signal 存在设备上；见 [levels.md](levels.md) | 平台跳跃两关，通关进下一关，数字键选已解锁的关，最好成绩重开后还在 |
| **Tiled 地图导入** | `ts/tiled.ts` 的 `tiled()` 把 Tiled 的 JSON 地图转成瓦片地图和对象列表，实心、z、视差按属性；不支持的地图报错并说怎么改；见 [tiled.md](tiled.md) | 平台跳跃第三关「云端」是 Tiled 地图，对象层摆金币、敌人、移动平台 |
| **背景重复** | 精灵的 `repeat`（`"x"`、`"y"`、`"xy"`）沿世界的轴一个挨一个铺满视野，和视差一起用；只画和视野相交的几份；见 [repeat.md](repeat.md) | 平台跳跃的远山换成一张山丘贴图横向重复，跑到哪都有 |
| **iOS 手柄和硬件键盘** | iOS 的 display link 每帧读 GameController：手柄和 Mac 共用读数和合并，硬件键盘按 HID 用法码换成同一套键名；按着 ⌘ 的不发，进后台补发松开；见 [ios-input.md](ios-input.md) | 平台跳跃在 iPad 上用键盘或手柄玩，数字键选关 |
| **关节和 2D 刚体带动 3D 模型** | `nge-physics2d` 的 `Joint`（焊死、转动、滑动、绳子、弹簧，带角度范围和马达），TSX 的 `scene.joints` 按精灵 id 连；模型节点的 `attach` 跟着精灵的位置和角度走；调试视图画关节；见 [joints.md](joints.md) | 平台跳跃有一串甩着的铁链吊球，终点前的 3D 箱子能推着走、跳上去 |
| **2D 特效** | 着色器库（内置 flash、outline、wave、circle 和 grayscale、vignette、pixelate，TSX 的 `scene.shaders` 写 WGSL）；精灵的 `shader`、`params`，模板遮罩（`mask`、`masked`），2D 点光源和环境光（`scene.lighting`），后处理链（`scene.post`）；编不过的着色器报错不崩；见 [effects2d.md](effects2d.md) | 平台跳跃的金币扫过亮光，被撞回去时闪白，终点上方有圆形传送门；第三关是夜里，角色带灯、四角压暗 |
| **3D 补齐** | 方向光阴影（`shadows`，阴影图 + 3×3 PCF）；glTF 蒙皮和骨骼 / 节点动画（`Animator`，GPU 蒙皮，模型节点的 `animation`）；`camera3d.interleave` 让模型和精灵按深度混排；见 [3d-more.md](3d-more.md) | 平台跳跃终点挂着挥动的 3D 旗子，角色从布后面跳过；箱子顶上的小箱子投下影子 |
| **管线预热** | 着色器库到位后后台线程先编好所有 2D 精灵管线和后处理管线，渲染线程拿现成的，没编好的照常当场编；见 [pipeline-warmup.md](pipeline-warmup.md) | 平台跳跃第一次闪白、进传送门、第三关开暗角时不再卡一下 |
| **性能面板** | 引擎记下每帧更新、渲染、等待各花多少毫秒，以及 draw call、实例数、实体数（`Perf`，`Game::timing`），左上角的面板每半秒更新（`PerfOverlay`，TSX 的 `scene.perf`）；见 [perf-overlay.md](perf-overlay.md) | 平台跳跃按 P 开关性能面板 |
| **开发版也开优化** | `.cargo/config.toml` 让 `nsh run` 和 `cargo test` 的依赖全开优化、自己的 crate 轻度优化；帧时间基准 `frame_bench`；见 [fast-dev.md](fast-dev.md) | 基准场景一帧的更新从 6.6 ms 降到 0.19 ms，改一行引擎代码重编约 3 秒 |
| **寻路和 AI** | 新 crate `nge-ai`：格子 A*（不切墙角、路径拉直）、小状态机 `Fsm`、追击 `Chaser`（看见就绕墙追，追丢了回家）；瓦片地图自带 `NavGrid`，精灵写 `chase` 就会追，状态变了发 `chase` 事件；见 [ai.md](ai.md) | 平台跳跃前两关的幽灵绕开平台追角色 |
| **角色控制器** | `nge-physics2d` 的 `Character`（底下是 rapier 的 `KinematicCharacterController`）：游戏只给速度，引擎负责重力、沿墙滑、爬坡、贴地、上矮台阶、被脚下的平台带着走、推箱子；场景写 `body.kind: "character"`；见 [character.md](character.md) | 平台跳跃的角色换上它，第一关加一段斜坡 |
| **3D 物理** | 新 crate `nge-physics3d`（rapier3d）：盒子、球、胶囊刚体，碰撞消息、接触、射线；模型节点写 `body` 就有刚体，`collide`、`push` 对模型也管用；见 [physics3d.md](physics3d.md) | 平台跳跃第一关的一摞 3D 箱子，角色跑过去撞散 |
| **音频补齐** | 混音分组（音乐、音效、界面）各自的音量、低通、混响；带位置的声音按离相机的距离衰减、按左右定声像；`scene.audio.buses`、`sound` 的 `bus` / `x` / `y`；见 [audio-more.md](audio-more.md) | 平台跳跃夜里那关的音效有回声，幽灵发现角色时从它那儿响一声 |
| **世界里的 UI** | `bars`：跟着精灵的进度条，值平滑过渡；`bubbles`：背景框按文字撑开的气泡，能固定在屏幕上当对话框；`typing` 打字机效果，打完发 `typed`；`Text2d.visible`、`TextBox`；见 [world-ui.md](world-ui.md) | 平台跳跃角色头顶的血条（失手三次整关重来）、每关开始时的关卡名气泡 |
| **存档槽** | `saveSlots()`：几个槽各存一份带版本号、保存时间、概要的数据；读档时按版本一级一级升级，坏档报成 `broken` 不崩；能接过 `saved()` 的旧存档；见 [saves.md](saves.md) | 平台跳跃三个存档位，按 S 换，以前的进度自动进第一个 |
| **本地化** | `i18n()`：每种语言一份文案表，`{参数}`、按语言的复数规则，默认跟系统语言、找不到时退到同语言别的地区和默认语言，换语言界面自己刷新；`missing()` 查缺的文案；见 [localization.md](localization.md) | 平台跳跃中英文两套文案，按 L 切 |
| **压缩纹理** | 贴图可以是 KTX2：ASTC、BC1 / BC3 / BC7、ETC2，带 mipmap；设备支持就直接上传压缩数据，不支持在 CPU 上解成 RGBA8；`ktx2` 读写、PNG 转 KTX2（BC3）的小工具；见 [compressed-textures.md](compressed-textures.md) | 平台跳跃的远山换成 BC3 的 KTX2 |
| **3D 粒子和拖尾** | `ParticleEmitter3d`：方向锥里发射、朝着相机的方片，透明（从远往近）或叠加；`Trail3d`：跟着实体的带子，越老越细越淡；`Billboards` 在网格之后画，被模型挡住、不写深度；场景的 `emitters3d` 和模型的 `trail`；见 [particles3d.md](particles3d.md) | 平台跳跃第一关箱子旁的篝火，被撞飞的箱子拖尾巴 |
| **2D 渲染补齐** | `NormalMap`：法线图让受光的精灵按表面朝向明暗（灯有 `height`）；`Occluder2d`：精灵的矩形或多边形挡住开了 `shadows` 的灯，硬阴影；后处理里的 `bloom` 是内置的多遍效果；`PostEffect.layer` 可以只给 z < 0 的背景层；见 [render2d-more.md](render2d-more.md) | 平台跳跃每关有 bloom 和远景模糊，夜里的石碑有凹凸、挡住角色的灯 |
| **3D 渲染补齐** | 材质 `blend` 的半透明网格按到相机的距离由远到近画；`SpotLight` 聚光灯；聚光灯、点光源（六面）能投影子，方向光可以分 2–4 段级联；`Animator::play(clip, fade)` 淡入淡出；glTF 的变形（morph target）和 `weights` 动画；见 [render3d-more.md](render3d-more.md) | 平台跳跃第一关有照着箱子堆、投影子的聚光灯，一只半透明的果冻靠近时从 idle 淡到 squish |
| **Web** | `wasm32` 上浏览器主线程用 `requestAnimationFrame` 驱动引擎，只用 WebGPU；仓库里改过的 `nsh-platform-web` 支持原生组件；资源启动前取进内存，键盘、声音（第一次按键后出声）；见 [web.md](web.md) | `nsh package --target web` 打出的长坂坡在浏览器里能玩 |
| **联网** | `nge-net`：WebSocket 连接（原生 tokio-tungstenite 在单独线程、浏览器 `WebSocket`），断线指数退避重连；postcard 协议；`NetSync` 每秒 20 次广播位置，`Remote` 缓冲 100 ms 插值、容忍乱序丢包；`Http` 的 GET / POST（原生 ureq、浏览器 fetch）；`nge-relay` 中继（房间、进出通知、心跳、人数 / 大小 / 频率上限，顺带排行榜）；场景的 `net`、TSX 的 `netRoom()` / `httpClient()`；见 [networking.md](networking.md) | 平台跳跃两个窗口同一关互相看见（半透明、头顶名字），N 开关，中继连不上照常单机，通关成绩进排行榜 |
| **编辑器** | `examples/editor`：用 nge 做的关卡编辑器，画布就是平台跳跃的游戏视图（编辑时 `timeScale: 0`、不放角色和刚体）；带版本号的关卡文件格式 `ts/level.ts`（读入校验、保存格式化）；命令模式的撤销重做、点选 / 框选 / 拖动 / 吸附、瓦片刷、复制粘贴、属性校验都是纯函数；⌘R 用当前关卡试玩，Esc 回来；见 [editor.md](editor.md)。补齐（关卡文件第 2 版，第 1 版自动迁移）：多个瓦片层（层面板、视差、z、显隐、锁定）、矩形和油漆桶、改瓦片规则（单向、斜坡、动画、摩擦弹性）；实体旋转和 z、分组和父子层级（场景树缩进、折叠、拖行改父节点，挪 / 删 / 复制带子孙）；走地怪的参数和可拖的路线点、预制体实例和覆盖；悬停高亮、边上自动滚动、缩放以指针为中心；试玩时改关卡就地更新（瓦片走 `setTiles`）；见 [editor-more.md](editor-more.md) | 平台跳跃第二关「石阶」从关卡文件读，编辑器里能打开、改、保存、直接试玩；石阶有远景木架层、路牌分组、树的预制体，试玩时改了关卡不用重开 |
| **M6 Android** | 仓库里改过的 `nsh-platform-android` 支持 Rust 写的原生组件（`SurfaceView`），wgpu 只走 Vulkan，画面没了暂停、回来换新的 surface；Android 键码和手柄的轴换成和 Mac 一样的键名；不用 Gradle，`tools/nsh-android.sh` 编好 Java 运行时再交给 `nsh package`；Vulkan 上的管线存到磁盘（`codeCacheDir`），坏了、版本不对当没有；见 [android.md](android.md) | 平台跳跃在 Android 模拟器上能玩，第二次启动读到管线缓存 |
| **输入映射** | 动作绑定（`Actions`，按钮 / 一维 / 二维轴，Rust 每帧问、TSX 收 `action` 事件）；`inputMap()` 改键（按任意键、冲突、恢复默认、存在设备上）；手柄按设备分（`pad` command，`Input::pad`，每个玩家一个手柄，接入 / 拔出事件）；扳机模拟量；手柄震动（`Rumble`，GameController haptics / Android `VibratorManager` / `vibrationActuator`）；浏览器的手柄；见 [input-actions.md](input-actions.md) | 平台跳跃的操作全走动作，有改键面板，第二个手柄接入时出现第二个角色，落地、受伤时震动 |
| **触屏控制** | `<VirtualStick>`（浮动 / 固定、死区、半径、八 / 四方向，按动作发）；手势识别的纯函数状态机（轻点、长按、滑动、拖动、双指捏合和旋转，`gestureHandlers`）；`<ActionButton>` 的按住连发、冷却显示、按下反馈；引擎认直接驱动的一帧内重按；见 [touch-input.md](touch-input.md) | 长坂坡在手机上用摇杆和按钮能玩；平台跳跃的 ← → 换成摇杆、双指缩放镜头；编辑器触控板捏合缩放 |
| **动画状态机** | 新 crate `nge-anim`：和骨骼无关的状态机（状态、转换、bool / float / trigger 参数、退出时间、任意状态、默认状态），一维 / 二维混合树和相位同步，分层（覆盖、叠加、骨骼遮罩），叠加动画，动画事件（`AnimEvent` 消息、TSX 的 `animEvent`）；3D 按输出混姿势，2D 按输出选精灵帧；节点的 `animGraph` / `animParams`、`animSet` command；见 [anim-graph.md](anim-graph.md) | 平台跳跃的主角由状态机驱动待机、走、跑、跳、落、落地，脚步声和落地声是动画事件；果冻改用状态机 |
| **3D 玩法基础** | `Character3d` 角色控制器（爬坡和坡度上限、上台阶、贴地、被平台带着走、沿墙滑，`CharacterState3d` 给着地、地面法线、实际速度、碰撞列表）；`Joint3d` 关节（焊死、球窝、铰链带限位和马达、滑轨、绳子、弹簧）；`MeshCollider` 从 glTF 网格生成凸包、按图元的凸包、三角网格；空间查询（球、胶囊、盒子的形状投射，重叠，点，最近点，碰撞分组过滤）；`Gizmos3d` 调试画线（线、盒、球、胶囊、坐标轴，被模型挡或画在最前）和 `Physics3dDebug`；场景的 `kind: "character"`、`shape: "convex" \| "hulls" \| "trimesh"`、`joints3d`、`query3d`；见 [gameplay3d.md](gameplay3d.md) | 平台跳跃按 G 进「3D 试验场」：3D 角色走斜坡和台阶、推开铰链门、撞链子吊着的球、推凸包石头、站上移动平台，Q 开调试线 |
| **2D 玩法补齐** | 单向平台（碰撞体的 `one_way`：角色控制器查询时过滤、动态刚体走 rapier 的接触修改钩子，`drop_through` 穿下去；瓦片的 `oneWay`、精灵刚体的 `oneWay`）；瓦片斜坡（`Shape::Slope`，45° 和 22.5°，一条直线上的坡连同下面的实心格并成一块，相接处不卡）；动画瓦片（只换那些格子的帧）、每种瓦片的摩擦和弹性、点地图报格子；`nge-ai` 的走地面寻路（站的地方、走 / 落 / 穿 / 跳的连线，跳按跳高跳远算、碰头时试矮的）和行为树（顺序、选择、反应式、并行、取反、重复、冷却、等待、黑板）；精灵的 `behavior`、`blackboard` command、`behavior` 事件；见 [gameplay2d.md](gameplay2d.md) | 平台跳跃第二关「石阶」有单向木板、45° 和缓坡、水和火把；走地怪巡逻，看见角色就追，跳上单向木板、落下来，追丢了回去；↓ 加跳穿下单向木板；编辑器能刷新瓦片 |
| **场景和资源流程** | 预制体（场景的 `prefabs` / `instances`，`asset:/` 的 JSON 文件也行，平移、按子节点覆盖、嵌套、查循环）；异步预加载（`LoadGroup`、`preload` command 和事件、TSX 的 `preloader()`）；换关过场（`scene.transition`：淡黑、擦除，过场里按住新场景、预加载、最短时长，挡住输入）；PNG / JPEG 上传后在 GPU 上生成 mip 链（线性空间、按透明度加权），`scene.textures` 按贴图关掉；见 [scene-flow.md](scene-flow.md) | 平台跳跃换关有淡黑和擦除两种过场，过场里预加载、按键不响应；远处的树是嵌套的预制体；主角图集关掉 mipmap |
| **3D 画质** | 天空（立方体贴图：6 张 PNG 或 KTX2；程序天空：渐变加太阳），从天空烘焙 SH9 漫反射和预滤波 mip 链 + BRDF LUT 镜面反射；距离雾、高度雾（可取天空色，网格、半透明、粒子都受影响）；泊松 PCF / PCSS 软阴影、bias 和法线偏移；多盏聚光灯、点光源按优先级投影（`ShadowPriority`）；级联交界混合和调试色；场景的 `sky`、`fog`、`shadows`；见 [render3d-quality.md](render3d-quality.md) | 平台跳跃试验场（按 G）有天空和环境光、雾、PCSS 软阴影、投影的点光源和聚光灯、带过渡的三段级联，按 C 看级联调试色 |
| **3D 粒子补齐** | 发射形状（球、半球、盒子、圆环、锥形、网格表面，沿法线 / 方向锥 / 随机）；序列帧（按寿命 / fps、随机起始、帧间混合）；朝向（朝相机、沿速度拉长、水平、固定）；大小、颜色、透明度、速度曲线，阻尼和 curl 湍流；软粒子（GL 上退成硬边）；地面和场景碰撞（反弹、死亡，查询有上限）；子发射器（出生、死亡、碰撞时 burst，层数和总数有上限）；受光；缓冲复用、粒子总数软上限进性能面板；见 [particles3d-more.md](particles3d-more.md) | 平台跳跃的篝火有序列帧火苗、拉长的火星落地溅起；试验场（按 G）有烟花、地面冲击波、软粒子的烟 |
| **大场景性能** | 瓦片地图按块（默认 32 × 32）缓存顶点、按每个相机的视野裁剪（视差、旋转、缩放），改一格只重建那一块（画面、碰撞体、导航、点格子），动画瓦片只改 uv，地面寻路图后台重建；2D 精灵按视野、3D 网格按视锥裁剪，看不见的粒子发射器不上传；时间戳查询量各 pass 的 GPU 耗时（不支持时显示不支持）；性能面板加 GPU、裁剪统计和最近 120 帧的曲线，`scene.perf` 的 `graph` / `events`，`setTiles` command；见 [big-scene-perf.md](big-scene-perf.md) | 平台跳跃按 B 进 2000 × 80 的大地图，点格子挖 / 填，按 O 开曲线；2000 × 200 的地图改一格、镜头移动一帧在阈值以下 |

## 待定

- 正式名称和 crate 前缀
- 是否给 nsh 上游提 PR：iOS `run_app` 支持注册组件，以及能否从原生层直接拿到输入以降低延迟
- 工具链的其余部分（资源打包、编辑器里编辑 Tiled 地图）：关卡编辑器已经有了（见 [editor.md](editor.md)、[editor-more.md](editor-more.md)），这些等有需要再做
