# 动画：帧动画和补间

> 前置：M3（TSX 声明式场景）、TSX 场景节点（[scene-nodes.md](scene-nodes.md)）、2D 物理（[physics2d.md](physics2d.md)）。

## 目标

TSX 里不写每帧逻辑，也能让精灵动起来：图集精灵按帧循环播放；用一条 command 让精灵在一段时间里平滑地移动、旋转、缩放、渐隐，播完通知 TSX。

**验收标准**：`examples/scene` 改成：
- 一圈石头在图集里按帧循环播放（`anim`）；
- 点飞船：飞船用补间滑到一个随机位置（缓出），同时缩放弹一下（往返一次）；到了以后 HUD 上「到达」次数加一（`tweenEnd` 事件）；
- 飞船位置不再由 TSX 的 signal 驱动。

## TSX

### 精灵的 `anim`

`sprites[].anim`：`{ frames: number[], fps = 10, loop = true }`，只对图集精灵（`grid` 或注册的图集）有用。

- 有 `anim` 时引擎按时间换 `frame`，节点上的 `frame` 不起作用。
- `loop: false` 时停在最后一帧，发一次 `animEnd { id }` 事件。
- 节点别的字段变了（比如 `x`）不会让动画从头开始；`anim` 本身变了才从头开始。
- `frames` 为空、`fps` 不是正数时不播，画 `frame`。`frames` 里有负数或小数时整份 `scene` 解析失败（和别的字段一样，只记 warn）。

### `tween` command

```ts
tween(game, { id, from?: { … }, to: { x?, y?, rotation?, scale?, alpha? }, duration = 0.3, delay = 0, ease = "easeOut", repeat = 0, yoyo = false, tag? })
```

- 目标是 `sprites` 里 id 为 `id` 的精灵。从开始那一刻（`delay` 过后）的当前值变到 `to`；`from` 里写了的属性用写的值当起点（比如缩放弹一下写 `from: { scale: 1 }`，连点打断时也会回到原来大小）；`from` 里有、`to` 里没有的属性不起作用。`from` / `to` 里写了别的字段（比如 `opacity`）时整条命令无效，`to` 里什么都没有时记 warn。`rotation` 是度，`alpha` 乘到颜色的透明度上（替换 `color` 的 alpha）。
- `ease`：`"linear"` / `"easeIn"` / `"easeOut"` / `"easeInOut"`（三次）/ `"backOut"`（冲过头再回来）/ `"bounceOut"`（落地弹跳）/ `"elasticOut"`（弹簧）。
- `repeat` 是额外再播几次，`-1` 是一直播；`yoyo` 为真时来回播（第二、四……次从 `to` 回到起点），所以 `yoyo` 要配 `repeat ≥ 1`，偶数次播完回到起点。
- 结束时发 `tweenEnd { id, tag, cancelled }` 事件（`tag` 原样带回，用来区分是哪一个补间）。一直播的不会正常结束，但被接管时也会发 `cancelled: true`。
- 同一个精灵上可以同时有几个补间（比如一个移动、一个渐隐）。补间**开始**（`delay` 过后）时接管它写到的属性：已经开始的旧补间不再写这些属性，所以带 `delay` 的新补间在等待期间不会让精灵停住。旧补间所有属性都被接管时删掉，发 `cancelled: true` 的 `tweenEnd`（比如连点时上一次的移动）；同一帧开始的几个补间，后发的赢；还在 `delay` 里的补间不会被接管，等它自己开始时再接管别人。
- `duration` 不是正数时，`delay` 过后直接跳到终点（这时忽略 `repeat`）。
- 有 dynamic 刚体的精灵：位置和旋转归物理管，补间里的 `x` / `y` / `rotation` 记 warn 不管，`scale` / `alpha` 照常（只剩位置和旋转的整条命令丢掉）。kinematic 刚体跟着 `Transform2d` 走，可以补间位置。
- 补间 `rotation` 期间盖过 `spin`，播完 `spin` 从终点接着转。
- 没有这个 id 的精灵时记 warn。

## Rust

`nge-scene` 里新增 `anim` 模块，组件也可以在 Rust 里直接用：

- `FrameAnimation::new(frames, fps, looped)`（不合法时返回 `None`）：挂在有图集 `Sprite` 的实体上，`Update` 里换帧。
- `Tweens`：一组正在播的补间（`Tween::new(to, duration)` 加 builder 设置 from / ease / delay / repeat / yoyo / tag），`Update` 里写 `Transform2d` 和 `Sprite::color`。`scale` 是等比的：起点取 `scale.x`，写回时 x、y 一样。
- 事件都是对有 `SceneId` 的实体才发。

## 实现上的决定

- 补间的起点在开始时（`delay` 过后的第一帧）从实体当前的值取，所以 `delay` 期间别的东西移动了它也没关系。
- 补间系统排在 `spin`、`push` 之后，同一帧里最后写 `Transform2d`。
- TSX 改了节点（同步时整体覆盖 `Transform2d` / `Sprite`）以后，正在播的补间下一帧接着写它管的属性，所以补间期间以补间为准；补间结束后再改节点，就以 TSX 声明为准（和别的 Rust 驱动的东西一样）。
- 帧动画每帧都按累计时间算当前帧写回 `Sprite`，所以同步时覆盖了 `Sprite` 也不会闪回 `frame`。
- 一帧的 `delta` 很大时补间直接算到对应的位置，帧动画按时间取模，不会一帧一帧追。实际运行时 `nge-core` 把每帧的 `delta` 截到 50 ms，所以切到后台回来只往前走 50 ms。

## 范围外

- 文字、3D 模型、相机的补间；颜色（RGB）补间
- 补间串联（时间线）、贝塞尔路径、按名字的动画状态机
- 骨骼动画、glTF 动画

## 验收记录（2026-11-04，Mac）

- **单元测试**：`nge-scene` 新增 15 个（帧动画循环、只改别的字段不从头播、`anim` 变了从头播、很大的 delta、不循环的停在最后一帧并只发一次 `animEnd`、不合法的 `anim` 不挂组件；补间的移动 / 旋转 / 渐隐和 `tweenEnd`、delay 期间不动并从开始时取起点、yoyo + repeat 和一直播、后开始的接管属性并发 `cancelled`、显式 `from` 让连点的缩放回到 1、带 delay 的补间开始时才接管、同一帧开始的后发的赢、补间旋转盖过 `spin`、补间中和补间后改 TSX 节点、dynamic 刚体只补间缩放和透明度、写错字段 / 空 `to` 的命令丢掉、缓动函数的端点）；`examples/scene` 1 个（和 `App.tsx` 一样的声明：石头按帧换颜色，连点两次飞船，第一次的移动和缩放被接管，停在第二次的目标、大小复原）。
- **修过的问题**：修了连点时缩放漂移、带 delay 的补间提前接管、补间和 `spin` 的系统顺序。
- **TS**：`nsh check` 通过 `examples/scene`、`examples/physics`、`examples/scene3d`；`ease` 写错时报类型错误。
- **iOS**：`cargo check --workspace --target aarch64-apple-ios` 通过；没上真机。
- **实跑**：`nsh dev --server-only` + `cargo build -p scene --release`，dev server 显示 macOS host 已连接，日志里没有 warn。
