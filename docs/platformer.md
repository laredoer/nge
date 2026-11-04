# 平台跳跃示例

> 前置：键盘（[keyboard.md](keyboard.md)）、相机（[camera.md](camera.md)）、瓦片地图（[tilemap.md](tilemap.md)）、2D 物理（[physics2d.md](physics2d.md)）。

## 目标

`examples/platformer`：一张 80 × 15 格的关卡，← → / A D 跑、空格 / ↑ / W 跳，吃金币、躲坑，碰到终点旗子通关。关卡、金币、终点、相机全部在 `App.tsx` 里声明；每帧的角色控制写在 Rust 里（引擎的分工：TSX 不跑每帧逻辑）。

**验收标准**：Mac 上能从起点玩到终点：相机跟着角色、到关卡两头停住；能跳上平台和台阶；掉进坑回到起点，HUD 记次数；金币吃掉就消失并飘「+1」，HUD 计数；碰到终点 HUD 显示用时。

## 物理层新增

### `Physics::contacts(entity) -> Vec<Contact>`

`Contact { other: Entity, normal: Vec2 }`：上一步结束时这个实体的碰撞体正在接触的碰撞体，`normal` 是从对方指向自己的单位向量（站在地上时是 `(0, 1)`，贴着左边的墙是 `(1, 0)`）。只算交给求解器的接触点（含很近还没碰上的预测点，「着地」会提前零点几个 point），sensor 的重叠不算；实体还没建到物理里时是空的。`other` 是碰撞体所在的实体：碰到瓦片地图时是地图下面的碰撞体子实体，要找地图得经过 `ChildOf`。角色控制用它判断「脚下有地」（`normal.y > 0.7`），墙的法线是水平的，所以贴着墙不能跳。

### `LockRotation` 组件 / `body.fixedRotation`

锁住刚体的旋转，碰撞不会让它转起来；加锁时原来的角速度也清零（rapier 只把转动惯量当无穷大，不管已有的角速度）。加上、去掉都在下一次物理同步时生效（去掉时把 `RigidBody` 标成变了，`create` 里解锁），不重建刚体。TSX 精灵的 `body.fixedRotation: true` 对应这个组件。

## 角色控制（`examples/platformer/src/lib.rs`）

- 按 id `player` 找到角色精灵，每帧在 `Update` 里改它的 `Velocity`（物理在 `PostUpdate`，这一帧就生效）。
- 水平：朝 `axis(←/A, →/D) × 260` 加速，地上 3000、空中 1500 point / s²。
- 跳：起跳速度 640（场景重力 1800，整跳大约 114 point，3 格半）；走出平台边 0.1 秒内还能跳（土狼时间），落地前 0.12 秒内按的跳也算（跳跃缓冲）；上升时跳跃键全松开了速度减半，轻按跳得矮；靠缓冲起跳时键已经松开就直接按短跳。
- 着地 = 竖直速度 ≤ 1 且有一个接触的法线 `y > 0.7`：刚起跳那一帧之后还可能有地面的预测接触点，往上走时不算着地，免得一次按键跳两下。
- 时间缩放为 0（暂停）或帧时间不是正数时整个控制不动：暂停中按的键不会留到继续以后。
- 掉到 y < −400：送回第一次见到角色的位置，速度清零，发 `respawn { falls }` 给 TSX。
- 关卡的 `friction` 是 0：角色的水平速度由控制决定，地面摩擦只会让最高速度比设定的低。

## TSX（`examples/platformer/App.tsx`）

- 关卡用一个 `tileAt(列, 行)` 函数生成 `data`：两头石墙、一行草地带三个坑、六块木平台、一段石台阶。坑边正上方不放平台（角色 32 高，平台底边低于 96 时从地面起跳会撞头掉坑）。
- 金币是 `sensor` 的固定刚体；角色的 `body.collide` 开着，所以穿过金币时发 `collide`，TSX 把它从列表里去掉、`floatText` 飘「+1」。终点也是 sensor。
- 相机 `follow: "player"`，`bounds` 是整张关卡（下面多留 160 让坑看得见）。

## 不做

- 敌人、移动平台、关卡切换、存档、音效：需要时再加。
- 暂停时松开跳跃键：控制在暂停时不跑，那一帧的「松开」丢了，继续后这一跳按整跳走完。很少碰到，先不管。
- 重新开始：现在只能重启示例。重生点是控制器第一次见到角色时的位置，TSX 改了角色的声明位置或者重新生成角色都不会更新它。

## 验收记录（2026-11-04，Mac）

- **单元测试**：`nge-physics2d` 新增 4 个（`contacts` 报对方和法线、sensor 不算接触；`LockRotation` 让箱子在台阶边不倒、去掉后会倒、加锁时清掉已有的转速）；`nge-scene` 新增 1 个（`fixedRotation` 加上 / 去掉）；`examples/platformer` 9 个（走和停、只能从地上跳且松键截短、多个跳跃键全松开才截短、跳跃缓冲、土狼时间、贴墙不能跳、掉坑重生发 `respawn`、穿过金币发 `collide`、暂停时不响应按键）。截短的两个用例用变异实验确认过：去掉截断或去掉「全松开」条件测试会挂。
- **修过的问题**：修了解锁旋转的测试太弱、第二个坑上方的平台会撞头、控制没挡住暂停和坏帧时间、起跳后一帧的预测接触点导致连跳、截短的测试上界和整跳贴在一起测不出截断。
- **TS**：`nsh check` 通过 `examples/scene`、`examples/physics`、`examples/scene3d`、`examples/platformer`。
- **iOS**：`cargo check --workspace --target aarch64-apple-ios` 通过。
- **实跑**：`platformer`（release + `nsh dev --server-only`）在 Mac 上启动，dev server 显示 macOS host 已连接、热重载正常；从起点玩到终点的手感还没手测。
