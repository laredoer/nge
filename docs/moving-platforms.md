# 移动平台和渲染插值

> 前置：2D 物理（[physics2d.md](physics2d.md)）、平台跳跃示例（[platformer.md](platformer.md)）。

## 目标

- 移动平台：TSX 里一个 kinematic 精灵加一个来回的补间就是移动平台，角色站上去跟着走、不陷进去、不打滑。
- 插值不再改 `Transform2d`：游戏读到的位置就是物理的真实位置，相对移动（`translation.x += …`）照常能用。

**验收标准**：`examples/platformer` 加一块左右移动、一块上下移动的平台（在坑里，见下面「上下移动的平台」）；角色站上去跟着走，画面上脚和平台贴着，不抖、不陷；在 120 Hz 的 Mac 上平台和角色都是每帧都动。

## 渲染插值：`DrawAt`

上一版把插值写进了动态刚体的 `Transform2d`，结果游戏读到的位置比物理慢一步，于是瞬移、只改角度、重建刚体都要特殊处理；kinematic 的 `Transform2d` 是游戏给的目标，没法这么插值，就和动态刚体差了一步。

改成：

- `Transform2d` 永远是真实值：动态刚体是物理这一步结束时的位置，kinematic 和固定刚体是游戏给的位置。
- 新组件 `nge_render2d::DrawAt { translation, rotation }`：「画在哪」。物理每帧给每个有刚体的实体写一次（没有就插上）：
  - 动态和 kinematic 刚体：上一步和这一步结束时的位置之间，按攒下没走的时间占一步的比例插值（比物理慢一步，约 16 ms）。两种刚体慢的一样多，所以站在平台上贴得住。
  - 固定刚体：就是 `Transform2d`。
  - 去掉刚体时一起去掉。
- 精灵渲染和相机跟随有 `DrawAt` 时用它，没有时用 `Transform2d`。缩放和 z 照样取 `Transform2d`。
- 游戏瞬移动态刚体：画面直接画在新位置（不够一步的帧也是，每帧拖着走时每帧都跟手），下一步推给物理时插值的两头都换成新位置，不会从旧位置滑过去。kinematic 不这样：它的 `Transform2d` 是目标，照样插值。
- 刚体只换了形状（种类不变、游戏也没挪它）重建时，画面接着原来的插值走，不往前跳一步。
- 点击（`tap`）按 `DrawAt` 判断点没点中，和画面一致。
- 文字、粒子（`floatText` 的坐标是 TSX 给的）不看 `DrawAt`：它们不挂刚体。

## 移动平台（TSX）

```ts
{ id: "lift", x, y, w: 96, h: 16, body: { kind: "kinematic", friction: 0 } }
// 挂上以后发一次：来回走、一直重复
tween(game, { id: "lift", to: { x: x + 160 }, duration: 2, ease: "easeInOut", repeat: -1, yoyo: true })
```

- kinematic 刚体的补间本来就能改位置（见 [animation.md](animation.md)）。
- 物理把这一帧的位移平分到每一步，写回的 `Velocity` 是平台的真实速度。
- 上下移动的平台：动态刚体站在上面会被托着走（物理自己处理）。
- 左右移动的平台：地面摩擦是 0 的游戏（比如平台跳跃，角色的水平速度归控制管）要自己把脚下平台的速度加进去，不然平台从脚底下滑走。平台跳跃的控制按 `contacts` 找脚下的东西（法线朝上的），用新加的 `Physics::velocity(entity)` 取它的速度（kinematic 没有 `Velocity` 组件也查得到），把水平速度加到目标速度上；同时踩着几样时取动得快的那个。
- 上下移动的平台不要让底下能站人：kinematic 降下来会把动态刚体压进固定的地面里（平台跳跃把它放在坑里）。

## 实现

- `nge-render2d`：`DrawAt` 组件；精灵渲染查询里带 `Option<&DrawAt>`。
- `nge-physics2d`：
  - `Handles` 的 `prev` / `curr` 对动态和 kinematic 都记。
  - 每帧（包括不够一步的帧）写 `DrawAt`。
  - `Transform2d` 的写回回到原来的样子（真实值）。上一版为插值加的「不够一步的帧不覆盖游戏的改动」「重建时用物理里的真实位置」「位置和角度分开比较」都不需要了，去掉。
  - `remove` 里去掉刚体时一起去掉 `DrawAt`。
- `nge-scene`：`camera_rig` 跟随、`tap` 点击时读 `DrawAt`。

## 不做

- 平台带着角色转（旋转的平台）、单向平台（从下面跳上去）、沿多段路径走：需要时再加。
- 文字 / 粒子跟着刚体走时的插值：现在没有这种用法。

## 验收记录（2026-11-04，Mac）

- **单元测试**：`nge-physics2d` 新增 6 个（上升平台上的箱子画面上贴着平台：去掉 kinematic 插值时陷进去约 2 px 会挂；固定刚体的 `DrawAt` 跟着 `Transform2d`、去掉刚体时一起去掉；每帧拖着动态刚体时每帧都画在新位置；只换形状重建时画面不跳；`Physics::velocity`），原来的插值测试改成看 `DrawAt`、`Transform2d` 回到真实值；`nge-scene` 新增 2 个（相机跟随画的位置、点击按画的位置）；`examples/platformer` 新增 2 个（站在横移的平台上跟着走、上升的平台上能起跳）。
- **修过的问题**：修了竖平台降下来把站在底下的角色压进地里（挪到坑里）、不够一步的帧里拖着的动态刚体画面晚一帧、只换形状重建时画面往前跳、同时踩地面和平台时随便取一个、点击还按 `Transform2d`、`insert` 理论上会 panic，文档里的缓动名写错和几处过时的说法。
- **TS**：`nsh check` 通过 `examples/scene`、`examples/physics`、`examples/scene3d`、`examples/platformer`。
- **iOS**：`cargo check --workspace --target aarch64-apple-ios` 通过。
- **实跑**：`platformer`（release + `nsh dev --server-only`）在 Mac 上启动，dev server 显示 macOS host 已连接，没有 warn。站在平台上的手感还没手测。
