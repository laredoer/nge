# M3：TSX 绑定

> 前置：M2 已完成（Rust 侧类型化通道、`game_view`）。

## 目标

用 TSX 写游戏界面：`<GameView>` 组件、声明式场景（TSX 描述精灵，Rust 端按 id 增量同步进 ECS）、TS 类型，以及热更新。

**验收标准**：开着 dev host 改 `App.tsx` 里的场景（增删精灵、改位置 / 颜色），保存后场景随之更新，进程不重启。

## 调研结论（nsh 0.3）

| 问题 | 结论 |
|---|---|
| TSX 怎么用原生组件 | `nativeComponent<P, E>("nge.game", { props, events })`；prop 是 JSON，变化时重发；事件处理器名为 `on${Capitalize<K>}`；`ref.command(name, args)` 发 command |
| prop 是否响应式 | 是。`difficulty={diff()}` 在 signal 变化时重发（实测） |
| `nsh dev` 自带的 host 能否注册平台组件 | 不能。生成的 `macos-host/main.rs` 只调 `set_ui_sets` 和 `nsh_macos_host::main()`，组件注册表属于 `AppOptions`，没有全局入口 |
| 解法 | 自己写 dev host：`AppSource::DevServer` + `AppOptions.components` 注册 `nge.game`，配合 `nsh dev --server-only` |
| 能否 import app 目录外的 TS | 能（`../../ts/...` 实测可打包） |
| 热更新时原生组件的去留 | **重建**。每次 rebuild 都会重新挂载 `nge.game`，引擎跟着重建，游戏内状态归零（实测 hud 分数 59 → 0），进程不重启 |
| 原生组件上的触摸 | 原生组件的 NSView 要在 `hitTest:` 返回 nil，把鼠标让给节点视图，`onTouchStart` 等 ViewProps 才收得到（同 Rust 的 `game_view`；M4 时才发现并补上，之前 Mac 上的触摸其实没生效） |

热更新会重建引擎，所以**场景必须完全由 TSX 声明**：重建后新引擎收到同一份 `scene` prop，画面即恢复到声明的状态；只有运行时产生的状态（比如 `spin` 转到的角度、Rust 游戏逻辑里的分数）会归零。跨热更新保留引擎是后续优化（见「已知限制」）。

## 新增内容

- `crates/nge-scene`：声明式场景。`ScenePlugin` 注册 `scene` prop，按 id 把节点同步成 ECS 实体；命名纹理；`tap` 事件。
- `crates/nge-platform`：`macos::dev_host(factory)`，自写 dev host 的入口。
- `ts/nge.tsx`：TS 库。`gameView()` 生成 `<GameView>` 组件、场景类型、触摸转发。
- `examples/scene`：M3 验收示例（TSX 场景 + HUD）。

## 场景协议（`scene` prop）

```ts
interface Scene {
  background?: string;          // "#rrggbb"，清屏色
  camera?: { x?: number; y?: number; zoom?: number };  // 跟随、边界见 camera.md
  timeScale?: number;           // 时间缩放，0 暂停，见 time.md
  sprites: SpriteNode[];
}
interface SpriteNode {
  id: string;                   // 必填，按它做增量同步
  x?: number; y?: number; z?: number;   // 世界坐标（point，y 朝上，原点在视口中心）
  rotation?: number;            // 角度（度，逆时针）
  scale?: number;
  w?: number; h?: number;       // 不填按纹理像素大小
  color?: string;               // "#rrggbb" / "#rrggbbaa"，sRGB，乘到纹理上
  texture?: string;             // Rust 侧注册的纹理名；不填是纯色矩形
  frame?: number;               // 图集格子序号（纹理注册成图集时）
  spin?: number;                // 每秒转多少度（引擎每帧推进）
  tap?: boolean;                // 可点击：按下时发 tap 事件
}
```

- 角度对外用度，进 ECS 时换成弧度；颜色按 sRGB 解析后转线性。
- `tap` 事件：`{ id, x, y }`，x / y 是世界坐标。多个可点击精灵重叠时取 z 最大的。

## 同步规则（Rust）

- 每个节点对应一个实体，带 `SceneId(String)`。Rust 游戏逻辑可以按 id 查询这些实体，TSX 声明场景、Rust 写逻辑。
- `scene` 变化时（PreUpdate）：新 id → spawn；已有 id 且节点内容变了 → 整体覆盖 `Transform2d` / `Sprite` 等组件；不在新场景里的 id → despawn。节点没变就不碰，`spin` 推进的角度得以保留。
- 重复的 id 只保留第一个，记 warn。
- 未知的纹理名用白色纹理，记 warn。
- 相机和背景：场景自己维护一个 `Camera2d` 实体，游戏不要再放别的相机（渲染只用第一个）。和精灵一样，`camera` / `background` 的声明没变就不碰，Rust 系统可以自己驱动相机；声明一变就以声明为准。相机跟随（`follow`）时由场景每帧写相机，震屏期间也会写，见 [camera.md](camera.md)。

## API 形态

Rust（游戏 crate）：

```rust
pub fn factory() -> GameFactory {
    nge_ecs::factory(|| {
        let mut app = App::new();
        app.add_plugins(Render2dPlugin)
            .add_plugins(ScenePlugin::new().atlas("ships", include_bytes!("../assets/ships.png"), Grid { cols: 4, rows: 4 }));
        app
    })
}

// dev host：配合 `nsh dev --server-only` 热更新
fn main() -> ExitCode {
    nge_platform::macos::dev_host(scene::factory())
}
```

TSX：

```tsx
import { gameView, type Scene } from "../../ts/nge";

const GameView = gameView();

function App() {
  const x = signal(0);
  const scene = (): Scene => ({
    sprites: [
      { id: "ship", texture: "ships", frame: 0, w: 64, h: 64, x: x(), tap: true },
      { id: "sun", color: "#ffcc00", w: 40, h: 40, y: 200, spin: 90 },
    ],
  });
  return <GameView class="flex-1" scene={scene()} onTap={(t) => x.set(t.x + 50)} />;
}
```

`gameView<P, E>({ props, events })` 生成组件时可以追加游戏自己的 prop / 事件名（类型由泛型给出），内置的有 prop `scene`、事件 `tap` / `fps` / `error`，触摸自动转成 `touch` command。

## 运行方式

```bash
nsh dev --server-only            # 在 examples/scene 下，监听 App.tsx
cargo run -p scene               # 自写 dev host，连 ws://127.0.0.1:8790/ws
```

后来改成 nsh 生成的宿主，自写 dev host 已经删掉，现在用 `nsh run examples/scene`（见 [generated-hosts.md](generated-hosts.md)）。

## 任务拆分

1. `nge-scene`：协议类型和解析（颜色、角度），加单元测试
2. 同步系统：spawn / update / despawn、重复 id、相机、背景，加单元测试
3. `spin`、`tap` 命中测试（考虑旋转和缩放），加单元测试
4. `dev_host`
5. `ts/nge.tsx`：类型、`gameView()`、触摸转发
6. `examples/scene`：TSX 场景 + HUD
7. 验收：改 TSX 热更新场景

## 实现和设计的差异

- **`dev_host` 放在 `nge-platform` 的 `dev` feature 里**：打开后才依赖 nsh 的 `dev` feature。注意 cargo 会统一 feature：`cargo build --workspace` 时 `scene` 打开的 `dev` 对其他示例也生效，只有单独 `-p` 构建时才不带 dev server 客户端。
- **`nge-render2d` 导出了 `PointerWorld` 系统集**：场景同步排在它前面，同一帧里改相机再点击，命中用的是新相机。
- **`Assets::sprite_size`、`SpriteImage` 公开了**：点击命中测试要用到精灵的实际大小。
- **只填 `w` 或 `h` 时按正方形**；都不填时，纹理按像素大小，纯色块是 1×1。
- **`gameView()` 包了一层组件**：在原生组件上挂 `onTouch*` 转发触摸，并保留用户自己的 `ref`、`onTouch*`、`onError`；没传 `onError` 时打 `console.error`。

## 验收记录（2026-11-03，Mac）

- **单元测试**：
  - `nge-scene` 19 个：协议默认值和颜色解析 5 个；同步 8 个（字段映射、命名纹理和图集、增删改且复用实体、没变的节点保留运行时状态、重复 id、相机和背景、只改精灵时不碰相机、移除 prop 清空场景）；行为 6 个（旋转加缩放的命中测试、spin、点中 z 最大的可点击精灵、屏幕 y 朝下到世界 y 朝上、同帧改相机后的点击坐标、去掉 tap / spin 标记）。
  - `nge-render2d` 新增 `sprite_size` 1 个；`nge-platform` 新增 dev host 参数解析 3 个。
- **TS 类型检查**：`nsh check` 通过。
- **热更新**：`nsh dev --server-only` 加 `cargo run -p scene`，改 `App.tsx`：
  - 加一个精灵：引擎收到的节点数 10 → 11；陨石环 8 个改成 4 个：11 → 7。
  - host 进程 PID 始终不变，没有重启。
  - 连续热更新 20 次：线程数一直是 11，RSS 从 126MB 涨到 127.5MB，没有累积。
  - 每次热更新都会重建引擎（nsh 重新挂载原生组件），由于场景完全由 TSX 声明，重建后画面恢复到声明的状态。
- **还没验证**（没有屏幕录制权限，也没法模拟点击；复审后重跑时这台机器拿不到可显示的画面，对照组 sprites 同样收不到 fps，所以 fps 事件这次也没看到）：画面是否正确、点飞船能否收到 `tap` 并让它跳到新位置、HUD 上的 FPS。需要人工看一眼：在 `examples/scene` 下跑 `nsh dev --server-only`，另开终端跑 `cargo run -p scene`。
- **iPhone**：范围外。

## 已知限制 / 后续

- **热更新会重置游戏状态**：nsh 每次 rebuild 都重新挂载原生组件。可以考虑按组件 key 缓存引擎、在热更新时复用，但需要 nsh 告诉我们这是一次热更新而不是真的卸载。
- **Rust 侧日志看不到**：nsh 的 DevLogger 只转发 `nsh*` 和 `js` 目标，以及 warn 以上级别的日志，引擎的 info / debug 日志在 dev host 里不显示。
- **每次整体下发场景**：节点多、或者 TSX 每帧都改场景时，JSON 序列化和全量比较的开销会上来。到时候改成 TS 侧 diff、发增量补丁。
- **改了节点就整体覆盖**：节点上任何字段一变，`Transform2d` / `Sprite` 都按声明重置，`spin` 转到的角度也会回到声明的 `rotation`。以后可以做成按字段覆盖。
- **整份场景要么全收、要么全拒**：任何一个字段不合法（比如 TSX 算出 `NaN`，`JSON.stringify` 后变成 `null`；颜色写错；`frame` 为负），这一整份场景都会被拒绝，旧场景保留，只记一条 warn。未知字段（比如把 `rotation` 拼成 `rotate`）会被静默忽略。
- **同 z 的重叠顺序不保证**：渲染时同 z 再按纹理分组，点击取查询顺序里的最后一个，两者在不同纹理时可能不一致。要确定上下关系就给不同的 z。
- **TS 库没有测试**：`nsh test` 里不会注册原生组件，只能测 handler 的组合，暂时没写。
- **点击只认按下**：拖动、长按、多指手势都没有封装，要自己用 `onTouch*` 或者在 Rust 里读 `Input`。

## 范围外

- iOS 的 TSX dev host（iOS 端同样需要自写 host，放到 M6 前后一起做）。
- `asset:/` 路径的纹理（M5）。
- TS 侧 diff：目前每次整体下发场景，由 Rust 端 diff。节点很多时再改成增量补丁。
