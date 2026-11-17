# 3D 物理：刚体、碰撞体、碰撞事件、射线

> 前置：基础 3D（[3d-more.md](3d-more.md)）、2D 物理（[physics2d.md](physics2d.md)）。

## 目标

3D 模型现在只能靠 2D 刚体带着动（平台跳跃的箱子挂在一个看不见的 2D 刚体上，见 [joints.md](joints.md)），没法在三维里掉落、翻滚、互相堆叠。

**验收标准**：
- Rust 侧：给带 `Transform3d` 的实体加刚体、碰撞体，固定步长模拟，动态刚体的位置、旋转、速度写回组件；碰撞时发消息；能打射线。
- 场景侧：模型节点写 `body` 就有刚体，`collide` 事件、`push` 命令对模型也管用。
- 平台跳跃里有一摞 3D 箱子，角色跑过去时被撞散，翻滚着掉到地上。

## 做法

### 新 crate `nge-physics3d`

依赖 rapier3d 0.36（内部用 glam 0.33，和引擎的 glam 0.34 之间按数组转换）、`nge-render3d`（`Transform3d`）。结构和 `nge-physics2d` 一样，只做最常用的一部分。

| 组件 | 内容 |
|---|---|
| `RigidBody` | `Dynamic` / `Fixed` / `Kinematic`。位置取 `Transform3d` 的平移和旋转，不看缩放 |
| `Collider` | `shape`：`Shape::Box(Vec3)`（整个长宽高）、`Shape::Ball(f32)`（半径）、`Shape::Capsule { half_height, radius }`（竖着的胶囊）；`restitution`（0）、`friction`（0.5）、`density`（1）、`sensor`、`events` |
| `Velocity` | `linear: Vec3`、`angular: Vec3`（弧度 / 秒，绕各轴）。动态刚体每步写回；游戏改了就推给 rapier |
| `Impulse` | `Vec3`，下一步施加后清零 |

- 资源 `Physics3d`：
  - `gravity` / `set_gravity`、`velocity(entity)`、`contacts(entity)`（接触的实体和法线，法线从对方指向自己）。
  - `raycast(origin, direction, max_distance, filter) -> Option<RayHit>`，规则和 2D 一样（起点在里面算立刻碰到，法线是 `-direction`）。
  - `len()`：刚体数、碰撞体数。
- 消息 `Collision3d { a, b, started }`：其中一个开了 `events` 才发。
- `Physics3dPlugin { gravity, length_unit }`：默认重力 `(0, -9.81, 0)`、`length_unit` 1（一米一个单位）；场景按 point 用时换成 `(0, -981, 0)` 和 100。
- 同步和 2D 一样：
  - 每帧之前把游戏改过的 `Transform3d` / `Velocity` 推给物理：动态和固定刚体瞬移，kinematic 把这一帧的位移平分到每一步（平移线性插值、旋转球面插值）。
  - 走完把刚体的位置、旋转、速度写回（kinematic 的速度是 rapier 按位移算的）；游戏改的速度只推给动态刚体。
  - 游戏写的旋转先归一化。
  - 删实体、去掉组件时删刚体；只有 `Collider` 的当成固定碰撞体；种类或形状变了就重建。
  - 固定步长 1/60，每帧最多补 4 步。系统在 `PostUpdate` 的 `Physics3dStepped` 里。
- 画面不插值：`Transform3d` 直接是这一步结束时的位置。3D 刚体在 120 Hz 屏上会一帧动一帧不动，要做插值时再说。

### 场景（`nge-scene`）

- `ModelNode.body?`：
  - `kind`（`"dynamic"` / `"fixed"` / `"kinematic"`），`shape`（`"box"` / `"ball"` / `"capsule"`）。
  - 大小：`size: [x, y, z]`、`r`、`h`（胶囊中间那段的高）。没写时按模型的 `scale`：盒子边长是 `scale`，球半径 `scale / 2`，胶囊半径 `scale / 4`、中间高 `scale / 2`（总高 `scale`）；引擎里的模型大多是一个单位大小。
  - `restitution`、`friction`、`density`、`sensor`、`velocity: [x, y, z]`、`collide`。
- 有 `body` 的模型，位置和旋转归物理管：声明的位置、角度、缩放没变时不拉回去（和精灵一样），变了就瞬移过去、重新设初速度。`attach`、`spin` 和 `body` 一起写时 `body` 说了算。
- `scene.physics3d?: { gravity: [x, y, z] }`，默认 `(0, -981, 0)`，单位和 2D 一样是 point。
- `collide` 事件 3D 也发（`a`、`b` 是模型的 id）；`push` 命令多一个可选的 `z`，id 是模型时推 3D 刚体（模型和精灵同名时推模型，id 别重名）。
- 模型删掉时它的停止接触发不出 `collide`（已经查不到 id 了）。

### TSX

- `ModelNode.body`（`ModelBody`）、`Scene.physics3d`、`push` 的 `z`。

### 示例（平台跳跃）

- 第一关第一个坑后面的地面上，关卡平面后面一点（z = -20）摆一个看不见的 3D 地面（模型缩到看不见，刚体大小单独写）和一摞六个边长 14 的 3D 箱子（底下三个、中间两个、顶上一个，之间留 1 的缝）。3D 重力和 2D 一样是 1800。
- 箱子左边一点摆一个看不见的 2D sensor；角色碰到它时，游戏把几个箱子往右上方 `push`，前后错开，箱子翻滚着散落一地。
- 箱子的 id 带着摞的次数，按 R 重来、换关时重建，回到摞着的样子。

后来加的：场景这边的系统在 `nge-scene` 的 feature `physics3d` 后面（默认开；工作区里用到的示例自己开），不用的游戏关掉能让 web 包小一些，见 [web.md](web.md)。

## 不做

- 3D 关节、3D 角色控制器、调试画线、射线以外的空间查询。
- 2D 和 3D 刚体之间的碰撞：两个世界互不相干。
- 画面插值（见上面）。
- 按模型网格生成碰撞体（凸包、三角网格）。

## 验收

- `nge-physics3d` 单元测试：
  - 掉到地面上停住；落地时有碰撞消息；`sensor` 只报重叠不挡。
  - 斜着放的盒子会翻过来；初速度、角速度、冲量；kinematic 按 `Transform3d` 平滑移动并推开动态刚体；动态刚体被游戏挪了是瞬移。
  - 去掉组件、删除实体时物理里也删掉；改形状重建；接触的法线方向；射线（含起点在里面、过滤自己、sensor）。
- `nge-scene`：模型节点的 `body` 建出刚体、默认大小按 `scale`；不改位置时不拉回；`push` 带 `z` 推 3D 刚体；3D 碰撞发 `collide` 事件。
- 平台跳跃：TSX 测试检查箱子摞在地面上、sensor 在箱子下面；Rust 测试检查角色跑过去后箱子散开。
- 变异检查：法线方向、sensor 和过滤、各种形状的大小、弹性、事件开关、初始位置和角速度、补质量、kinematic 平分位移和转角、瞬移、推速度和冲量、挪固定碰撞体、停止接触、删实体时的停止接触、旋转归一化、最多补几步、速度写回；场景的种类、默认大小、不拉回、重新放（位置、角度、刚加上刚体）、设初速度、去掉 body、重力、`push` 的 `z`、刚体压过 `attach`，改掉都有测试报错。
  - 射线起点在里面时的法线：parry 这时给的法线就是 0，不判断也一样。
  - 组件的值没变时不重建：重建了结果也一样，只是白做一次。
  - 速度只推给动态刚体：推给 kinematic 也会被 rapier 按位移覆盖，结果一样。
- 还没手测：箱子散开的样子。

## 修过的问题

- 按 R 重来时箱子没重新摞起来：id 用的是关卡的局数，重来不换关、局数不变。改成单独数摞的次数，重来时加一。
- kinematic 也被推了速度（rapier 随后按位移覆盖掉，白做）；只推给动态刚体。
- 游戏手写没归一化的四元数直接进了 rapier；先归一化。
