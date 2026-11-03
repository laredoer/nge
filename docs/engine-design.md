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
| 已知缺口 | nsh 0.3 iOS 的 `run_app` 不支持注册组件，现在用自建的宿主（`ios_host.rs`）绕过 |

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
| **M3 TSX** | `<GameView>`、场景协调器、TS 类型、热重载 | 改 TSX 后热更新场景，不重启 |
| **M4 基础 3D** | 网格、深度缓冲、3D 相机、简化 PBR 材质、方向光/点光源、glTF | 加载并显示一个带贴图的 glTF 模型 |
| **M5 扩展** | 文字、粒子、音频、物理（可选） | — |
| **M6 Android** | SurfaceView 和 Vulkan，JNI 胶水层 | 示例游戏在 Android 真机上能跑 |

## 待定

- 正式名称和 crate 前缀
- 是否给 nsh 上游提 PR：iOS `run_app` 支持注册组件，以及能否从原生层直接拿到输入以降低延迟
- 编辑器和工具链（场景编辑、资源打包）放在 M3 之后再评估
