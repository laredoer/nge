# 暂停和时间缩放

> 前置：M1（ECS、`Time`）、M3（TSX 声明式场景）。

## 目标

TSX 一个字段就能让游戏停下来或者变慢：物理、补间、帧动画、`spin`、粒子、飘字、相机平滑和震屏都按同一个时间走。

**验收标准**：`examples/scene` 按空格暂停 / 继续（HUD 显示「已暂停」），暂停时石头、太阳、飞船的补间都停住，继续后接着动。

## Rust

`Time` 增加两个字段：

- `scale`：时间缩放，默认 1；0 是暂停，0.5 是慢一半。`delta` = 这一帧的真实时间 × `scale`，`elapsed` 按缩放后的时间累计。改了从下一帧起生效（这一帧的 `delta` 已经算好）。
- `real_delta`：这一帧的真实时间（同样被截断到 50 ms 以内），暂停时也在走，给暂停菜单的动画之类用。

`scale` 是负数时当暂停，NaN 或无穷大时当 1，不让时间倒流。`frame` 照样每帧加一。

所有引擎系统本来就只读 `delta`，所以不用逐个改。

## TSX

```ts
scene: { timeScale?: number, … }  // 默认 1
```

- 声明的值变了才写 `Time.scale`（和相机一样），Rust 系统也可以自己改 `Time.scale`；`scene` 里的 `timeScale` 一变就以声明为准。
- 声明变了的那一帧就生效：场景同步在 PreUpdate，会按新的缩放重算这一帧的 `delta` 和 `elapsed`，Update 里的系统看到的已经是新值。
- 暂停时点击（`tap`）、按键（`onKey`）、command 都照常收，而且马上生效，只是不推进，继续后才动：
  - `burst` / `floatText` 马上生成，停在原处；
  - `tween` 马上接管属性（写了 `from` 的马上跳到起点），被接管的旧补间马上发 `tweenEnd { cancelled: true }`；`duration: 0` 的补间马上完成；带 `delay` 的补间暂停期间一直在等，继续后才开始计时；
- 相机：暂停时震屏停在当前偏移（暂停中收到的 `shake` 也是马上偏一下停住），继续后接着抖；`smooth > 0` 的跟随不动（但暂停中新设的 `follow` 或刚出现的目标照样第一帧直接对准），`smooth: 0` 的跟随照样每帧对准（目标本来也不动）。
- 声音不受影响：背景音乐继续放，`sound` 照样出声。要静音就同时改 `audio.volume`。
- `fps` 统计按真实时间算，不受影响。

## 不做

- 只暂停一部分（比如 UI 层的精灵照样动）：需要时再给节点加「不受时间缩放影响」的开关。
- 暂停时自动降低帧率省电。

## 验收记录（2026-11-04，Mac）

- **单元测试**：`nge-ecs` 新增 1 个（缩放、暂停、`real_delta`、负数 / NaN / 无穷）；`nge-scene` 新增 3 个（声明暂停当帧生效、Rust 改的缩放在声明没变时保留、去掉声明回到 1；没声明时不覆盖 Rust 的缩放、负数当暂停、整个 `scene` 去掉回到 1；暂停几帧再继续，物理不落、补间的 delay 不被吃掉、帧动画不换帧）。
- **修过的问题**：修了场景第一次同步覆盖 Rust 设的缩放、缩放规则写了两份、文档里暂停时 command 的表现说得不准。
- **TS**：`nsh check` 通过 `examples/scene`、`examples/physics`、`examples/scene3d`。
- **iOS**：`cargo check --workspace --target aarch64-apple-ios` 通过。
- **实跑**：`scene`（`nsh dev --server-only`）在 Mac 上启动，dev server 显示 macOS host 已连接；空格暂停的效果还没手测。
