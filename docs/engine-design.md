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
| 已知缺口 | nsh 0.3 iOS 的 `run_app` 不支持注册组件，现在用自建的宿主（`ios_app.rs`）绕过；0.4 的 `#[native_components]` 能注册进生成的宿主，平台跳跃已经换过去（[native-components.md](native-components.md)） |

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
| **M6 Android** | SurfaceView 和 Vulkan，JNI 胶水层 | 示例游戏在 Android 真机上能跑 |

## 待定

- 正式名称和 crate 前缀
- 是否给 nsh 上游提 PR：iOS `run_app` 支持注册组件，以及能否从原生层直接拿到输入以降低延迟
- 编辑器和工具链（场景编辑、资源打包）放在 M3 之后再评估
