# M2：NSH 绑定（Rust）

> 前置：M1 已完成。

## 目标

M1 之前，NSH 和游戏之间靠字符串名加 `serde_json::Value` 通信。两边各自手写解析代码，名字拼错、字段对不上都只能到运行时才发现。M2 把通信改成**类型化通道**：协议类型只定义一次，NSH 侧和游戏侧用的是同一个 Rust 类型。

**验收**：`examples/dodge` 是一个纯 Rust 小游戏，HUD、暂停、难度切换和结束面板都用 NSH 写，两边之间不出现任何手写 JSON 解析。

## 四种通道

| 方向 | 持续值（最新值有意义） | 一次性（每次都要处理） |
|---|---|---|
| NSH → 游戏 | `PropKey<T>`：NSH signal 变化后推给游戏，游戏侧变成资源 `T` | `CommandKey<T>`：NSH 发出，游戏侧变成消息 `T` |
| 游戏 → NSH | `StateKey<T>`：资源 `T` 变化后，在帧末推给 NSH，NSH 侧写入 signal | `EventKey<T>`：游戏发出，NSH 侧回调处理 |

所有 key 都是 `const`，只有名字，不带数据。协议模块写法如下：

```rust
pub const DIFFICULTY: PropKey<Difficulty> = PropKey::new("difficulty");
pub const CONTROL: CommandKey<Control> = CommandKey::new("control");
pub const HUD: StateKey<Hud> = StateKey::new("hud");
pub const GAME_OVER: EventKey<GameOver> = EventKey::new("game_over");
```

游戏侧（渲染线程，`nge-ecs`）：

```rust
app.add_prop(DIFFICULTY)      // 资源 Difficulty，NSH 改了就更新
    .add_command(CONTROL)     // MessageReader<Control>
    .add_state(HUD)           // ResMut<Hud> 变了就推送
    .add_event(GAME_OVER);   // 可选：登记后检查重名；发送用 outbox.send(GAME_OVER, &GameOver { .. })
```

NSH 侧（主线程，`nge-nsh`）：

```rust
let game = game_ref();
game_view(game)
    .game_prop(DIFFICULTY, move || difficulty.get())
    .sync_state(HUD, hud)                       // hud: Signal<Hud>
    .on_game_event(GAME_OVER, move |e| over.set(Some(e)))
    .flex_1();
game.send(CONTROL, &Control::Restart);
```

## 关键设计

1. **key 只是类型标签加名字**，定义在 `nge-core::bridge`，只依赖 serde。两边同属一个 crate，同一个类型，编译期就能检查。传输格式仍然是 JSON，走 NSH 原生组件现有的 prop / command / event 通道，所以 M3 的 TSX 可以直接复用同样的名字和结构。
2. **State 由 bevy 的变更检测驱动**：每个 State 对应一个帧末系统，资源 `is_changed()` 才推送，所以不变的资源不产生流量。游戏代码应该用 `set_if_neq` 之类的方式，避免每帧都把资源标成已修改。
3. **触摸和帧统计也走通道**：内置的 `TOUCH: CommandKey<Touch>`、`FRAME_STATS: EventKey<FrameReport>` 和 `ENGINE_ERROR: EventKey<EngineError>`，`game_view()` 默认会把触摸转发过去，示例里不用再手写转发代码。
4. **组件名固定**：`nge_core::GAME_COMPONENT = "nge.game"`，平台层注册时和 `game_view()` 都用这个名字。
5. **解析失败**只打 warn 日志并丢弃这条消息，不会 panic。名字没有注册的 command 仍然作为 `NshCommand` 透传，兼容 M1 的写法；注册过的 command 只会变成类型化消息，不会再出现在 `NshCommand` 里。
6. **名字检查**：State 和 Event 共用 NSH 的事件名空间。注册时会拒绝保留名（`fps`、`error`）和重名，command 会拒绝 `touch` 以及 `nge.` 开头的名字，被拒绝时打 error 日志并忽略这次注册。
7. **prop 的边界情况**：
   - prop 解析失败时，类型化资源保留旧值，但原始的 `Props` 已经更新。
   - NSH 移除 prop 时恢复的是 `T::default()`。
   - prop 在第一次 tick 之前到达时，Startup 能读到它；如果 Startup 里又 `insert_resource` 了同一个类型，会把它覆盖掉。
   - 两个不同的 PropKey 不要指向同一个资源类型。
8. **State 资源被移除时不推送**，NSH 侧会保留最后一次的值。

## 任务拆分

1. `nge-core::bridge`：四种 key、`Touch`、`FrameReport`、`EngineError`，以及编解码（单测）
2. `nge-ecs`：`add_prop` / `add_command` / `add_state` / `add_event`（可选）、`Outbox::send`（单测，不需要 GPU）
3. `nge-nsh`：`game_ref()` / `game_view()` / `GameViewExt::{game_prop, sync_state, on_game_event}` / `GameRef::send`
4. `examples/dodge`：手指左右拖动飞船躲陨石；NSH 显示得分、生命、最高分和 FPS，提供暂停、难度切换和结束面板；游戏逻辑单测
5. 把 `bounce` 和 `sprites` 迁移到新 API
6. Mac 验收；iOS 等手机连上后再测

## 实现备注

- **复审后暂未处理的**：
  - 每个 `on_game_event` / `sync_state` 都单独挂一个 listener，每个事件都要逐个复制一份 JSON；以后 State 很大或推送很频繁时，改成每个视图只挂一个 listener，内部按名字分派。
  - `game_view` 返回的是 `NativeView`，不能阻止用户再调 `.element_ref`。
  - `touches()` 没有单测，因为构造 NSH 的 `TouchEvent` 需要用到内部类型。
- **NSH 侧的方法名**是 `game_prop` / `sync_state` / `on_game_event`：NSH 的 `Element` 和 `Interactive` trait 里已经有 `bind` 和 `on`，用短名字会冲突。
- **`Touch` 带一个 `t` 字段**：值是 NSH 的毫秒时间戳，算甩动速度要用。字段标了 `serde(default)`，没带这个字段的数据也能解析。
- **bounce 保留了底层写法**：它直接实现 `Game` trait、不走 ECS，自己的事件仍然用 `on_native_event` 按名字手动处理，作为底层写法的参考。sprites 已经改成类型化命令（`ADD` / `REMOVE`）和事件（`COUNT`）。

## 验收记录（2026-11-03）

- **单元测试**：
  - `nge-core::bridge` 4 个。
  - `nge-ecs` 的通道绑定 5 个（prop 变成资源、command 变成消息、State 只在变化时推送、Startup 中插入的 State、类型化事件和触摸）。
  - dodge 游戏逻辑 9 个（暂停、受击和无敌、游戏结束、重开后保留最高分、难度、触摸操控、结束帧分数一致、结束后忽略暂停）。
  - 复审后补了保留名 / 重名检查、prop 先于第一次 tick 到达、已注册 command 不再透传，这几项测试。
- **Mac（release，无人操作跑了 75 秒）**：NSH 侧依次收到 `hud`（分数约每 0.1 秒推一次，生命 3 → 2 → … → 0）和一次 `game_over`；游戏结束后 HUD 不再推送。帧率 120fps。
- **迁移**：sprites 和 bounce 已迁移到 `game_view()`，帧率和事件都正常。
- **iPhone**：还没测。测试命令：`NGE_EXAMPLE=dodge examples/bounce/scripts/ios-device.sh`。
