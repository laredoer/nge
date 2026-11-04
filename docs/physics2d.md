# 2D 物理

> 前置：TSX 场景节点（[scene-nodes.md](scene-nodes.md)）。

## 目标

接入 rapier2d，让精灵有刚体和碰撞体：Rust 侧是几个组件，TSX 侧是精灵节点上的 `body` 字段；碰撞时给 Rust 发消息、给 TSX 发 `collide` 事件。

**验收标准**：新示例 `examples/physics`，场景全部在 `App.tsx` 里：
- 地面和两面墙是静态刚体，场景里有几个会掉下来、互相堆叠的箱子；
- 点空地在点击处生成一个弹球；点中弹球把它往上弹（`push`）；
- 弹球撞到东西时 HUD 上的碰撞次数增加；
- 有一个按钮一次加 200 个球；Mac 上 1000 个动态刚体时一步物理模拟的耗时在 2 ms 以内（release），保持 120 FPS。

## 新 crate `nge-physics2d`

依赖 rapier2d 0.36（内部用 glam 0.33，和引擎的 glam 0.34 之间按数组转换），用它的 `PhysicsWorld`。

### 组件

| 组件 | 内容 |
|---|---|
| `RigidBody` | `Dynamic` / `Fixed` / `Kinematic`。位置取实体的 `Transform2d`（平移和旋转，不看缩放） |
| `Collider` | `shape: Shape::Rect(Vec2)`（整个宽高）或 `Shape::Circle(f32)`（半径），`restitution`（默认 0）、`friction`（0.5）、`density`（1）、`sensor`（false）、`events`（false，碰撞时发消息） |
| `Velocity` | `linear: Vec2`、`angular: f32`（弧度 / 秒）。动态刚体每步写回；游戏改了就推给 rapier |
| `Impulse` | `Vec2`，下一步施加到刚体上后清零 |
| `LockRotation` | 锁住旋转（平台游戏的角色），加上去掉都不重建刚体；见 [platformer.md](platformer.md) |

- 单位：世界坐标的 point。`PhysicsPlugin` 的 `pixels_per_meter`（默认 100）交给 rapier 的 `length_unit`，让它的容差按 point 缩放；重力默认 `(0, -981)` point / s²。
- 同步方向：
  - 每步之前：游戏或场景改过 `Transform2d`（和上一步写回的值不一样）就把刚体挪过去（动态刚体相当于瞬移，`Kinematic` 用 `set_next_kinematic_position` 平滑移动）；`Velocity` 同理。
  - 每步之后：把动态刚体的位置、速度写回 `Transform2d` / `Velocity`。
- 实体删除或去掉 `RigidBody` 时删除刚体和它的碰撞体；只有 `Collider` 没有 `RigidBody` 的实体当成固定碰撞体。
- 固定步长 1/60 秒，每帧按 `Time.delta` 累计，最多补 4 步（卡顿时宁可变慢也不越补越多）。
- 画面插值：`Transform2d` 是真实值（动态刚体是物理这一步结束时的位置，kinematic / 固定刚体是游戏给的），物理每帧另外给有刚体的实体写 `DrawAt`（画在哪）：动态和 kinematic 在上一步和这一步之间按攒下没走的时间插值（比物理慢一步，约 16 ms，两种刚体慢的一样多，站在移动平台上贴得住），固定刚体就是 `Transform2d`。精灵渲染和相机跟随用 `DrawAt`。不插值时 Mac 的 120 Hz 屏幕上刚体一帧动一帧不动，帧时间在 1/60 上下抖时会一帧走 0 步、下一帧走 2 步。详见 [moving-platforms.md](moving-platforms.md)（第一版把插值直接写进了 `Transform2d`，游戏读到的位置慢一步，kinematic 没法跟着插值，改掉了）。
- 系统排在 `PostUpdate` 的 `PhysicsStepped` 里：游戏在 `Update` 里改速度、施加冲量，这一帧就生效。

### 接触查询

`Physics::contacts(entity)` 返回上一步结束时正在接触的碰撞体和法线（从对方指向自己），用来判断站没站在地上；见 [platformer.md](platformer.md)。

### 碰撞消息

`Collision { a: Entity, b: Entity, started: bool }`（`MessageReader<Collision>` 读）。两个碰撞体里有一个开了 `events` 就发。接触中删掉实体时，下一步发停止接触（这时 `a` / `b` 里有一个已经删掉，`query.get` 会查不到；TSX 的 `collide` 因为要按实体找节点 id，收不到这一条）；碰撞体重建（形状、材质变了）时先发停止再发开始。

## TSX

精灵节点加一个 `body` 字段：

```ts
{ id: "ball", x: 0, y: 200, w: 40, h: 40, texture: "asset:/…",
  body: { kind: "dynamic", shape: "circle", restitution: 0.6, collide: true } }
```

| 字段 | 内容 |
|---|---|
| `kind` | `"dynamic"`（默认）/ `"fixed"` / `"kinematic"` |
| `shape` | `"box"`（默认）/ `"circle"` |
| `w` / `h` / `r` | 碰撞体大小，不填时取精灵的 `w` / `h`（圆取较短边的一半）；和精灵一样只给 `w` 或 `h` 时按正方形。都没有时只建刚体、不建碰撞体，记一条 warn |
| `restitution` / `friction` / `density` / `sensor` | 同 `Collider` |
| `velocity` / `angularVelocity` | 初速度（point / s，度 / s） |
| `collide` | 碰撞时发 `collide` 事件 |

- **位置只在生成时和声明的位置变了时覆盖**：刚体落下以后，TSX 改了这个精灵的颜色之类的别的字段，不会把它拉回声明的 x / y。初速度只在生成、`velocity` 变了、或者声明的位置变了（相当于把它重新放一次）时设置。
- 有 `body` 的精灵写了 `spin` 时不转，记一条 warn（`spin` 每帧改 `Transform2d`，物理会把它当成瞬移）。
- `push` 只对 dynamic 刚体有用，推 fixed / kinematic 时记 warn。
- `scene.physics`：`{ gravity: [x, y] }`，不填用默认重力。
- 事件 `collide`：`{ a, b, started }`，`a` / `b` 是节点 id。只在两个都是场景节点、至少一个开了 `collide` 时发。
- 命令 `push`：`{ id, x, y }`，给这个节点的刚体加上 (x, y) 的速度（point / s，和质量无关，游戏里比冲量好用）。TS 里用 `push(node, …)`。

## 实现上的决定

- `nge-scene` 直接依赖 `nge-physics2d`（不做 feature 开关，少一种组合要测），`ScenePlugin` 没加 `PhysicsPlugin` 时自己补上（和粒子、文字一样）。
- 实体和刚体 / 碰撞体的对应关系存在 `Physics` 资源的表里（实体删掉以后组件就读不到了），表里还记着上一次同步的位置和速度，用来判断游戏改没改；rapier 的 `user_data` 存 `Entity` 的 bits，碰撞时反查实体。
- `RigidBody` / `Collider` 用同样的值重新插入（场景每次覆盖节点都会这样）不重建；种类或形状变了才删掉重建，重建时没有 `Velocity` 组件就沿用原来的速度。
- 没有碰撞体、或者碰撞体面积 / 密度为 0 的动态刚体按质量 1 算（不然不受重力，冲量也没用）；转动惯量还是 0，碰撞不会让它转起来。
- kinematic 在一帧补多步时，位移平分到每一步，写回的速度是真实速度。
- 角度写回时按差值累加，不规范到 (-π, π]，游戏看不到跳变。
- `Time.delta` 是负数或 NaN 时这一帧不攒时间。
- 场景同步节点时不覆盖 Rust 系统设了还没用掉的 `Impulse`。
- `bevy_ecs` 的消息只留两帧：游戏要每帧读 `Collision`，不然会漏。

## 范围外

- 关节、射线 / 形状查询给 TSX、3D 物理
- 确定性回放
- 多边形 / 胶囊碰撞体、按贴图轮廓生成碰撞体

## 验收记录（2026-11-04，Mac）

- **单元测试**：`nge-physics2d` 12 个（动态刚体下落、固定刚体不动、箱子停在地面上、只有开了 `events` 的碰撞体发消息且挪开后发停止接触、游戏瞬移 / 改速度 / 施加冲量、kinematic 跟着 `Transform2d` 走并推开别的刚体、同值重新插入不重建 / 换种类 / 去掉刚体只剩碰撞体 / 删除实体、固定步长攒零头、没有质量的动态刚体也会落、接触中删除 / 重建碰撞体发停止接触、kinematic 在多步的帧里平分位移、角度不被规范化、坏的帧时间不冻住模拟）；`nge-scene` 新增 4 个（刚体下落且改别的字段不被拉回、改了声明的位置就瞬移并重设速度、碰撞体大小的默认值和缩放、去掉 `body` 时清掉物理组件、`collide` 事件和 `push`、`scene.physics.gravity`）；`examples/physics` 2 个（用和 `App.tsx` 一样的 JSON：一摞箱子三秒后没倒、球停在地上并发出撞地面的 `collide`、点空地打到 `sky`、`push` 把球弹起来）。
- **性能**（release，`cargo test -p nge-physics2d --release -- --ignored`）：1000 个球从生成开始一直在下落、互相挤，物理一步平均 0.30 ms，最长 2.1 ms（第一帧含建 1000 个刚体）。整个示例 `update`（场景同步 + 物理 + 粒子）1000 个球平均 0.54 ms，最长 2.6 ms。
- **TS**：`nsh check` 通过 `examples/physics` 和 `examples/scene3d`。
- **离屏截图**：60 个球时箱子摞着、球堆在地上；1000 个球时容器堆满。
- **实跑**：`nsh dev --server-only` + `cargo run -p physics --release`，dev server 显示 macOS host 已连接。

## 已知限制 / 后续

- 碰撞体不跟着 `Transform2d.scale` 变（场景节点在生成时按 `scale` 算好尺寸，之后改 `scale` 会整体重建）。
- 没有插值：渲染帧率高于 60 时，刚体每两帧动一次（120 Hz 屏上能看出来就加插值）。（后来加了，见 [moving-platforms.md](moving-platforms.md)。）
- 掉出世界的刚体不会自动删除。
- `collide` 事件不带接触点和力度。
